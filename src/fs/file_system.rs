use alloc::sync::Arc;
use core::{array, ptr};
use eonix_spin::Spin;

use bitflags::bitflags;
use kernel_macros::define_class_compat;

use crate::{
    compat::compat_get_time, dev::{
        buffer::{Buf, DevId, PhysicalBlock},
        buffer_manager::global_buffer_manager,
        device_manager::ROOTDEV,
    }, fs::{
        self, InodeRef, InodeRefCompat, SuperBlockRef, inode::Inode
    }, sync::SpinExt
};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SuperBlockFlag: u32 {
        const S_FLOCK = 0x1;
        const S_ILOCK = 0x2;
        const S_FMOD  = 0x4;
        const S_RONLY = 0x8;
    }
}

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
pub struct SuperBlock {
    pub s_isize: i32,
    pub s_fsize: i32,

    pub s_nfree: i32,
    pub s_free: [i32; 100],

    pub s_ninode: i32,
    pub s_inode: [i32; 100],

    pub s_flag: SuperBlockFlag,
    pub s_time: i32,

    padding: [i32; 50],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CppSuperBlock {
    s_isize: i32,
    s_fsize: i32,
    s_nfree: i32,
    s_free: [i32; 100],
    s_ninode: i32,
    s_inode: [i32; 100],
    s_flock: i32,
    s_ilock: i32,
    s_fmod: i32,
    s_ronly: i32,
    s_time: i32,
    padding: [i32; 47],
}

#[repr(C)]
pub struct CppMount {
    m_dev: i16,
    m_spb: *mut CppSuperBlock,
    m_inodep: InodeRefCompat,
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
            s_flag: SuperBlockFlag::empty(),
            s_time: 0,
            padding: [0; 50],
        }))
    }

    pub fn is_readonly(&self) -> bool {
        self.s_flag.contains(SuperBlockFlag::S_RONLY)
    }

    pub fn is_modified(&self) -> bool {
        self.s_flag.contains(SuperBlockFlag::S_FMOD)
    }

    pub fn is_flock(&self) -> bool {
        self.s_flag.contains(SuperBlockFlag::S_FLOCK)
    }

    pub fn is_ilock(&self) -> bool {
        self.s_flag.contains(SuperBlockFlag::S_ILOCK)
    }

    fn from_cpp(spb: &CppSuperBlock, time: i32) -> Self {
        let mut flag = SuperBlockFlag::empty();
        if spb.s_flock != 0 {
            flag.insert(SuperBlockFlag::S_FLOCK);
        }
        if spb.s_ilock != 0 {
            flag.insert(SuperBlockFlag::S_ILOCK);
        }
        if spb.s_fmod != 0 {
            flag.insert(SuperBlockFlag::S_FMOD);
        }
        if spb.s_ronly != 0 {
            flag.insert(SuperBlockFlag::S_RONLY);
        }

        flag.remove(SuperBlockFlag::S_FLOCK | SuperBlockFlag::S_ILOCK | SuperBlockFlag::S_RONLY);

        Self {
            s_isize: spb.s_isize,
            s_fsize: spb.s_fsize,
            s_nfree: spb.s_nfree,
            s_free: spb.s_free,
            s_ninode: spb.s_ninode,
            s_inode: spb.s_inode,
            s_flag: flag,
            s_time: time,
            padding: [0; 50],
        }
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

    fn install_loaded_super_block(&mut self, loaded_super_block: &CppSuperBlock, time: i32) {
        self.m_mount[0].m_dev = DevId(ROOTDEV);
        self.m_mount[0].m_spb = Some(Arc::new(Spin::new(SuperBlock::from_cpp(
            loaded_super_block,
            time,
        ))));
        self.m_mount[0].m_inode = None;
    }

    fn read_super_block() -> Result<CppSuperBlock, FileSystemError> {
        let mut super_block = core::mem::MaybeUninit::<CppSuperBlock>::zeroed();
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
                sb.s_flag.remove(SuperBlockFlag::S_FMOD);
                sb.s_time = 0;
            }

            // TODO: buffer manager
            // for j in 0..2 {
            //     let buf = buffer_manager.get_blk(mount.m_dev, Self::SUPER_BLOCK_SECTOR_NUMBER + j);
            //     copy
            //     buffer_manager.bwrite(buf);
            // }
        }

        fs::global_inode_table().update_inode_table();

        self.updlock = false;

        // TODO: buffer manager
        // buffer_manager.bflush(NODEV);
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
                sb.s_flag.insert(SuperBlockFlag::S_ILOCK);

                // TODO: scan on-disk inode area and refill sb.s_inode.
                // for i in 0..sb.s_isize { ... }

                sb.s_flag.remove(SuperBlockFlag::S_ILOCK);

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
                spb.lock().s_flag.insert(SuperBlockFlag::S_FMOD);
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
        sb.s_flag.insert(SuperBlockFlag::S_FMOD);
        Ok(())
    }

    pub fn alloc(&mut self, dev: DevId) -> Result<Buf, FileSystemError> {
        let spb = self.get_fs(dev)?;

        {
            let sb = spb.lock();
            if sb.is_flock() {
                // XXX: **** WE ARE USING SPINLOCKS FOR NOW ****
                //      ******* DON'T SLEEP WITH SPINLOCKS HELD *******
                //      ******* OR YOU ** MAY **  GET DEADLOCKS *******
                // TODO: sleep
            }
        }

        let blkno = {
            let mut sb = spb.lock();
            if sb.s_nfree <= 0 {
                return Err(FileSystemError::NoSpace);
            }

            sb.s_nfree -= 1;
            sb.s_free[sb.s_nfree as usize]
        };

        if blkno == 0 {
            spb.lock().s_nfree = 0;
            return Err(FileSystemError::NoSpace);
        }

        if self.bad_block(&spb.lock(), dev, blkno) {
            return Err(FileSystemError::BadBlock);
        }

        {
            let mut sb = spb.lock();
            if sb.s_nfree <= 0 {
                sb.s_flag.insert(SuperBlockFlag::S_FLOCK);

                // TODO: read next free-block group from disk block `blkno`
                // and refill sb.s_nfree / sb.s_free.

                sb.s_flag.remove(SuperBlockFlag::S_FLOCK);
            }

            sb.s_flag.insert(SuperBlockFlag::S_FMOD);
        }

        // TODO: buffer_manager.get_blk + clr_buf.
        Err(FileSystemError::BufferUnavailable)
    }

    pub fn free(&mut self, dev: DevId, blkno: i32) -> Result<(), FileSystemError> {
        let spb = self.get_fs(dev)?;

        {
            let mut sb = spb.lock();
            sb.s_flag.insert(SuperBlockFlag::S_FMOD);
            if sb.is_flock() {
                // TODO: unlock && sleep and wakeup around s_flock.
            }
        }

        if self.bad_block(&spb.lock(), dev, blkno) {
            return Err(FileSystemError::BadBlock);
        }

        let mut sb = spb.lock();

        if sb.s_nfree <= 0 {
            sb.s_nfree = 1;
            sb.s_free[0] = 0;
        }

        if sb.s_nfree >= 100 {
            sb.s_flag.insert(SuperBlockFlag::S_FLOCK);

            // TODO: write current free-block stack into released disk block `blkno`.

            sb.s_nfree = 0;
            sb.s_flag.remove(SuperBlockFlag::S_FLOCK);
        }

        let idx = sb.s_nfree as usize;
        sb.s_free[idx] = blkno;
        sb.s_nfree += 1;
        sb.s_flag.insert(SuperBlockFlag::S_FMOD);
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
    pub fn load_super_block(mount: *mut CppMount, super_block: *mut CppSuperBlock) -> bool {
        if mount.is_null() || super_block.is_null() {
            return false;
        }

        let time = compat_get_time() as i32;
        let Ok(mut loaded_super_block) = FileSystem::read_super_block() else {
            return false;
        };

        loaded_super_block.s_flock = 0;
        loaded_super_block.s_ilock = 0;
        loaded_super_block.s_ronly = 0;
        loaded_super_block.s_time = time;

        fs::global_file_system().install_loaded_super_block(&loaded_super_block, time);

        unsafe {
            super_block.write(loaded_super_block);

            (*mount).m_dev = ROOTDEV;
            (*mount).m_spb = super_block;
        }

        true
    }
}}
