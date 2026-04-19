use core::{cmp::min, ffi::CStr, ptr};

use kernel_macros::define_class_compat;

use crate::{
    compat::compat_get_time,
    constants::PosixError,
    dev::{
        buffer::{Buffer, DevId, LogicalBlock, PhysicalBlock},
        buffer_manager::{PPIPE, global_buffer_manager},
        device_manager::ROOTDEV,
    },
    fs::{
        self, File, FileRef, Inode, InodeRef, InodeRefCompat, InodeRefGuard, InodeRefPutExt,
        file::FileFlags,
        file_system::FileSystem,
        inode::{InodeFlag, InodeMode, inoderef_leak},
    },
    proc::{Channel, sleep, wakeup_all},
    sync::SpinExt,
    user::{Process, Userspace},
};

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirSearchMode {
    Open = 0,
    Create = 1,
    Delete = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DirectoryEntry {
    pub m_ino: i32,
    pub m_name: [u8; 28],
}

#[repr(C)]
pub struct FileManager;

extern "C" {
    fn Userspace_is_root() -> bool;
    fn Process_psignal(proc: *mut Process, signal: i32);

    fn User_get_arg_() -> *mut [usize; 5];
    fn User_get_uid_() -> *mut u16;
    fn User_get_gid_() -> *mut u16;
    fn User_get_cdir_() -> *mut Option<InodeRefCompat>;
    fn User_get_curdir_() -> *mut [u8; 128];
    fn User_get_procp_() -> *mut *mut Process;

}

impl DirectoryEntry {
    pub const DIRSIZ: usize = 28;

    pub const fn new() -> Self {
        Self {
            m_ino: 0,
            m_name: [0; Self::DIRSIZ],
        }
    }

    pub fn name(&self) -> &[u8] {
        let end = self
            .m_name
            .iter()
            .position(|&s| s == 0)
            .expect("Invalid string");
        &self.m_name[..end]
    }
}

fn args() -> &'static mut [usize; 5] {
    unsafe { &mut *User_get_arg_() }
}

fn uid() -> i16 {
    unsafe { *User_get_uid_() as i16 }
}

fn gid() -> i16 {
    unsafe { *User_get_gid_() as i16 }
}

fn set_error(err: PosixError) {
    Userspace::get().error = Some(err);
}

fn is_root() -> bool {
    unsafe { Userspace_is_root() }
}

fn i_put(inode: InodeRefCompat) {
    fs::global_inode_table().i_put(inode.own());
}

fn i_get(dev: DevId, ino: i32) -> Result<InodeRefGuard, PosixError> {
    fs::global_inode_table().i_get(dev, ino)
}

fn access_inode(inode: &Inode, mode: InodeMode) -> bool {
    if mode == InodeMode::IWRITE {
        let Ok(spb) = fs::global_file_system().get_fs(inode.i_dev) else {
            set_error(PosixError::EIO);
            return false;
        };

        if spb.lock().is_readonly() {
            set_error(PosixError::EROFS);
            return false;
        }
    }

    if uid() == 0 {
        let exec_bits =
            InodeMode::IEXEC.bits() | (InodeMode::IEXEC.bits() >> 3) | (InodeMode::IEXEC.bits() >> 6);
        let has_exec = (inode.i_mode.bits() & exec_bits) != 0;

        if mode == InodeMode::IEXEC && !has_exec {
            set_error(PosixError::EACCES);
            return false;
        }

        return true;
    }

    let mut mode_bits = mode.bits();
    if uid() != inode.i_uid {
        mode_bits >>= 3;
        if gid() != inode.i_gid {
            mode_bits >>= 3;
        }
    }

    if (inode.i_mode.bits() & mode_bits) != 0 {
        return true;
    }

    set_error(PosixError::EACCES);
    false
}

