use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};

use crate::{
    constants::PosixError,
    dev::buffer::{DevId, PhysicalBlock},
    fs::{
        self,
        file::{FileFlags, OpenFiles},
        inode::{inoderef_leak, InodeFlag, InodeMode, Inode, OpenError},
        FileRef, InodeRef,
    },
    sync::SpinExt,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirSearchMode {
    Open = 0,
    Create = 1,
    Delete = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDisposition {
    OpenExisting,
    TruncateExisting,
    CreateNew,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekWhence {
    Set,
    Current,
    End,
    SetBlock,
    CurrentBlock,
    EndBlock,
}

pub struct FileManager {
    pub root_dir_inode: Option<InodeRef>,
    current_dir: String,
    path_index: BTreeMap<String, InodeRef>,
    pending_parent: Option<InodeRef>,
    pending_path: Option<String>,
    pending_name: Option<[u8; DirectoryEntry::DIRSIZ]>,
}

impl FileManager {
    pub fn new() -> Self {
        Self {
            root_dir_inode: None,
            current_dir: "/".to_string(),
            path_index: BTreeMap::new(),
            pending_parent: None,
            pending_path: None,
            pending_name: None,
        }
    }

    pub fn open(
        &mut self,
        inode: InodeRef,
        mode: FileFlags,
        open_files: &mut OpenFiles,
    ) -> Result<(usize, FileRef), PosixError> {
        self.open1(inode, mode, OpenDisposition::OpenExisting, open_files)
    }

    pub fn creat(
        &mut self,
        path: &str,
        create_mode: InodeMode,
        open_files: &mut OpenFiles,
    ) -> Result<(usize, FileRef), PosixError> {
        match self.name_i(path, DirSearchMode::Create)? {
            Some(inode) => {
                let result = self.open1(
                    inode.clone(),
                    FileFlags::FWRITE,
                    OpenDisposition::TruncateExisting,
                    open_files,
                )?;
                inode.lock().i_mode |=
                    create_mode & (InodeMode::IRWXU | InodeMode::IRWXG | InodeMode::IRWXO);
                Ok(result)
            }
            None => {
                let inode = self.mak_node((create_mode & !InodeMode::ISVTX).bits())?;
                self.open1(
                    inode,
                    FileFlags::FWRITE,
                    OpenDisposition::CreateNew,
                    open_files,
                )
            }
        }
    }

    pub fn open1(
        &mut self,
        inode: InodeRef,
        mode: FileFlags,
        disposition: OpenDisposition,
        open_files: &mut OpenFiles,
    ) -> Result<(usize, FileRef), PosixError> {
        if disposition != OpenDisposition::CreateNew {
            let inode_ref = inode.lock();

            if mode.contains(FileFlags::FWRITE)
                && (inode_ref.i_mode & InodeMode::IFMT) == InodeMode::IFDIR
            {
                return Err(PosixError::EISDIR);
            }
        }

        if disposition == OpenDisposition::TruncateExisting {
            inode.lock().i_trunc();
        }

        inode.lock().prele();

        let (fd, file) = match fs::global_open_file_table().f_alloc(open_files) {
            Ok(result) => result,
            Err(err) => {
                fs::global_inode_table().i_put(inode);
                return Err(err);
            }
        };

        {
            let mut file_ref = file.lock();
            file_ref.f_flag = mode & (FileFlags::FREAD | FileFlags::FWRITE);
            file_ref.f_inode = Some(inoderef_leak(inode.clone()));
        }

        let write_mode = if mode.contains(FileFlags::FWRITE) {
            1
        } else {
            0
        };
        let open_result = {
            let inode_ref = inode.lock();
            inode_ref.open_i(write_mode)
        };

        if let Err(err) = open_result {
            open_files.clear_f(fd);
            let mut file_ref = file.lock();
            file_ref.f_inode = None;
            file_ref.f_count -= 1;
            fs::global_inode_table().i_put(inode);
            return Err(err);
        }

        Ok((fd, file))
    }

    pub fn close(&mut self, fd: usize, open_files: &mut OpenFiles) -> Result<(), PosixError> {
        let file = open_files.get_f(fd)?;
        open_files.clear_f(fd);
        fs::global_open_file_table().close_f(&file);
        Ok(())
    }

    pub fn seek(
        &mut self,
        fd: usize,
        offset: i32,
        whence: SeekWhence,
        open_files: &OpenFiles,
    ) -> Result<i32, PosixError> {
        let file = open_files.get_f(fd)?;
        let mut file_ref = file.lock();

        if file_ref.f_flag.contains(FileFlags::FPIPE) {
            return Err(PosixError::ESPIPE);
        }

        let (offset, whence) = match whence {
            SeekWhence::Set => (offset, SeekWhence::Set),
            SeekWhence::Current => (offset, SeekWhence::Current),
            SeekWhence::End => (offset, SeekWhence::End),
            SeekWhence::SetBlock => (offset << 9, SeekWhence::Set),
            SeekWhence::CurrentBlock => (offset << 9, SeekWhence::Current),
            SeekWhence::EndBlock => (offset << 9, SeekWhence::End),
        };

        file_ref.f_offset = match whence {
            SeekWhence::Set | SeekWhence::SetBlock => offset,
            SeekWhence::Current | SeekWhence::CurrentBlock => file_ref.f_offset + offset,
            SeekWhence::End | SeekWhence::EndBlock => {
                let inode = file_ref.f_inode.as_ref().ok_or(PosixError::EBADF)?;
                inode.lock().i_size as i32 + offset
            }
        };

        Ok(file_ref.f_offset)
    }

    pub fn dup(&mut self, fd: usize, open_files: &mut OpenFiles) -> Result<usize, PosixError> {
        open_files.clone_fd(fd)
    }

    pub fn fstat(&mut self, fd: usize, open_files: &OpenFiles) -> Result<FileStat, PosixError> {
        let file = open_files.get_f(fd)?;
        let inode = file
            .lock()
            .f_inode
            .map(|inode| inode.own())
            .ok_or(PosixError::EBADF)?;

        Ok(self.stat1(inode))
    }

    pub fn stat(&mut self, path: &str) -> Result<FileStat, PosixError> {
        let inode = self
            .name_i(path, DirSearchMode::Open)?
            .ok_or(PosixError::ENOENT)?;
        let stat = self.stat1(inode.clone());
        fs::global_inode_table().i_put(inode);
        Ok(stat)
    }

    pub fn stat1(&mut self, inode: InodeRef) -> FileStat {
        let inode_ref = inode.lock();
        FileStat::from(&*inode_ref)
    }

    pub fn read(
        &mut self,
        fd: usize,
        count: usize,
        open_files: &OpenFiles,
    ) -> Result<usize, PosixError> {
        self.rdwr(fd, count, FileFlags::FREAD, open_files)
    }

    pub fn write(
        &mut self,
        fd: usize,
        count: usize,
        open_files: &OpenFiles,
    ) -> Result<usize, PosixError> {
        self.rdwr(fd, count, FileFlags::FWRITE, open_files)
    }

    pub fn rdwr(
        &mut self,
        fd: usize,
        count: usize,
        mode: FileFlags,
        open_files: &OpenFiles,
    ) -> Result<usize, PosixError> {
        let file = open_files.get_f(fd)?;

        if file.lock().f_flag.contains(FileFlags::FPIPE) {
            return Err(PosixError::ENOSYS);
        }

        let inode = {
            let file_ref = file.lock();

            if !file_ref.f_flag.contains(mode) {
                return Err(PosixError::EBADF);
            }

            file_ref
                .f_inode
                .as_ref()
                .cloned()
                .ok_or(PosixError::EBADF)?
        };

        let start_offset = file.lock().f_offset;
        let advanced = {
            let mut inode_ref = Inode::lock_file(&inode);

            let result = if mode == FileFlags::FREAD {
                inode_ref
                    .read_i(count, start_offset as usize)
                    .map(|_| Self::compute_advanced_bytes(&inode_ref, count, start_offset, mode))
            } else {
                inode_ref
                    .write_i(count, start_offset as usize)
                    .map(|_| Self::compute_advanced_bytes(&inode_ref, count, start_offset, mode))
            };

            inode_ref.nf_rele();
            result.map_err(|_| PosixError::EIO)?
        };

        file.lock().f_offset = start_offset + advanced as i32;
        Ok(advanced)
    }

    pub fn pipe(&mut self, open_files: &mut OpenFiles) -> Result<(usize, usize), PosixError> {
        let inode = fs::global_file_system()
            .i_alloc(DevId(0))
            .map_err(|_| PosixError::ENOSPC)?;

        let (read_fd, read_file) = match fs::global_open_file_table().f_alloc(open_files) {
            Ok(v) => v,
            Err(err) => {
                fs::global_inode_table().i_put(inode);
                return Err(err);
            }
        };

        let (write_fd, write_file) = match fs::global_open_file_table().f_alloc(open_files) {
            Ok(v) => v,
            Err(err) => {
                open_files.clear_f(read_fd);
                read_file.lock().f_count = 0;
                fs::global_inode_table().i_put(inode);
                return Err(err);
            }
        };

        {
            let mut read_file_ref = read_file.lock();
            read_file_ref.f_flag = FileFlags::FREAD | FileFlags::FPIPE;
            read_file_ref.f_inode = Some(inoderef_leak(inode.clone()));
        }

        {
            let mut write_file_ref = write_file.lock();
            write_file_ref.f_flag = FileFlags::FWRITE | FileFlags::FPIPE;
            write_file_ref.f_inode = Some(inoderef_leak(inode.clone()));
        }

        {
            let mut inode_ref = inode.lock();
            inode_ref.i_count = 2;
            inode_ref.i_flag = InodeFlag::IACC | InodeFlag::IUPD;
            inode_ref.i_mode = InodeMode::IALLOC;
        }

        Ok((read_fd, write_fd))
    }

    pub fn read_p(&mut self, file: FileRef, count: usize) -> Result<usize, PosixError> {
        let inode = file
            .lock()
            .f_inode
            .as_ref()
            .cloned()
            .ok_or(PosixError::EBADF)?;

        let mut inode_ref = Inode::lock_pipe(&inode);

        let mut file_ref = file.lock();
        if file_ref.f_offset == inode_ref.i_size as i32 {
            if file_ref.f_offset != 0 {
                file_ref.f_offset = 0;
                inode_ref.i_size = 0;
                inode_ref.i_mode.remove(InodeMode::IWRITE);
            }
            inode_ref.prele();

            if inode_ref.i_count < 2 {
                return Ok(0);
            }
            return Ok(0);
        }

        let start = file_ref.f_offset as usize;
        let advanced = inode_ref.i_size.saturating_sub(start as _).min(count as _) as usize;
        inode_ref
            .read_i(count, start)
            .map_err(|_| PosixError::EIO)?;
        file_ref.f_offset += advanced as i32;
        inode_ref.prele();
        Ok(advanced)
    }

    pub fn write_p(&mut self, file: FileRef, count: usize) -> Result<usize, PosixError> {
        let inode = file
            .lock()
            .f_inode
            .as_ref()
            .cloned()
            .ok_or(PosixError::EBADF)?;

        let mut inode_ref = Inode::lock_pipe(&inode);

        if inode_ref.i_count < 2 {
            inode_ref.prele();
            return Err(PosixError::EPIPE);
        }

        if inode_ref.i_size as usize == super::inode::Inode::PIPSIZ {
            inode_ref.i_mode.insert(InodeMode::IWRITE);
            inode_ref.prele();
            return Ok(0);
        }

        let writable = count.min(super::inode::Inode::PIPSIZ - inode_ref.i_size as usize);
        let start = inode_ref.i_size as usize;
        inode_ref
            .write_i(writable, start)
            .map_err(|_| PosixError::EIO)?;
        inode_ref.prele();
        Ok(writable)
    }

    pub fn name_i(
        &mut self,
        path: &str,
        mode: DirSearchMode,
    ) -> Result<Option<InodeRef>, PosixError> {
        self.ensure_root_registered();

        let normalized = self.normalize_path(path);
        match mode {
            DirSearchMode::Open => {
                let inode = self.lookup_counted_inode(&normalized)?;
                Ok(Some(inode))
            }
            DirSearchMode::Create => {
                if self.path_index.contains_key(&normalized) {
                    return Ok(Some(self.lookup_counted_inode(&normalized)?));
                }

                let (parent_path, entry_name) = self.split_parent_child(&normalized)?;
                let parent = self.lookup_counted_inode(&parent_path)?;
                self.pending_parent = Some(parent);
                self.pending_path = Some(normalized);
                self.pending_name = Some(Self::encode_name(&entry_name));
                Ok(None)
            }
            DirSearchMode::Delete => {
                if self.path_index.contains_key(&normalized) {
                    let (parent_path, entry_name) = self.split_parent_child(&normalized)?;
                    self.pending_parent = Some(self.lookup_counted_inode(&parent_path)?);
                    self.pending_path = Some(normalized.clone());
                    self.pending_name = Some(Self::encode_name(&entry_name));
                    return Ok(self.pending_parent.as_ref().cloned());
                }
                Err(PosixError::ENOENT)
            }
        }
    }

    pub fn next_char() -> char {
        todo!();
        '\0'
    }

    pub fn mak_node(&mut self, mode: u32) -> Result<InodeRef, PosixError> {
        let parent = self
            .pending_parent
            .as_ref()
            .cloned()
            .ok_or(PosixError::ENOENT)?;
        let dev = parent.lock().i_dev;
        let inode = fs::global_file_system()
            .i_alloc(dev)
            .map_err(|_| PosixError::ENOSPC)?;

        {
            let mut inode_ref = inode.lock();
            inode_ref.i_flag |= InodeFlag::IACC | InodeFlag::IUPD;
            inode_ref.i_mode = InodeMode::from_bits_truncate(mode) | InodeMode::IALLOC;
            inode_ref.i_nlink = 1;
            inode_ref.i_uid = 0;
            inode_ref.i_gid = 0;
        }

        self.write_dir(inode.clone());
        Ok(inode)
    }

    pub fn write_dir(&mut self, inode: InodeRef) {
        if let Some(path) = self.pending_path.take() {
            self.path_index.insert(path, inode);
        }
        if let Some(parent) = self.pending_parent.take() {
            fs::global_inode_table().i_put(parent);
        }
        self.pending_name = None;
    }

    pub fn set_cur_dir(&mut self, pathname: &str) {
        self.current_dir = self.normalize_path(pathname);
    }

    pub fn access(&mut self, _inode: InodeRef, _mode: u32) -> i32 {
        todo!();
        0
    }

    pub fn owner(&mut self) -> Option<InodeRef> {
        todo!();
        None
    }

    pub fn chmod(&mut self) {
        todo!()
    }

    pub fn chown(&mut self) {
        todo!()
    }

    pub fn chdir(&mut self) {
        todo!()
    }

    pub fn link(&mut self) {
        todo!()
    }

    pub fn unlink(&mut self) {
        todo!()
    }

    fn map_open_error(err: OpenError) -> PosixError {
        match err {
            OpenError::NoSuchDevice => PosixError::ENXIO,
        }
    }

    fn ensure_root_registered(&mut self) {
        if self.path_index.contains_key("/") {
            return;
        }

        if let Some(root) = self.root_dir_inode.as_ref() {
            self.path_index.insert("/".to_string(), root.clone());
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        if path.is_empty() {
            return self.current_dir.clone();
        }

        let joined = if path.starts_with('/') {
            path.to_string()
        } else if self.current_dir == "/" {
            format!("/{}", path)
        } else {
            format!("{}/{}", self.current_dir, path)
        };

        let mut parts: Vec<&str> = Vec::new();
        for part in joined.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                _ => parts.push(part),
            }
        }

        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        }
    }

    fn split_parent_child(&self, path: &str) -> Result<(String, String), PosixError> {
        if path == "/" {
            return Err(PosixError::EEXIST);
        }

        let (parent, name) = path.rsplit_once('/').ok_or(PosixError::ENOENT)?;
        if name.is_empty() {
            return Err(PosixError::ENOENT);
        }

        let parent = if parent.is_empty() { "/" } else { parent };
        Ok((parent.to_string(), name.to_string()))
    }

    fn encode_name(name: &str) -> [u8; DirectoryEntry::DIRSIZ] {
        let mut buf = [0u8; DirectoryEntry::DIRSIZ];
        let bytes = name.as_bytes();
        let len = bytes.len().min(DirectoryEntry::DIRSIZ);
        buf[..len].copy_from_slice(&bytes[..len]);
        buf
    }

    fn lookup_counted_inode(&self, path: &str) -> Result<InodeRef, PosixError> {
        let inode = self
            .path_index
            .get(path)
            .cloned()
            .ok_or(PosixError::ENOENT)?;
        let (dev, ino) = {
            let inode_meta = inode.lock();
            (inode_meta.i_dev, inode_meta.i_number)
        };
        fs::global_inode_table().i_get(dev, ino)
    }

    fn compute_advanced_bytes(
        inode: &super::inode::Inode,
        requested: usize,
        start_offset: i32,
        mode: FileFlags,
    ) -> usize {
        if mode == FileFlags::FWRITE {
            return requested;
        }

        let file_type = inode.i_mode & InodeMode::IFMT;
        if file_type == InodeMode::IFCHR || file_type == InodeMode::IFBLK {
            return requested;
        }

        inode
            .i_size
            .saturating_sub(start_offset.max(0) as u32)
            .min(requested as u32) as usize
    }
}

#[repr(C)]
#[derive(Clone)]
pub struct DirectoryEntry {
    pub m_ino: i32,
    pub m_name: [u8; 28],
}

#[derive(Debug, Clone)]
pub struct FileStat {
    pub dev: DevId,
    pub ino: i32,
    pub mode: InodeMode,
    pub nlink: i32,
    pub uid: i16,
    pub gid: i16,
    pub size: u32,
    pub addr: [PhysicalBlock; 10],
}

impl From<&Inode> for FileStat {
    fn from(inode: &Inode) -> Self {
        Self {
            dev: inode.i_dev,
            ino: inode.i_number,
            mode: inode.i_mode,
            nlink: inode.i_nlink,
            uid: inode.i_uid,
            gid: inode.i_gid,
            size: inode.i_size,
            addr: inode.i_addr,
        }
    }
}

pub const DIRSIZ: usize = 28;

impl DirectoryEntry {
    pub const DIRSIZ: usize = 28;

    pub const fn new() -> Self {
        Self {
            m_ino: 0,
            m_name: [0; Self::DIRSIZ],
        }
    }
}
