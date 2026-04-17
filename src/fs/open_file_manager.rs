use alloc::boxed::Box;
use kernel_macros::define_class_compat;

use crate::{
    constants::PosixError,
    dev::buffer::DevId,
    fs::{
        self,
        file::{File, FileFlags, FileRefCompat, InodeRefCompat, OpenFiles},
        file_system::FileSystem,
        inode::{fileref_leak, inoderef_leak, Inode, InodeFlag, InodeMode},
        FileRef, InodeRef,
    },
    proc::{sleep, wakeup_all},
    sync::SpinExt,
    user::Userspace,
};

extern "C" {
    fn f_close_bottom2(file: FileRefCompat);
}

define_class_compat! {impl OpenFileTable {
    pub fn f_alloc(&mut self) -> Option<FileRefCompat> {
        let open_files = &mut Userspace::get().open_files;
        match this.f_alloc(open_files) {
            Ok((fd, fileref)) => {
                Userspace::get().set_user_retval(fd as u32);
                Some(fileref_leak(fileref))
            }
            Err(e) => {
                Userspace::get().set_error(e);
                None
            }
        }
    }

    pub fn f_close(&mut self, file: FileRefCompat) {
        this.close_f(&file.own());
    }

    pub fn alloc() -> *const OpenFileTable {
        let me = Box::new(OpenFileTable::new());

        Box::into_raw(me)
    }
}}

pub(crate) struct OpenFileTable {
    m_file: [FileRef; Self::NFILE],
}

impl OpenFileTable {
    const NFILE: usize = 100;

    pub fn new() -> Self {
        Self {
            m_file: core::array::from_fn(|_| File::new()),
        }
    }

    fn find_free(&mut self) -> Option<&mut FileRef> {
        for file in &mut self.m_file {
            if file.lock().f_count != 0 {
                continue;
            }

            return Some(file);
        }

        None
    }

    /// 在系统打开文件表中分配一个空闲 File，
    /// 同时在进程打开文件描述符表中分配对应 fd。
    /// 返回 (fd, FileRef) 或 Err。
    pub fn f_alloc(&mut self, open_files: &mut OpenFiles) -> Result<(usize, FileRef), PosixError> {
        let fd = open_files.alloc_free_slot()?;
        let free_file = self.find_free().ok_or(PosixError::ENFILE)?;

        {
            let mut file = free_file.lock();
            file.f_count = 1;
            file.f_offset = 0;
            file.f_flag = FileFlags::empty();
            file.f_inode = None;
        }
        open_files.set_f(fd, free_file.clone());
        Ok((fd, free_file.clone()))
    }

    fn close_pipe(&mut self, file: &mut File) {
        // let Some(inode) = file.f_inode else { return };

        // let mut inode = inode.lock();
        let mut inoderef = file.f_inode.expect("Pipe without inodes");
        let inode = unsafe { inoderef.deref_compat() };

        inode.i_mode &= !(InodeMode::IREAD | InodeMode::IWRITE);
        wakeup_all(inode.channel_read());
        wakeup_all(inode.channel_write());
    }

    pub fn close_f(&mut self, fileref: &FileRef) {
        let mut file = fileref.lock();

        if file.f_flag.contains(FileFlags::FPIPE) {
            self.close_pipe(&mut file);
        }

        // if file.f_count <= 1 {
        //     if let Some(inode) = file.f_inode.take() {
        //         let write_flag = if file.f_flag.contains(FileFlags::FWRITE) {
        //             1
        //         } else {
        //             0
        //         };
        //         inode.lock().close_i(write_flag);
        //         fs::global_inode_table().i_put(inode.own());
        //     }
        // }
        if file.f_count <= 1 {
            unsafe {
                f_close_bottom2(FileRefCompat::new(&file));
            }
        }

        file.f_count -= 1;
    }
}

pub(crate) struct InodeTable {
    pub m_inode: [InodeRef; InodeTable::NINODE],
}

impl InodeTable {
    pub const NINODE: usize = 100;

    pub fn new() -> Self {
        Self {
            m_inode: core::array::from_fn(|_| Inode::new()),
        }
    }