fn read_i(inode: &mut Inode) {
    let iop = Userspace::get().ioparam;
    let buffer = unsafe { core::slice::from_raw_parts_mut(iop.m_base as *mut u8, iop.m_count) };

    match inode.read(buffer, iop.m_offset) {
        Ok(nread) => {
            let iop = &mut Userspace::get().ioparam;
            iop.m_count -= nread;
            iop.m_base += nread;
            iop.m_offset += nread;
        }
        Err(err) => set_error(err),
    }
}

fn write_i(inode: &mut Inode) {
    let iop = Userspace::get().ioparam;
    let buffer = unsafe { core::slice::from_raw_parts(iop.m_base as *const u8, iop.m_count) };

    match inode.write(buffer, iop.m_offset) {
        Ok(nwrite) => {
            let iop = &mut Userspace::get().ioparam;
            iop.m_count -= nwrite;
            iop.m_base += nwrite;
            iop.m_offset += nwrite;
        }
        Err(err) => set_error(err),
    }
}

trait InodeRefExt {
    fn has_access(&self, mode: InodeMode) -> bool;
    fn readblk(&self, lbn: LogicalBlock) -> Buffer;
    fn search(&self, name: &[u8], create: bool, remove: bool) -> Result<Option<InodeRefGuard>, PosixError>;
}

impl InodeRefExt for InodeRef {
    fn has_access(&self, mode: InodeMode) -> bool {
        access_inode(&self.lock(), mode)
    }

    fn readblk(&self, lbn: LogicalBlock) -> Buffer {
        let mut ra = None;
        self.lock().get_blk(lbn, &mut ra).unwrap()
    }

    fn search(&self, name: &[u8], create: bool, remove: bool) -> Result<Option<InodeRefGuard>, PosixError> {
        const DENTRY_SIZE: usize = size_of::<DirectoryEntry>();
        let mut count = self.lock().i_size as usize / DENTRY_SIZE;
        let mut offset = 0;
        let mut free_offset = None;

        let mut buffer = self.readblk(LogicalBlock(0));
        while count != 0 {
            let blkoff = offset % Inode::BLOCK_SIZE;
            let idx = blkoff / DENTRY_SIZE;

            if offset != 0 && blkoff == 0 {
                buffer = self.readblk(LogicalBlock((offset / Inode::BLOCK_SIZE) as u32));
            }

            let dentry = &buffer.as_slice::<DirectoryEntry>()[idx];

            if dentry.m_ino == 0 {
                free_offset.get_or_insert(offset);
            }

            if dentry.m_ino != 0 && dentry.name() == name {
                Userspace::get().dentry = *dentry;
                break;
            }

            offset += DENTRY_SIZE;
            count -= 1;
        }

        let mut ret = None;
        let mut err = None;
        loop {
            if count == 0 {
                if !create {
                    err = Some(PosixError::ENOENT);
                    break;
                }

                if !self.has_access(InodeMode::IWRITE) {
                    err = Some(PosixError::EACCES);
                    break;
                }

                Userspace::get().set_cwd_parent(self.clone());

                if free_offset.is_none() {
                    let _ = free_offset.insert(offset);
                    self.lock().i_flag.insert(InodeFlag::IUPD);
                }

                offset = free_offset.unwrap();
                break;
            }

            if remove {
                if !self.has_access(InodeMode::IWRITE) {
                    err = Some(PosixError::EACCES);
                }
                break;
            }

            let dev = self.lock().i_dev;
            let ino = Userspace::get().dentry.m_ino;

            match i_get(dev, ino) {
                Err(e) => err = Some(e),
                Ok(iref) => ret = Some(iref),
            }

            break;
        }

        Userspace::get().ioparam.m_offset = offset;
        Userspace::get().ioparam.m_count = count;

        match (err, ret) {
            (Some(err), _) => Err(err),
            (None, ret) => Ok(ret),
        }
    }
}

impl FileManager {
    fn mode(&self, mode: u32) -> DirSearchMode {
        match mode {
            1 => DirSearchMode::Create,
            2 => DirSearchMode::Delete,
            _ => DirSearchMode::Open,
        }
    }

