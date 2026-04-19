use alloc::sync::Arc;
use core::{array, ptr};
use eonix_spin::Spin;

use bitflags::bitflags;
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
    m_inodep: Option<InodeRefCompat>,
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

    fn to_cpp(&self) -> CppSuperBlock {
        CppSuperBlock {
            s_isize: self.s_isize,
            s_fsize: self.s_fsize,
            s_nfree: self.s_nfree,
            s_free: self.s_free,
            s_ninode: self.s_ninode,
            s_inode: self.s_inode,
            s_flock: self.s_flag.contains(SuperBlockFlag::S_FLOCK) as i32,
            s_ilock: self.s_flag.contains(SuperBlockFlag::S_ILOCK) as i32,
            s_fmod: self.s_flag.contains(SuperBlockFlag::S_FMOD) as i32,
            s_ronly: self.s_flag.contains(SuperBlockFlag::S_RONLY) as i32,
            s_time: self.s_time,
            padding: [0; 47],
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

    fn write_super_block(dev: DevId, super_block: &CppSuperBlock) -> Result<(), FileSystemError> {
        let super_block_ptr = super_block as *const CppSuperBlock as *const u8;

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

    fn get_cpp_fs(mount: *mut CppMount, dev: DevId) -> *mut CppSuperBlock {
        if mount.is_null() {
            return ptr::null_mut();
        }

        for i in 0..FileSystem::NMOUNT {
            let mount = unsafe { &mut *mount.add(i) };
            if mount.m_spb.is_null() || mount.m_dev != dev.0 {
                continue;
            }

            let spb = unsafe { &mut *mount.m_spb };
            if spb.s_nfree > 100 || spb.s_ninode > 100 {
                spb.s_nfree = 0;
                spb.s_ninode = 0;
            }

            return mount.m_spb;
        }

        ptr::null_mut()
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
                sb.s_flag.remove(SuperBlockFlag::S_FMOD);
                sb.s_time = time;
            }

            let _ = Self::write_super_block(mount.m_dev, &spb.lock().to_cpp());
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
                sb.s_flag.insert(SuperBlockFlag::S_ILOCK);

                let isize = sb.s_isize;
                let refill_result = Self::scan_free_inodes(dev, isize);

                sb.s_flag.remove(SuperBlockFlag::S_ILOCK);

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

    pub fn alloc(&mut self, dev: DevId) -> Result<*mut Buf, FileSystemError> {
        let spb = self.get_fs(dev)?;

        let mut sb = spb.lock();
        let flock_addr = (&raw const sb.s_flag) as usize;
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
            sb.s_flag.insert(SuperBlockFlag::S_FLOCK);
            drop(sb);

            let refill_result = global_buffer_manager()
                .bread(dev, PhysicalBlock(blkno as u32))
                .map_err(|_| FileSystemError::BufferUnavailable)
                .map(|buf| {
                    let table = buf.as_slice::<i32>();
                    let mut sb = spb.lock();
                    sb.s_nfree = table[0];
                    sb.s_free.copy_from_slice(&table[1..101]);
                    sb.s_flag.remove(SuperBlockFlag::S_FLOCK);
                });

            if refill_result.is_err() {
                spb.lock().s_flag.remove(SuperBlockFlag::S_FLOCK);
            }

            wakeup_all(flock_addr);
            refill_result?;
            sb = spb.lock();
        }

        let buf = global_buffer_manager()
            .get_blk(dev, PhysicalBlock(blkno as u32))
            .map_err(|_| FileSystemError::BufferUnavailable)?;
        global_buffer_manager().clr_buf(buf);
        sb.s_flag.insert(SuperBlockFlag::S_FMOD);

        Ok(buf)
    }

    pub fn free(&mut self, dev: DevId, blkno: i32) -> Result<(), FileSystemError> {
        let spb = self.get_fs(dev)?;

        let mut sb = spb.lock();
        sb.s_flag.insert(SuperBlockFlag::S_FMOD);

        let flock_addr = (&raw const sb.s_flag) as usize;
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
            sb.s_flag.insert(SuperBlockFlag::S_FLOCK);
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
            sb.s_flag.remove(SuperBlockFlag::S_FLOCK);
            wakeup_all(flock_addr);
            write_result?;
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

    pub fn get_fs(mount: *mut CppMount, dev: DevId) -> *mut CppSuperBlock {
        FileSystem::get_cpp_fs(mount, dev)
    }

    pub fn update(mount: *mut CppMount, updlock: *mut i32) {
        if mount.is_null() || updlock.is_null() {
            return;
        }

        unsafe {
            if *updlock != 0 {
                return;
            }

            *updlock += 1;
        }

        let time = compat_get_time() as i32;

        for i in 0..FileSystem::NMOUNT {
            let mount = unsafe { &mut *mount.add(i) };
            if mount.m_spb.is_null() {
                continue;
            }

            let spb = unsafe { &mut *mount.m_spb };
            if spb.s_fmod == 0 || spb.s_ilock != 0 || spb.s_flock != 0 || spb.s_ronly != 0 {
                continue;
            }

            spb.s_fmod = 0;
            spb.s_time = time;

            let _ = FileSystem::write_super_block(DevId(mount.m_dev), spb);
        }

        fs::global_inode_table().update_inode_table();

        unsafe {
            *updlock = 0;
        }

        let _ = global_buffer_manager().bflush(None);
    }

    pub fn i_alloc(mount: *mut CppMount, dev: DevId) -> Option<InodeRefCompat> {
        let spb = FileSystem::get_cpp_fs(mount, dev);
        if spb.is_null() {
            Userspace::get().set_error(PosixError::EIO);
            return None;
        }

        let spb = unsafe { &mut *spb };
        let ilock_addr = (&raw const spb.s_ilock) as usize;

        while spb.s_ilock != 0 {
            sleep(ilock_addr, PINOD);
        }

        if spb.s_ninode <= 0 {
            spb.s_ilock += 1;

            let refill_result = FileSystem::scan_free_inodes(dev, spb.s_isize);

            spb.s_ilock = 0;
            wakeup_all(ilock_addr);

            let (ninode, inode) = match refill_result {
                Ok(result) => result,
                Err(err) => {
                    Userspace::get().set_error(FileSystem::map_error_to_posix(err));
                    return None;
                }
            };

            spb.s_ninode = ninode;
            spb.s_inode = inode;

            if spb.s_ninode <= 0 {
                Userspace::get().set_error(PosixError::ENOSPC);
                return None;
            }
        }

        loop {
            if spb.s_ninode <= 0 {
                Userspace::get().set_error(PosixError::ENOSPC);
                return None;
            }

            spb.s_ninode -= 1;
            let ino = spb.s_inode[spb.s_ninode as usize];

            let inode = match fs::global_inode_table().i_get(dev, ino) {
                Ok(inode) => inode,
                Err(err) => {
                    Userspace::get().set_error(err);
                    return None;
                }
            };

            if inode.lock().i_mode.is_empty() {
                inode.lock().clean();
                spb.s_fmod = 1;
                return Some(inoderef_leak(inode));
            }

            fs::global_inode_table().i_put(inode);
        }
    }

    pub fn i_free(mount: *mut CppMount, dev: DevId, number: i32) {
        let spb = FileSystem::get_cpp_fs(mount, dev);
        if spb.is_null() {
            return;
        }

        let spb = unsafe { &mut *spb };
        if spb.s_ilock != 0 || spb.s_ninode >= 100 {
            return;
        }

        spb.s_inode[spb.s_ninode as usize] = number;
        spb.s_ninode += 1;
        spb.s_fmod = 1;
    }

    pub fn alloc(mount: *mut CppMount, dev: DevId) -> *mut Buf {
        let spb = FileSystem::get_cpp_fs(mount, dev);
        if spb.is_null() {
            Userspace::get().set_error(PosixError::EIO);
            return ptr::null_mut();
        }

        let spb = unsafe { &mut *spb };
        let flock_addr = (&raw const spb.s_flock) as usize;

        while spb.s_flock != 0 {
            sleep(flock_addr, PINOD);
        }

        if spb.s_nfree <= 0 {
            Userspace::get().set_error(PosixError::ENOSPC);
            return ptr::null_mut();
        }

        spb.s_nfree -= 1;
        let blkno = spb.s_free[spb.s_nfree as usize];

        if blkno == 0 {
            spb.s_nfree = 0;
            Userspace::get().set_error(PosixError::ENOSPC);
            return ptr::null_mut();
        }

        if spb.s_nfree <= 0 {
            spb.s_flock += 1;

            let refill_result = global_buffer_manager()
                .bread(dev, PhysicalBlock(blkno as u32))
                .map(|buf| {
                    let table = buf.as_slice::<i32>();
                    spb.s_nfree = table[0];
                    spb.s_free.copy_from_slice(&table[1..101]);
                });

            spb.s_flock = 0;
            wakeup_all(flock_addr);

            if refill_result.is_err() {
                Userspace::get().set_error(PosixError::EIO);
                return ptr::null_mut();
            }
        }

        let buf = match global_buffer_manager().get_blk(dev, PhysicalBlock(blkno as u32)) {
            Ok(buf) => buf,
            Err(_) => {
                Userspace::get().set_error(PosixError::EIO);
                return ptr::null_mut();
            }
        };

        global_buffer_manager().clr_buf(buf);
        spb.s_fmod = 1;
        buf
    }

    pub fn free(mount: *mut CppMount, dev: DevId, blkno: i32) {
        let spb = FileSystem::get_cpp_fs(mount, dev);
        if spb.is_null() {
            Userspace::get().set_error(PosixError::EIO);
            return;
        }

        let spb = unsafe { &mut *spb };
        spb.s_fmod = 1;

        let flock_addr = (&raw const spb.s_flock) as usize;
        while spb.s_flock != 0 {
            sleep(flock_addr, PINOD);
        }

        if spb.s_nfree <= 0 {
            spb.s_nfree = 1;
            spb.s_free[0] = 0;
        }

        if spb.s_nfree >= 100 {
            spb.s_flock += 1;

            let buf = match global_buffer_manager().get_blk(dev, PhysicalBlock(blkno as u32)) {
                Ok(buf) => buf,
                Err(_) => {
                    spb.s_flock = 0;
                    wakeup_all(flock_addr);
                    Userspace::get().set_error(PosixError::EIO);
                    return;
                }
            };

            unsafe {
                let table = core::slice::from_raw_parts_mut((*buf).b_addr as *mut i32, 101);
                table[0] = spb.s_nfree;
                table[1..101].copy_from_slice(&spb.s_free);
            }

            spb.s_nfree = 0;

            if global_buffer_manager().bwrite(buf).is_err() {
                Userspace::get().set_error(PosixError::EIO);
            }

            spb.s_flock = 0;
            wakeup_all(flock_addr);
        }

        spb.s_free[spb.s_nfree as usize] = blkno;
        spb.s_nfree += 1;
        spb.s_fmod = 1;
    }

    pub fn get_mount(mount: *mut CppMount, inode: Option<InodeRefCompat>) -> *mut CppMount {
        if mount.is_null() {
            return ptr::null_mut();
        }

        let Some(inode) = inode else {
            return ptr::null_mut();
        };

        let inode = {
            let inode = inode.lock();
            (&*inode) as *const Inode
        };

        for i in 0..FileSystem::NMOUNT {
            let mount = unsafe { &mut *mount.add(i) };
            let Some(mount_inode) = mount.m_inodep else {
                continue;
            };

            let mount_inode = {
                let mount_inode = mount_inode.lock();
                (&*mount_inode) as *const Inode
            };

            if mount_inode == inode {
                return mount;
            }
        }

        ptr::null_mut()
    }
}}