    fn i_get_not_found(&mut self, dev: DevId, ino: i32) -> Result<InodeRef, PosixError> {
        let iref = self.get_free_inode().ok_or(PosixError::ENFILE)?;
        {
            let mut inode = inode.lock();
            inode.i_dev = dev;
            inode.i_number = ino;
            inode.i_flag = InodeFlag::ILOCK;
            inode.i_count = 1;
            inode.i_lastr = -1;
        }

        let sector = FileSystem::INODE_ZONE_START_SECTOR
            + ino as usize / FileSystem::INODE_NUMBER_PER_SECTOR;

        let buffer = buffer_read(dev, sector);

        // TODO: buffer
        //let buf = kernel::buf_bread(dev, PhysicalBlock(sector as u32));

        //if buf.b_flags.contains(BufFlag::B_ERROR) {
        //    drop(buf);
        //    self.i_put(inode.clone());
        //    return Err(Error::EIO);
        //}

        //inode.lock().i_copy(&buf, inumber as usize);
        return Ok(inode);
    }

    pub fn i_get(&mut self, dev: DevId, ino: i32) -> Result<InodeRef, PosixError> {
        let mut dev = dev;

        loop {
            let Some(iref) = self.get(dev, ino) else {
                todo!()
            };
            let mut inode = iref.lock();

            if inode.i_flag.contains(InodeFlag::ILOCK) {
                inode.i_flag.insert(InodeFlag::IWANT);
                sleep(&***inode);
                continue;
            }

            if iref.i_flag.contains(InodeFlag::IMOUNT) {
                itable_comp_iget1(inoderef_leak(iref.clone()), &mut dev, &mut ino);
                continue;
                // let mount_dev = fs::global_file_system()
                //     .get_mount(&inode_ref)
                //     .map(|mount| mount.m_dev);
                // drop(inode_ref);

                // if let Some(mount_dev) = mount_dev {
                //     dev = mount_dev;
                //     ino = FileSystem::ROOTINO;
                //     continue;
                // }

                // return Err(PosixError::ENOENT);
            }

            inode.i_count += 1;
            inode.i_flag.insert(InodeFlag::ILOCK);

            break Ok(iref);
        }
    }

    /// 减少引用计数，计数归零时将 inode 写回磁盘并释放。
    pub fn i_put(&mut self, inode: InodeRef) {
        let mut inode = inode.lock();

        if inode.i_count == 1 {
            inode.i_flag |= InodeFlag::ILOCK;

            if inode.i_nlink <= 0 {
                inode.i_trunc();
                inode.i_mode = InodeMode::empty();
                let _ = fs::global_file_system().i_free(inode.i_dev, inode.i_number);
            }

            // TODO: time
            inode.i_update(0);
            inode.prele();
            inode.i_flag = InodeFlag::empty();
            inode.i_number = -1;
        }

        inode.i_count -= 1;
        inode.prele();
    }

    pub fn update_inode_table(&mut self) {
        for inode in &self.m_inode {
            let mut inode = inode.lock();
            if !inode.i_flag.contains(InodeFlag::ILOCK) && inode.i_count != 0 {
                inode.i_flag |= InodeFlag::ILOCK;
                // TODO: time
                inode.i_update(0);
                inode.prele();
            }
        }
    }

    pub fn get(&self, dev: DevId, ino: i32) -> Option<InodeRef> {
        for iref in &self.m_inode {
            let inode = iref.lock();
            if inode.i_dev != dev || inode.i_number != ino || inode.i_count <= 0 {
                continue;
            }

            return Some(iref.clone());
        }

        None
    }

    pub fn is_loaded(&self, dev: DevId, ino: i32) -> bool {
        self.get(dev, ino).is_some()
    }

    pub fn get_free_inode(&self) -> Option<InodeRef> {
        self.m_inode
            .iter()
            .find(|inode| inode.lock().i_count == 0)
            .cloned()
    }
}

define_class_compat! {impl InodeTable {
    pub fn is_loaded(&self, dev: DevId, ino: i32) -> bool {
        this.get(dev, ino).is_some()
    }
}}