    pub fn find(&self, mut path: &[u8], mode: DirSearchMode) -> Result<Option<InodeRef>, PosixError> {
        let mut iref = if let Some(b'/') = path.first() {
            i_get(DevId(ROOTDEV), FileSystem::ROOTINO)?
        } else {
            Userspace::get().getcwd().with_i_put()
        };

        while let Some(b'/') = path.first() {
            path = &path[1..];
        }

        while Userspace::get().error.is_none() && !path.is_empty() {
            if (iref.lock().i_mode & InodeMode::IFMT) != InodeMode::IFDIR {
                return Err(PosixError::ENOTDIR);
            }

            if !iref.has_access(InodeMode::IEXEC) {
                return Err(PosixError::EACCES);
            }

            let next_idx = path.iter().position(|&c| c == b'/').unwrap_or(path.len());
            let name = &path[..next_idx];
            path = &path[next_idx..];

            if name.len() >= DirectoryEntry::DIRSIZ {
                return Err(PosixError::EINVAL);
            }

            {
                let dbuf = Userspace::get().argdir_mut();
                dbuf[..name.len()].copy_from_slice(name);
                dbuf[name.len()..].fill(0);
            }

            while let Some(b'/') = path.first() {
                path = &path[1..];
            }

            if let Some(i) = iref.search(
                name,
                mode == DirSearchMode::Create && path.is_empty(),
                mode == DirSearchMode::Delete && path.is_empty(),
            )? {
                iref = i;
            } else {
                return Ok(None);
            }
        }

        Ok(Some(iref.into_inner()))
    }

    fn readp_inner(&self, file_ref: &FileRef) {
        loop {
            let (inode_compat, foff) = {
                let file = file_ref.lock();
                (file.f_inode.expect("pipe without inode"), file.f_offset)
            };
            let inode_ref = inode_compat.own();
            let mut inode = Inode::lock_pipe(&inode_ref);

            if foff == inode.i_size as i32 {
                if foff != 0 {
                    {
                        let mut file = file_ref.lock();
                        file.f_offset = 0;
                    }
                    inode.i_size = 0;

                    if inode.i_mode.contains(InodeMode::IWRITE) {
                        inode.i_mode.remove(InodeMode::IWRITE);
                        let chan = inode.channel_write().channel_addr();
                        wakeup_all(chan);
                    }
                }

                inode.prele();
                drop(inode);

                if inode_ref.lock().i_count < 2 {
                    return;
                }

                let mut inode = inode_ref.lock();
                inode.i_mode.insert(InodeMode::IREAD);
                let chan = inode.channel_read().channel_addr();
                drop(inode);
                sleep(chan, PPIPE);
                continue;
            }

            Userspace::get().ioparam.m_offset = foff as usize;
            read_i(&mut inode);
            let new_off = Userspace::get().ioparam.m_offset as i32;
            {
                let mut file = file_ref.lock();
                file.f_offset = new_off;
            }
            inode.prele();
            return;
        }
    }

    fn writep_inner(&self, file_ref: &FileRef) {
        let mut count = Userspace::get().ioparam.m_count as i32;

        loop {
            let inode_compat = {
                let file = file_ref.lock();
                file.f_inode.expect("pipe without inode")
            };
            let inode_ref = inode_compat.own();
            let mut inode = Inode::lock_pipe(&inode_ref);

            if count == 0 {
                inode.prele();
                Userspace::get().ioparam.m_count = 0;
                return;
            }

            if inode.i_count < 2 {
                inode.prele();
                set_error(PosixError::EPIPE);
                unsafe {
                    Process_psignal(*User_get_procp_(), 13);
                }
                return;
            }

            if inode.i_size as usize == Inode::PIPSIZ {
                inode.i_mode.insert(InodeMode::IWRITE);
                let chan = inode.channel_write().channel_addr();
                inode.prele();
                sleep(chan, PPIPE);
                continue;
            }

            Userspace::get().ioparam.m_offset = inode.i_size as usize;
            Userspace::get().ioparam.m_count = min(
                count as usize,
                Inode::PIPSIZ.saturating_sub(Userspace::get().ioparam.m_offset),
            );
            count -= Userspace::get().ioparam.m_count as i32;

            write_i(&mut inode);

            let wake_read = inode.i_mode.contains(InodeMode::IREAD);
            if wake_read {
                inode.i_mode.remove(InodeMode::IREAD);
            }
            let chan = inode.channel_read().channel_addr();
            inode.prele();

            if wake_read {
                wakeup_all(chan);
            }
        }
    }
}

