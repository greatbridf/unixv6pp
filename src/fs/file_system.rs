use alloc::sync::Arc;
use core::{array, ffi::c_void, ptr};
use eonix_spin::Spin;

use kernel_macros::define_class_compat;

use crate::{
    constants::PosixError, compat::compat_get_time, dev::{
        buffer::{Buf, DevId, PhysicalBlock},
        buffer_manager::global_buffer_manager,
        device_manager::ROOTDEV,
    }, fs::{
        self, InodeRef, InodeRefCompat, SuperBlockRef, inode::{DiskInode, Inode, inoderef_leak}
    }, proc::{PINOD, sleep, wakeup_all}, sync::SpinExt, user::Userspace
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemError {
    NoSuchFileSystem,
    LoadSuperBlockFailed,
    NoSpace,
    BadBlock,
    BufferUnavailable,
    InodeUnavailable,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SuperBlock {
    pub s_isize: i32,
    pub s_fsize: i32,

    pub s_nfree: i32,
    pub s_free: [i32; 100],

    pub s_ninode: i32,
    pub s_inode: [i32; 100],

    pub s_flock: i32,
    pub s_ilock: i32,
    pub s_fmod: i32,
    pub s_ronly: i32,
    pub s_time: i32,

    padding: [i32; 47],
}

impl SuperBlock {
    pub fn new() -> SuperBlockRef {
        Arc::new(Spin::new(Self {
            s_isize: 0,
            s_fsize: 0,
            s_nfree: 0,
            s_free: [0; 100],
            s_ninode: 0,
            s_inode: [0; 100],
            s_flock: 0,
            s_ilock: 0,
            s_fmod: 0,
            s_ronly: 0,
            s_time: 0,
            padding: [0; 47],
        }))
    }

    pub fn is_readonly(&self) -> bool {
        self.s_ronly != 0
    }

    pub fn is_modified(&self) -> bool {
        self.s_fmod != 0
    }

    pub fn is_flock(&self) -> bool {
        self.s_flock != 0
    }

    pub fn is_ilock(&self) -> bool {
        self.s_ilock != 0
    }
}

pub struct Mount {
    pub m_dev: DevId,
    pub m_spb: Option<SuperBlockRef>,
    pub m_inode: Option<InodeRef>,
}

impl Mount {
    pub fn new() -> Self {
        Self {
            m_dev: DevId(-1),
            m_spb: None,
            m_inode: None,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.m_spb.is_some() && self.m_dev.0 >= 0
    }
}

pub struct FileSystem {
    pub m_mount: [Mount; Self::NMOUNT],
    updlock: bool,
}

impl FileSystem {
    pub const NMOUNT: usize = 5;

    pub const SUPER_BLOCK_SECTOR_NUMBER: usize = 512;
    pub const ROOTINO: i32 = 1;

    pub const INODE_NUMBER_PER_SECTOR: usize = 8;
    pub const INODE_ZONE_START_SECTOR: usize = 514;
    pub const INODE_ZONE_SIZE: usize = 510;

    pub const DATA_ZONE_START_SECTOR: usize = 1024;
    pub const DATA_ZONE_SIZE: usize = 0x7400;
    pub const DATA_ZONE_END_SECTOR: usize = Self::DATA_ZONE_START_SECTOR + Self::DATA_ZONE_SIZE;

    fn install_loaded_super_block(&mut self, loaded_super_block: &SuperBlock, time: i32) {
        let mut super_block = *loaded_super_block;
        super_block.s_time = time;
        self.m_mount[0].m_dev = DevId(ROOTDEV);
        self.m_mount[0].m_spb = Some(Arc::new(Spin::new(super_block)));
        self.m_mount[0].m_inode = None;
    }

    fn read_super_block() -> Result<SuperBlock, FileSystemError> {
        let mut super_block = core::mem::MaybeUninit::<SuperBlock>::zeroed();
        let super_block_ptr = super_block.as_mut_ptr() as *mut u8;

        for i in 0..2 {
            let buf = global_buffer_manager()
                .bread(
                    DevId(ROOTDEV),
                    PhysicalBlock((Self::SUPER_BLOCK_SECTOR_NUMBER + i) as u32),
                )
                .map_err(|_| FileSystemError::LoadSuperBlockFailed)?;

            unsafe {
                ptr::copy_nonoverlapping(
                    buf.as_bytes().as_ptr(),
                    super_block_ptr.add(i * Buf::BLOCK_SIZE),
                    Buf::BLOCK_SIZE,
                );
            }
        }

        Ok(unsafe { super_block.assume_init() })
    }

    fn write_super_block(dev: DevId, super_block: &SuperBlock) -> Result<(), FileSystemError> {
        let super_block_ptr = super_block as *const SuperBlock as *const u8;

        for i in 0..2 {
            let buf = global_buffer_manager()
                .get_blk(
                    dev,
                    PhysicalBlock((Self::SUPER_BLOCK_SECTOR_NUMBER + i) as u32),
                )
                .map_err(|_| FileSystemError::BufferUnavailable)?;

            unsafe {
                ptr::copy_nonoverlapping(
                    super_block_ptr.add(i * Buf::BLOCK_SIZE),
                    (*buf).as_slice_mut().as_mut_ptr(),
                    Buf::BLOCK_SIZE,
                );
            }

            global_buffer_manager()
                .bwrite(buf)
                .map_err(|_| FileSystemError::BufferUnavailable)?;
        }

        Ok(())
    }

    fn scan_free_inodes(dev: DevId, isize: i32) -> Result<(i32, [i32; 100]), FileSystemError> {
        let mut ino = -1;
        let mut ninode = 0;
        let mut inode = [0; 100];

        for i in 0..isize {
            let buf = global_buffer_manager()
                .bread(
                    dev,
                    PhysicalBlock(Self::INODE_ZONE_START_SECTOR as u32 + i as u32),
                )
                .map_err(|_| FileSystemError::BufferUnavailable)?;

            for disk_inode in buf.as_slice::<DiskInode>().iter().take(Self::INODE_NUMBER_PER_SECTOR)
            {
                ino += 1;

                if !disk_inode.d_mode.is_empty() {
                    continue;
                }

                if fs::global_inode_table().get(dev, ino).is_some() {
                    continue;
                }

                inode[ninode as usize] = ino;
                ninode += 1;

                if ninode >= 100 {
                    break;
                }
            }

            if ninode >= 100 {
                break;
            }
        }

        Ok((ninode, inode))
    }

    fn map_error_to_posix(err: FileSystemError) -> PosixError {
        match err {
            FileSystemError::NoSpace => PosixError::ENOSPC,
            FileSystemError::InodeUnavailable => PosixError::ENFILE,
            _ => PosixError::EIO,
        }
    }

    pub fn new() -> Self {
        Self {
            m_mount: array::from_fn(|_| Mount::new()),
            updlock: false,
        }
    }

    pub fn load_super_block(&mut self) -> Result<(), FileSystemError> {
        let time = compat_get_time() as i32;
        self.install_loaded_super_block(&Self::read_super_block()?, time);

        Ok(())
    }

    pub fn get_fs(&self, dev: DevId) -> Result<SuperBlockRef, FileSystemError> {
        for mount in &self.m_mount {
            if mount.m_dev != dev {
                continue;
            }

            let Some(spb) = mount.m_spb.as_ref() else {
                continue;
            };

            let mut sb = spb.lock();
            if sb.s_nfree > 100 || sb.s_ninode > 100 {
                sb.s_nfree = 0;
                sb.s_ninode = 0;
            }

            return Ok(spb.clone());
        }

        Err(FileSystemError::NoSuchFileSystem)
    }

    pub fn update(&mut self) {
        if self.updlock {
            return;
        }

        self.updlock = true;
        let time = compat_get_time() as i32;

        for mount in &self.m_mount {
            let Some(spb) = mount.m_spb.as_ref() else {
                continue;
            };

            let should_sync = {
                let sb = spb.lock();
                sb.is_modified() && !sb.is_ilock() && !sb.is_flock() && !sb.is_readonly()
            };

            if !should_sync {
                continue;
            }

            {
                let mut sb = spb.lock();
                sb.s_fmod = 0;
                sb.s_time = time;
            }

            let sb = *spb.lock();
            let _ = Self::write_super_block(mount.m_dev, &sb);
        }

        fs::global_inode_table().update_inode_table();

        self.updlock = false;

        let _ = global_buffer_manager().bflush(None);
    }

    pub fn i_alloc(&mut self, dev: DevId) -> Result<InodeRef, FileSystemError> {
        let spb = self.get_fs(dev)?;

        {
            let sb = spb.lock();
            if sb.is_ilock() {
                // XXX: **** WE ARE USING SPINLOCKS FOR NOW ****
                //      ******* DON'T SLEEP WITH SPINLOCKS HELD *******
                //      ******* OR YOU ** MAY **  GET DEADLOCKS *******
                // TODO: sleep
            }
        }

        {
            let mut sb = spb.lock();
            if sb.s_ninode <= 0 {
                sb.s_ilock = 1;

                let isize = sb.s_isize;
                let refill_result = Self::scan_free_inodes(dev, isize);

                sb.s_ilock = 0;

                let (ninode, inode) = refill_result?;
                sb.s_ninode = ninode;
                sb.s_inode = inode;

                if sb.s_ninode <= 0 {
                    return Err(FileSystemError::NoSpace);
                }
            }
        }

        loop {
            let ino = {
                let mut sb = spb.lock();
                if sb.s_ninode <= 0 {
                    return Err(FileSystemError::NoSpace);
                }
                sb.s_ninode -= 1;
                sb.s_inode[sb.s_ninode as usize]
            };

            let inode = fs::global_inode_table()
                .i_get(dev, ino)
                .map_err(|_| FileSystemError::InodeUnavailable)?;

            let is_free = {
                let inode = inode.lock();
                inode.i_mode.is_empty()
            };

            if is_free {
                inode.lock().clean();
                spb.lock().s_fmod = 1;
                return Ok(inode);
            }

            fs::global_inode_table().i_put(inode);
        }
    }

    pub fn i_free(&mut self, dev: DevId, number: i32) -> Result<(), FileSystemError> {
        let spb = self.get_fs(dev)?;
        let mut sb = spb.lock();

        if sb.is_ilock() || sb.s_ninode >= 100 {
            return Ok(());
        }

        let idx = sb.s_ninode as usize;
        sb.s_inode[idx] = number;
        sb.s_ninode += 1;
        sb.s_fmod = 1;
        Ok(())
    }

    pub fn alloc(&mut self, dev: DevId) -> Result<*mut Buf, FileSystemError> {
        let spb = self.get_fs(dev)?;

        let mut sb = spb.lock();
        let flock_addr = (&raw const sb.s_flock) as usize;
        while sb.is_flock() {
            drop(sb);
            sleep(flock_addr, PINOD);
            sb = spb.lock();
        }

        if sb.s_nfree <= 0 {
            return Err(FileSystemError::NoSpace);
        }

        sb.s_nfree -= 1;
        let blkno = sb.s_free[sb.s_nfree as usize];
        if blkno == 0 {
            sb.s_nfree = 0;
            return Err(FileSystemError::NoSpace);
        }

        if self.bad_block(&sb, dev, blkno) {
            return Err(FileSystemError::BadBlock);
        }

        if sb.s_nfree <= 0 {
            sb.s_flock = 1;
            drop(sb);

            let refill_result = global_buffer_manager()
                .bread(dev, PhysicalBlock(blkno as u32))
                .map_err(|_| FileSystemError::BufferUnavailable)
                .map(|buf| {
                    let table = buf.as_slice::<i32>();
                    let mut sb = spb.lock();
                    sb.s_nfree = table[0];
                    sb.s_free.copy_from_slice(&table[1..101]);
                    sb.s_flock = 0;
                });

            if refill_result.is_err() {
                spb.lock().s_flock = 0;
            }

            wakeup_all(flock_addr);
            refill_result?;
            sb = spb.lock();
        }

        let buf = global_buffer_manager()
            .get_blk(dev, PhysicalBlock(blkno as u32))
            .map_err(|_| FileSystemError::BufferUnavailable)?;
        global_buffer_manager().clr_buf(buf);
        sb.s_fmod = 1;

        Ok(buf)
    }

    pub fn free(&mut self, dev: DevId, blkno: i32) -> Result<(), FileSystemError> {
        let spb = self.get_fs(dev)?;

        let mut sb = spb.lock();
        sb.s_fmod = 1;

        let flock_addr = (&raw const sb.s_flock) as usize;
        while sb.is_flock() {
            drop(sb);
            sleep(flock_addr, PINOD);
            sb = spb.lock();
        }

        if self.bad_block(&sb, dev, blkno) {
            return Err(FileSystemError::BadBlock);
        }

        if sb.s_nfree <= 0 {
            sb.s_nfree = 1;
            sb.s_free[0] = 0;
        }

        if sb.s_nfree >= 100 {
            sb.s_flock = 1;
            let nfree = sb.s_nfree;
            let free = sb.s_free;
            drop(sb);

            let write_result = global_buffer_manager()
                .get_blk(dev, PhysicalBlock(blkno as u32))
                .map_err(|_| FileSystemError::BufferUnavailable)
                .and_then(|buf| {
                    unsafe {
                        let table =
                            core::slice::from_raw_parts_mut((*buf).b_addr as *mut i32, 101);
                        table[0] = nfree;
                        table[1..101].copy_from_slice(&free);
                    }

                    global_buffer_manager()
                        .bwrite(buf)
                        .map_err(|_| FileSystemError::BufferUnavailable)
                });

            sb = spb.lock();
            sb.s_nfree = 0;
            sb.s_flock = 0;
            wakeup_all(flock_addr);
            write_result?;
        }

        let idx = sb.s_nfree as usize;
        sb.s_free[idx] = blkno;
        sb.s_nfree += 1;
        sb.s_fmod = 1;
        Ok(())
    }

    pub fn get_mount(&self, inode: &Inode) -> Option<&Mount> {
        self.m_mount.iter().find(|mount| {
            mount.m_inode.as_ref().is_some_and(|mount_inode| {
                let mount_inode = mount_inode.lock();
                mount_inode.i_dev == inode.i_dev && mount_inode.i_number == inode.i_number
            })
        })
    }

    pub fn bad_block(&self, _spb: &SuperBlock, _dev: DevId, _blkno: i32) -> bool {
        // TODO
        false
    }
}

define_class_compat! {impl FileSystem {
    pub fn load_super_block() -> bool {
        fs::global_file_system().load_super_block().is_ok()
    }

    pub fn get_fs(dev: DevId, super_block: *mut SuperBlock) -> bool {
        if super_block.is_null() {
            return false;
        }

        let Ok(spb) = fs::global_file_system().get_fs(dev) else {
            return false;
        };

        unsafe {
            super_block.write(*spb.lock());
        }

        true
    }

    pub fn is_readonly(dev: DevId) -> bool {
        let Ok(spb) = fs::global_file_system().get_fs(dev) else {
            Userspace::get().set_error(PosixError::EIO);
            return true;
        };

        let readonly = {
            let sb = spb.lock();
            sb.is_readonly()
        };
        readonly
    }

    pub fn update() {
        fs::global_file_system().update();
    }

    pub fn i_alloc(dev: DevId) -> Option<InodeRefCompat> {
        match fs::global_file_system().i_alloc(dev) {
            Ok(inode) => Some(inoderef_leak(inode)),
            Err(err) => {
                Userspace::get().set_error(FileSystem::map_error_to_posix(err));
                None
            }
        }
    }

    pub fn i_free(dev: DevId, number: i32) {
        let _ = fs::global_file_system().i_free(dev, number);
    }

    pub fn alloc(dev: DevId) -> *mut Buf {
        match fs::global_file_system().alloc(dev) {
            Ok(buf) => buf,
            Err(err) => {
                Userspace::get().set_error(FileSystem::map_error_to_posix(err));
                ptr::null_mut()
            }
        }
    }

    pub fn free(dev: DevId, blkno: i32) {
        if let Err(err) = fs::global_file_system().free(dev, blkno) {
            Userspace::get().set_error(FileSystem::map_error_to_posix(err));
        }
    }
}}