define_class_compat! {impl FileManager {
    pub fn initialize(&mut self) {
        // no-op: FileManager state now lives in Rust globals/userspace fields.
    }

    pub fn open(&mut self) {
        let Some(inode) = (unsafe { FileManager_namei(this, DirSearchMode::Open as u32) }) else {
            return;
        };

        unsafe { FileManager_open1(this, inode, args()[1] as i32, 0) };
    }

    pub fn creat(&mut self) {
        let new_acc_mode = (args()[1] as u32) & (InodeMode::IRWXU | InodeMode::IRWXG | InodeMode::IRWXO).bits();

        match unsafe { FileManager_namei(this, DirSearchMode::Create as u32) } {
            None => {
                if Userspace::get().error.is_some() {
                    return;
                }

                let Some(inode) = (unsafe { FileManager_maknode(this, new_acc_mode & !InodeMode::ISVTX.bits()) }) else {
                    return;
                };

                unsafe { FileManager_open1(this, inode, FileFlags::FWRITE.bits() as i32, 2) };
            }
            Some(mut inode) => {
                unsafe { FileManager_open1(this, inode, FileFlags::FWRITE.bits() as i32, 1) };
                unsafe {
                    inode
                        .deref_compat()
                        .i_mode
                        .insert(InodeMode::from_bits_retain(new_acc_mode));
                }
            }
        }
    }

    pub fn open1(&mut self, mut pinode: InodeRefCompat, mode: i32, trf: i32) {
        if trf != 2 {
            if (mode & FileFlags::FREAD.bits() as i32) != 0
                && unsafe { FileManager_access(this, pinode, InodeMode::IREAD.bits()) }
            {
                i_put(pinode);
                return;
            }

            if (mode & FileFlags::FWRITE.bits() as i32) != 0 {
                if unsafe { FileManager_access(this, pinode, InodeMode::IWRITE.bits()) } {
                    i_put(pinode);
                    return;
                }

                let inode = unsafe { pinode.deref_compat() };
                if (inode.i_mode & InodeMode::IFMT) == InodeMode::IFDIR {
                    set_error(PosixError::EISDIR);
                    i_put(pinode);
                    return;
                }
            }
        }

        if trf == 1 {
            unsafe { pinode.deref_compat() }.release();
        }

        unsafe { pinode.deref_compat() }.prele();

        let (fd, fileref) = match fs::global_open_file_table().f_alloc(&mut Userspace::get().open_files) {
            Ok(v) => v,
            Err(err) => {
                set_error(err);
                i_put(pinode);
                return;
            }
        };
        Userspace::get().set_user_retval(fd as u32);

        {
            let mut file = fileref.lock();
            file.f_flag = FileFlags::from_bits_truncate(mode as u32)
                & (FileFlags::FREAD | FileFlags::FWRITE);
            file.f_inode = Some(pinode);
        }

        if let Err(err) = unsafe { pinode.deref_compat() }.open_i((mode as u32) & FileFlags::FWRITE.bits()) {
            set_error(err);
        }

        if Userspace::get().error.is_some() {
            Userspace::get().open_files.clear_f(fd);
            fileref.lock().f_count -= 1;
            i_put(pinode);
        }
    }

    pub fn close(&mut self) {
        let fd = args()[0];

        let file_ref = match Userspace::get().open_files.get_f(fd) {
            Ok(file) => file,
            Err(err) => {
                set_error(err.into());
                return;
            }
        };

        Userspace::get().open_files.clear_f(fd);
        fs::global_open_file_table().close_f(&file_ref);
    }

    pub fn seek(&mut self) {
        let fd = args()[0];
        let file_ref = match Userspace::get().open_files.get_f(fd) {
            Ok(file) => file,
            Err(err) => {
                set_error(err);
                return;
            }
        };

        let mut file = file_ref.lock();
        if file.f_flag.contains(FileFlags::FPIPE) {
            set_error(PosixError::ESPIPE);
            return;
        }

        let mut offset = args()[1] as i32;
        let mut whence = args()[2] as i32;
        if whence > 2 {
            offset <<= 9;
            whence -= 3;
        }

        match whence {
            0 => file.f_offset = offset,
            1 => file.f_offset += offset,
            2 => {
                let inode = file.f_inode.expect("file without inode").own();
                file.f_offset = inode.lock().i_size as i32 + offset;
            }
            _ => {}
        }
    }

    pub fn dup(&mut self) {
        let fd = args()[0];

        let file_ref = match Userspace::get().open_files.get_f(fd) {
            Ok(file) => file,
            Err(err) => {
                set_error(err);
                return;
            }
        };

        let new_fd = match Userspace::get().open_files.alloc_free_slot() {
            Ok(fd) => fd,
            Err(err) => {
                set_error(err);
                return;
            }
        };

        Userspace::get().open_files.set_f(new_fd, file_ref.clone());
        file_ref.lock().f_count += 1;
    }

    pub fn fstat(&mut self) {
        let fd = args()[0];

        let file_ref = match Userspace::get().open_files.get_f(fd) {
            Ok(file) => file,
            Err(err) => {
                set_error(err);
                return;
            }
        };

        let inode = file_ref.lock().f_inode.expect("file without inode");
        unsafe { FileManager_stat1(this, inode, args()[1]) };
    }

    pub fn stat(&mut self) {
        let Some(inode) = (unsafe { FileManager_namei(this, DirSearchMode::Open as u32) }) else {
            return;
        };

        unsafe { FileManager_stat1(this, inode, args()[1]) };
        i_put(inode);
    }

    pub fn stat1(&mut self, mut pinode: InodeRefCompat, stat_buf: usize) {
        let inode = unsafe { pinode.deref_compat() };
        inode.i_update(compat_get_time() as i32);

        let sector = FileSystem::INODE_ZONE_START_SECTOR as u32
            + inode.i_number as u32 / FileSystem::INODE_NUMBER_PER_SECTOR as u32;

        let buf = match global_buffer_manager().bread(inode.i_dev, PhysicalBlock(sector)) {
            Ok(buf) => buf,
            Err(err) => {
                set_error(err.into());
                return;
            }
        };

        const DISK_INODE_SIZE: usize = 64;
        let off = (inode.i_number as usize % FileSystem::INODE_NUMBER_PER_SECTOR) * DISK_INODE_SIZE;
        unsafe {
            ptr::copy_nonoverlapping(
                buf.as_slice::<u8>().as_ptr().add(off),
                stat_buf as *mut u8,
                DISK_INODE_SIZE,
            );
        }
    }

    pub fn read(&mut self) {
        unsafe { FileManager_rdwr(this, FileFlags::FREAD.bits()) };
    }

    pub fn write(&mut self) {
        unsafe { FileManager_rdwr(this, FileFlags::FWRITE.bits()) };
    }

    pub fn rdwr(&mut self, mode: u32) {
        let fd = args()[0];
        let count = args()[2];

        let file_ref = match Userspace::get().open_files.get_f(fd) {
            Ok(file) => file,
            Err(err) => {
                set_error(err);
                return;
            }
        };

        {
            let file = file_ref.lock();
            let mode = FileFlags::from_bits_truncate(mode);
            if !file.f_flag.contains(mode) {
                set_error(PosixError::EACCES);
                return;
            }
        }

        Userspace::get().ioparam.m_base = args()[1];
        Userspace::get().ioparam.m_count = count;

        let is_pipe = file_ref.lock().f_flag.contains(FileFlags::FPIPE);
        if is_pipe {
            if mode == FileFlags::FREAD.bits() {
                this.readp_inner(&file_ref);
            } else {
                this.writep_inner(&file_ref);
            }
        } else {
            let (inode_compat, foff) = {
                let file = file_ref.lock();
                (file.f_inode.expect("file without inode"), file.f_offset)
            };
            let inode_ref = inode_compat.own();

            let mut inode = Inode::lock_file(&inode_ref);
            Userspace::get().ioparam.m_offset = foff as usize;
            if mode == FileFlags::FREAD.bits() {
                read_i(&mut inode);
            } else {
                write_i(&mut inode);
            }
            inode.nf_rele();

            let moved = count.saturating_sub(Userspace::get().ioparam.m_count);
            let mut file = file_ref.lock();
            file.f_offset += moved as i32;
        }

        Userspace::get().set_user_retval((count.saturating_sub(Userspace::get().ioparam.m_count)) as u32);
    }

    pub fn pipe(&mut self) {
        let inode_ref = match fs::global_file_system().i_alloc(DevId(ROOTDEV)) {
            Ok(inode) => inode,
            Err(_) => {
                set_error(PosixError::ENOSPC);
                return;
            }
        };
        let inode_compat = inoderef_leak(inode_ref.clone());

        let (fd_r, file_r) = match fs::global_open_file_table().f_alloc(&mut Userspace::get().open_files) {
            Ok(v) => v,
            Err(err) => {
                set_error(err);
                i_put(inode_compat);
                return;
            }
        };

        let (fd_w, file_w) = match fs::global_open_file_table().f_alloc(&mut Userspace::get().open_files) {
            Ok(v) => v,
            Err(err) => {
                set_error(err);
                Userspace::get().open_files.clear_f(fd_r);
                file_r.lock().f_count = 0;
                i_put(inode_compat);
                return;
            }
        };

        let fdarr = args()[0] as *mut i32;
        unsafe {
            fdarr.write(fd_r as i32);
            fdarr.add(1).write(fd_w as i32);
        }

        {
            let mut fr = file_r.lock();
            fr.f_flag = FileFlags::FREAD | FileFlags::FPIPE;
            fr.f_inode = Some(inode_compat);
        }
        {
            let mut fw = file_w.lock();
            fw.f_flag = FileFlags::FWRITE | FileFlags::FPIPE;
            fw.f_inode = Some(inode_compat);
        }

        let mut inode = inode_ref.lock();
        inode.i_count = 2;
        inode.i_flag = InodeFlag::IACC | InodeFlag::IUPD;
        inode.i_mode = InodeMode::IALLOC;
    }

    pub fn readp(&mut self, _file: *mut File) {
        // Compatibility entrypoint is currently unused; rdwr() calls Rust-native implementation.
    }

    pub fn writep(&mut self, _file: *mut File) {
        // Compatibility entrypoint is currently unused; rdwr() calls Rust-native implementation.
    }

    pub fn namei(&mut self, mode: u32) -> Option<InodeRefCompat> {
        let path = Userspace::get().argdir();
        match this.find(path, this.mode(mode)) {
            Ok(Some(iref)) => Some(inoderef_leak(iref)),
            Ok(None) => None,
            Err(err) => {
                set_error(err);
                None
            }
        }
    }

    pub fn maknode(&mut self, mode: u32) -> Option<InodeRefCompat> {
        let parent = Userspace::get().cwd_parent?;
        let dev = parent.own().lock().i_dev;

        let inode = match fs::global_file_system().i_alloc(dev) {
            Ok(inode) => inode,
            Err(_) => {
                set_error(PosixError::ENOSPC);
                return None;
            }
        };

        {
            let mut inode_l = inode.lock();
            inode_l.i_flag.insert(InodeFlag::IACC | InodeFlag::IUPD);
            inode_l.i_mode = InodeMode::from_bits_retain(mode) | InodeMode::IALLOC;
            inode_l.i_nlink = 1;
            inode_l.i_uid = uid();
            inode_l.i_gid = gid();
        }

        let inode_compat = inoderef_leak(inode);
        unsafe { FileManager_writedir(this, inode_compat) };
        Some(inode_compat)
    }

    pub fn writedir(&mut self, mut pinode: InodeRefCompat) {
        let inode = unsafe { pinode.deref_compat() };
        Userspace::get().dentry.m_ino = inode.i_number;

        for i in 0..DirectoryEntry::DIRSIZ {
            Userspace::get().dentry.m_name[i] = Userspace::get().argdir_mut()[i];
        }

        Userspace::get().ioparam.m_count = DirectoryEntry::DIRSIZ + 4;
        Userspace::get().ioparam.m_base = (&raw mut Userspace::get().dentry) as usize;

        let Some(mut parent) = Userspace::get().cwd_parent else {
            return;
        };

        write_i(unsafe { parent.deref_compat() });
        i_put(parent);
    }

    pub fn setcurdir(&mut self, pathname: usize) {
        let path = unsafe { CStr::from_ptr(pathname as *const i8) }.to_bytes();
        let curdir = unsafe { &mut *User_get_curdir_() };

        if path.first().copied() != Some(b'/') {
            let mut len = curdir.iter().position(|&x| x == 0).unwrap_or(curdir.len());
            if len > 0 && curdir[len - 1] != b'/' {
                if len < curdir.len() {
                    curdir[len] = b'/';
                    len += 1;
                }
            }

            let copy_len = min(path.len(), curdir.len().saturating_sub(len + 1));
            curdir[len..len + copy_len].copy_from_slice(&path[..copy_len]);
            if len + copy_len < curdir.len() {
                curdir[len + copy_len] = 0;
            }
        } else {
            curdir.fill(0);
            let copy_len = min(path.len(), curdir.len().saturating_sub(1));
            curdir[..copy_len].copy_from_slice(&path[..copy_len]);
            curdir[copy_len] = 0;
        }
    }

    pub fn access(&mut self, mut pinode: InodeRefCompat, mode: u32) -> bool {
        let inode = unsafe { pinode.deref_compat() };
        !access_inode(inode, InodeMode::from_bits_retain(mode))
    }

    pub fn owner(&mut self) -> Option<InodeRefCompat> {
        let mut inode = unsafe { FileManager_namei(this, DirSearchMode::Open as u32) }?;

        let ino = unsafe { inode.deref_compat() };
        if uid() == ino.i_uid || is_root() {
            return Some(inode);
        }

        i_put(inode);
        None
    }

    pub fn chmod(&mut self) {
        let mode = args()[1] as u32;

        let Some(mut iref) = (unsafe { FileManager_owner(this) }) else {
            return;
        };

        let inode = unsafe { iref.deref_compat() };
        inode.i_mode &= !InodeMode::from_bits_retain(0xFFF);
        inode.i_mode |= InodeMode::from_bits_retain(mode & 0xFFF);
        inode.i_flag.insert(InodeFlag::IUPD);

        i_put(iref);
    }

    pub fn chown(&mut self) {
        if !is_root() {
            return;
        }

        let Some(mut iref) = (unsafe { FileManager_owner(this) }) else {
            return;
        };

        let inode = unsafe { iref.deref_compat() };
        inode.i_uid = args()[1] as i16;
        inode.i_gid = args()[2] as i16;
        inode.i_flag.insert(InodeFlag::IUPD);

        i_put(iref);
    }

    pub fn chdir(&mut self) {
        let Some(mut inode) = (unsafe { FileManager_namei(this, DirSearchMode::Open as u32) }) else {
            return;
        };

        {
            let inod = unsafe { inode.deref_compat() };
            if (inod.i_mode & InodeMode::IFMT) != InodeMode::IFDIR {
                set_error(PosixError::ENOTDIR);
                i_put(inode);
                return;
            }
        }

        if unsafe { FileManager_access(this, inode, InodeMode::IEXEC.bits()) } {
            i_put(inode);
            return;
        }

        let cdir = unsafe { &mut *User_get_cdir_() };
        if let Some(old) = cdir.take() {
            i_put(old);
        }
        *cdir = Some(inode);

        unsafe { inode.deref_compat() }.prele();
        unsafe { FileManager_setcurdir(this, args()[0]) };
    }

    pub fn link(&mut self) {
        let Some(mut inode) = (unsafe { FileManager_namei(this, DirSearchMode::Open as u32) }) else {
            return;
        };

        {
            let i = unsafe { inode.deref_compat() };
            if i.i_nlink >= 255 {
                set_error(PosixError::EMLINK);
                i_put(inode);
                return;
            }

            if (i.i_mode & InodeMode::IFMT) == InodeMode::IFDIR && !is_root() {
                i_put(inode);
                return;
            }

            i.i_flag.remove(InodeFlag::ILOCK);
        }

        let old_dirp = Userspace::get().dirp;
        Userspace::get().dirp = args()[1] as *mut u8;
        let new_inode = unsafe { FileManager_namei(this, DirSearchMode::Create as u32) };
        Userspace::get().dirp = old_dirp;

        if let Some(new_inode) = new_inode {
            set_error(PosixError::EEXIST);
            i_put(new_inode);
        }

        if Userspace::get().error.is_some() {
            i_put(inode);
            return;
        }

        let Some(parent) = Userspace::get().cwd_parent else {
            i_put(inode);
            return;
        };

        if parent.own().lock().i_dev != unsafe { inode.deref_compat() }.i_dev {
            i_put(parent);
            set_error(PosixError::EXDEV);
            i_put(inode);
            return;
        }

        unsafe { FileManager_writedir(this, inode) };
        {
            let i = unsafe { inode.deref_compat() };
            i.i_nlink += 1;
            i.i_flag.insert(InodeFlag::IUPD);
        }
        i_put(inode);
    }

    pub fn unlink(&mut self) {
        let Some(mut d_inode) = (unsafe { FileManager_namei(this, DirSearchMode::Delete as u32) }) else {
            return;
        };

        unsafe { d_inode.deref_compat() }.prele();

        let dev = unsafe { d_inode.deref_compat() }.i_dev;
        let ino = Userspace::get().dentry.m_ino;

        let inode = match i_get(dev, ino) {
            Ok(i) => i,
            Err(_) => {
                set_error(PosixError::EIO);
                i_put(d_inode);
                return;
            }
        };

        if (inode.lock().i_mode & InodeMode::IFMT) == InodeMode::IFDIR && !is_root() {
            i_put(d_inode);
            return;
        }

        Userspace::get().ioparam.m_offset -= DirectoryEntry::DIRSIZ + 4;
        Userspace::get().ioparam.m_base = (&raw mut Userspace::get().dentry) as usize;
        Userspace::get().ioparam.m_count = DirectoryEntry::DIRSIZ + 4;
        Userspace::get().dentry.m_ino = 0;
        write_i(unsafe { d_inode.deref_compat() });

        {
            let mut inode_l = inode.lock();
            inode_l.i_nlink -= 1;
            inode_l.i_flag.insert(InodeFlag::IUPD);
        }

        i_put(d_inode);
    }

    pub fn mknod(&mut self) {
        if !is_root() {
            set_error(PosixError::EPERM);
            return;
        }

        if let Some(inode) = unsafe { FileManager_namei(this, DirSearchMode::Create as u32) } {
            set_error(PosixError::EEXIST);
            i_put(inode);
            return;
        }

        if Userspace::get().error.is_some() {
            return;
        }

        let Some(mut inode) = (unsafe { FileManager_maknode(this, args()[1] as u32) }) else {
            return;
        };

        let inode_l = unsafe { inode.deref_compat() };
        if inode_l
            .i_mode
            .intersects(InodeMode::IFBLK | InodeMode::IFCHR)
        {
            inode_l.i_addr[0].0 = args()[2] as u32;
        }

        i_put(inode);
    }
}}
