use crate::{
    constants::PosixError, dev::{
        block_device::block_device_for_dev, buffer::{Buf, Buffer, DevId, LogicalBlock, PhysicalBlock}, buffer_manager::{PPIPE, PRIBIO, global_buffer_manager}, char_device::{char_device_for_dev, char_device_read, char_device_write}
    }, fs::{
        self, FileRef, IOParameter, InodeRef, file::{FileFlags, FileRefCompat, InodeRefCompat}, file_system::FileSystem
    }, proc::{Channel, sleep, wakeup_all}, sync::SpinExt, user::Userspace
};
use alloc::sync::Arc;
use bitflags::bitflags;
use kernel_macros::define_class_compat;
use core::ptr::NonNull;
use eonix_spin::{NoContext, Spin, SpinGuard};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct InodeFlag: u32 {
        /// 索引节点上锁
        const ILOCK  = 0x1;
        /// 内存 Inode 被修改过，需要更新相应外存 Inode
        const IUPD   = 0x2;
        /// 内存 Inode 被访问过，需要修改最近一次访问时间
        const IACC   = 0x4;
        /// 内存 Inode 用于挂载子文件系统
        const IMOUNT = 0x8;
        /// 有进程正在等待该内存 Inode 被解锁
        const IWANT  = 0x10;
        /// 内存 Inode 对应进程图像的正文段
        const ITEXT  = 0x20;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct InodeMode: u32 {
        /// 文件被使用
        const IALLOC = 0x8000;
        /// 文件类型掩码
        const IFMT   = 0x6000;
        /// 文件类型：目录文件
        const IFDIR  = 0x4000;
        /// 字符设备特殊类型文件
        const IFCHR  = 0x2000;
        /// 块设备特殊类型文件，为0表示常规数据文件
        const IFBLK  = 0x6000;
        /// 文件长度类型：大型或巨型文件
        const ILARG  = 0x1000;
        /// 执行时将有效用户ID修改为文件所有者UID
        const ISUID  = 0x800;
        /// 执行时将有效组ID修改为文件所有者GID
        const ISGID  = 0x400;
        /// 使用后仍然位于交换区上的正文段
        const ISVTX  = 0x200;
        /// 对文件的读权限
        const IREAD  = 0x100;
        /// 对文件的写权限
        const IWRITE = 0x80;
        /// 对文件的执行权限
        const IEXEC  = 0x40;

        const IRWXU  = Self::IREAD.bits() | Self::IWRITE.bits() | Self::IEXEC.bits();
        const IRWXG  = Self::IRWXU.bits() >> 3;
        const IRWXO  = Self::IRWXU.bits() >> 6;
    }
}

const I_ADDR_SIZE: usize = 10;

#[repr(C)]
#[derive(Debug)]
pub struct Inode {
    pub i_flag: InodeFlag, // 状态标志位
    pub i_mode: InodeMode, // 文件工作方式信息

    pub i_count: i32, // 引用计数
    pub i_nlink: i32, // 文件联结计数（目录树中不同路径名的数量）

    pub i_dev: DevId,  // 外存inode所在存储设备的设备号
    pub i_number: i32, // 外存inode区中的编号

    pub i_uid: i16, // 文件所有者的用户标识数
    pub i_gid: i16, // 文件所有者的组标识数

    pub i_size: u32,                          // 文件大小（字节）
    pub i_addr: [PhysicalBlock; I_ADDR_SIZE], // 文件逻辑块号和物理块号转换的基本索引表

    pub i_lastr: i32, // 最近一次读取文件的逻辑块号（用于判断是否预读）
}

#[derive(Debug, PartialEq)]
pub enum BmapError {
    FileTooLarge, // EFBIG
    AllocFailed,
}

pub struct BmapResult {
    pub phyblk: PhysicalBlock,
    pub rablock: Option<PhysicalBlock>, // None 表示放弃预读
}

#[derive(Debug, PartialEq)]
pub enum OpenError {
    NoSuchDevice, // ENXIO
}

enum IndexKind {
    /// lbn 0–5，直接从 i_addr[lbn] 取物理块号
    Direct { slot: usize },
    /// lbn 6–261，i_addr[6/7] -> 一次间接表 -> 数据块
    Indirect {
        i_addr_slot: usize, // 6 或 7
        inner: usize,       // 在一次间接表中的下标
    },
    /// lbn 262–33031，i_addr[8/9] -> 二次间接表 -> 一次间接表 -> 数据块
    DoubleIndirect {
        i_addr_slot: usize, // 8 或 9
        mid: usize,         // 在二次间接表中的下标
        inner: usize,       // 在一次间接表中的下标
    },
}

impl IndexKind {
    fn from_lbn(lbn: usize) -> Result<Self, BmapError> {
        const N: usize = Inode::ADDRESS_PER_INDEX_BLOCK; // 128
        let small = Inode::SMALL_FILE_BLOCK; // 6
        let large = Inode::LARGE_FILE_BLOCK; // 262
        let huge = Inode::HUGE_FILE_BLOCK; // 33032

        if lbn >= huge {
            return Err(BmapError::FileTooLarge);
        }
        if lbn < small {
            Ok(IndexKind::Direct { slot: lbn })
        } else if lbn < large {
            Ok(IndexKind::Indirect {
                i_addr_slot: (lbn - small) / N + 6,
                inner: (lbn - small) % N,
            })
        } else {
            Ok(IndexKind::DoubleIndirect {
                i_addr_slot: (lbn - large) / (N * N) + 8,
                mid: ((lbn - large) / N) % N,
                inner: (lbn - large) % N,
            })
        }
    }
}

fn map_fs_alloc_error<T>(_err: T) -> BmapError {
    BmapError::AllocFailed
}

pub fn inoderef_leak(inode_ref: InodeRef) -> InodeRefCompat {
    let inoderef_compat = unsafe {
        // SAFETY: Leaking the Inode is always safe.
        //         Just make sure we don't leak too much...
        InodeRefCompat::new(&inode_ref.lock())
    };

    core::mem::forget(inode_ref);
    inoderef_compat
}

pub fn fileref_leak(file_ref: FileRef) -> FileRefCompat {
    let fileref_compat = unsafe {
        // SAFETY: Leaking the Inode is always safe.
        //         Just make sure we don't leak too much...
        FileRefCompat::new(&file_ref.lock())
    };

    core::mem::forget(file_ref);
    fileref_compat
}

#[allow(unused)]
impl Inode {
    pub const BLOCK_SIZE: usize = 512;
    pub const ADDRESS_PER_INDEX_BLOCK: usize = Self::BLOCK_SIZE / size_of::<i32>();
    pub const SMALL_FILE_BLOCK: usize = 6;
    pub const LARGE_FILE_BLOCK: usize = 128 * 2 + 6;
    pub const HUGE_FILE_BLOCK: usize = 128 * 128 * 2 + 128 * 2 + 6;
    pub const PIPSIZ: usize = Self::SMALL_FILE_BLOCK * Self::BLOCK_SIZE;

    const fn new_const() -> Self {
        Self {
            i_flag: InodeFlag::empty(),
            i_mode: InodeMode::empty(),
            i_count: 0,
            i_nlink: 0,
            i_dev: DevId(-1),
            i_number: -1,
            i_uid: -1,
            i_gid: -1,
            i_size: 0,
            i_addr: [PhysicalBlock(0); I_ADDR_SIZE],
            i_lastr: -1,
        }
    }

    pub fn new() -> InodeRef {
        Arc::new(Spin::new(Self::new_const()))
    }

    pub fn read_i(
        &mut self, m_count: usize, m_offset: usize
    ) -> Result<(), PosixError> {
        todo!()
    }

    pub fn read(
        &mut self, mut buffer: &mut [u8], mut offset: usize
    ) -> Result<usize, PosixError> {
        if buffer.len() == 0 {
            return Ok(0);
        }

        self.i_flag |= InodeFlag::IACC;

        // char dev
        if (self.i_mode & InodeMode::IFMT) == InodeMode::IFCHR {
            let devid = self.i_addr[0].0 as i16;
            char_device_read(devid);
            return Ok(0);
        }

        let is_blk = (self.i_mode & InodeMode::IFMT) == InodeMode::IFBLK;

        let mut nread = 0;
        while Userspace::get().error.is_none() && buffer.len() != 0 {
            let lbn = offset / Self::BLOCK_SIZE;
            let inner_offset = offset % Self::BLOCK_SIZE;
            let mut nbytes = (Self::BLOCK_SIZE - inner_offset).min(buffer.len());

            let bn;
            let mut dev = self.i_dev;
            let mut rablock = None;

            if !is_blk {
                let remain = self.i_size.saturating_sub(offset as _);
                if remain == 0 {
                    return Ok(nread);
                }
                nbytes = nbytes.min(remain as usize);

                let Some(blk) = self.get_blk(
                    LogicalBlock(lbn as u32), &mut rablock) else {
                    return Ok(nread);
                };

                bn = blk.phyblk();
            } else {
                bn = PhysicalBlock(lbn as u32);
                dev = DevId(self.i_addr[0].0 as i16);
                rablock = Some(PhysicalBlock(lbn as u32 + 1));
            };

            let buf = if self.i_lastr + 1 == lbn as i32 {
                global_buffer_manager().breada(dev, bn, rablock)?
            } else {
                global_buffer_manager().bread(dev, bn)?
            };

            self.i_lastr = lbn as i32;

            let data = &buf.as_bytes()[inner_offset..];
            buffer[..nbytes].copy_from_slice(&data[..nbytes]);

            buffer = &mut buffer[nbytes..];
            offset += nbytes;
            nread += nbytes;
        }

        Ok(nread)
    }

    pub fn write_i(&mut self, m_count: usize, m_offset: usize) -> Result<(), PosixError> {
        todo!()
    }

    pub fn write(
        &mut self, mut buffer: &[u8], mut offset: usize
    ) -> Result<usize, PosixError> {
        self.i_flag |= InodeFlag::IACC | InodeFlag::IUPD;

        let is_blk = (self.i_mode & InodeMode::IFMT) == InodeMode::IFBLK;
        let is_chr = (self.i_mode & InodeMode::IFMT) == InodeMode::IFCHR;

        if is_chr {
            let devid = self.i_addr[0].0 as i16;
            char_device_write(devid);
            return Ok(0);
        }

        if buffer.is_empty() {
            return Ok(0);
        }

        let mut nwrite = 0;
        while Userspace::get().error.is_none() && buffer.len() != 0 {
            let lbn = offset / Self::BLOCK_SIZE;
            let inner_offset = offset % Self::BLOCK_SIZE;
            let mut nbytes = (Self::BLOCK_SIZE - inner_offset).min(buffer.len());

            let mut ra = None;
            let (bn, dev);
            if !is_blk {
                let Some(blk) = self.get_blk(LogicalBlock(lbn as u32), &mut ra) else {
                    return Ok(nwrite);
                };

                bn = blk.phyblk();
                dev = self.i_dev;
            } else {
                bn = PhysicalBlock(lbn as u32);
                dev = DevId(self.i_addr[0].0 as i16);
            }

            let mut blk;
            if nbytes == Self::BLOCK_SIZE {
                let bp = global_buffer_manager().get_blk(dev, bn)?;
                blk = unsafe { Buffer::new(bp) };
            } else {
                blk = global_buffer_manager().bread(dev, bn)?;
            }

            let data = &mut blk.as_bytes_mut()[inner_offset..];
            data[..nbytes].copy_from_slice(&buffer[..nbytes]);

            buffer = &buffer[nbytes..];
            offset += nbytes;
            nwrite += nbytes;

            if offset % Self::BLOCK_SIZE == 0 {
                global_buffer_manager().bawrite(blk.into_raw());
            } else {
                global_buffer_manager().bdwrite(blk.into_raw());
            }

            if !is_blk && !is_chr && self.i_size < offset as _ {
                self.i_size = offset as _;
            }

            self.i_flag |= InodeFlag::IUPD;
        }

        Ok(nwrite)
    }

    /// Get the physical block ID of direct blocks.
    fn get_direct(&mut self, slot: u32) -> Option<PhysicalBlock> {
        match self.i_addr[slot as usize] {
            PhysicalBlock::ZERO => None,
            phyblk => Some(phyblk),
        }
    }

    fn alloc_blk(&mut self) -> Option<(PhysicalBlock, Buffer)> {
        extern "C" {
            fn compat_filesys_alloc(dev: DevId) -> Option<NonNull<Buf>>;
        }

        Some(unsafe {
            let buf = Buffer::new(compat_filesys_alloc(self.i_dev)?.as_ptr());
            assert_ne!(buf.phyblk(), PhysicalBlock::ZERO);
            (buf.phyblk(), buf)
        })
    }

    /// Get the buffer corresponding to some direct block.
    /// Alloc a new one and store it to i_addr if possible if not exists.
    /// Return `None`s only if allocation fails.
    fn get_direct_blk(&mut self, slot: u32) -> Option<Buffer> {
        if let Some(phyblk) = self.get_direct(slot) {
            global_buffer_manager().bread(self.i_dev, phyblk).ok()
        } else {
            let (phyblk, buf) = self.alloc_blk()?;
            self.i_flag.insert(InodeFlag::IUPD);
            self.i_addr[slot as usize] = phyblk;
            Some(buf)
        }
    }

    fn get_indirect_blk(&mut self, slot: &mut u32) -> Option<Buffer> {
        if let Some(phyblk) = PhysicalBlock::new(*slot) {
            global_buffer_manager().bread(self.i_dev, phyblk).ok()
        } else {
            let (phyblk, buf) = self.alloc_blk()?;
            self.i_flag.insert(InodeFlag::IUPD);
            *slot = phyblk.0;
            Some(buf)
        }
    }

    fn get_blk(
        &mut self, LogicalBlock(lbn): LogicalBlock, ra: &mut Option<PhysicalBlock>,
    ) -> Option<Buffer> {
        const LBN_SMALL: u32 = 6;
        const LBN_SMALL_OFF: u32 = 6;
        const LBN_LARGE: u32 = 262;
        const LBN_LARGE_OFF: u32 = 8;
        const INDIR_SLOTS: u32 = 128;
        const INDIR2_SLOTS: u32 = INDIR_SLOTS * INDIR_SLOTS;

        match lbn {
            0..6 => {
                let blk = self.get_direct_blk(lbn)?;

                if lbn != 5 {
                    *ra = PhysicalBlock::new(self.i_addr[lbn as usize + 1].0);
                }

                Some(blk)
            }
            6..262 => {
                let indir_slot = LBN_SMALL_OFF + (lbn - LBN_SMALL) / INDIR_SLOTS;
                let mut indir_blk = self.get_direct_blk(indir_slot)?;
                let mut indir_table = indir_blk.as_slice_mut::<u32>();
                let indir_idx = (lbn - LBN_SMALL) % INDIR_SLOTS;
                let blk = self.get_indirect_blk(&mut indir_table[indir_idx as usize])?;

                if indir_idx + 1 != INDIR_SLOTS {
                    *ra = PhysicalBlock::new(indir_table[indir_idx as usize + 1]);
                }

                Some(blk)
            }
            262..33030 => {
                let indir_slot = LBN_LARGE_OFF + (lbn - LBN_LARGE) / INDIR2_SLOTS;
                let mut indir_blk = self.get_direct_blk(indir_slot)?;
                let indir_table = indir_blk.as_slice_mut::<u32>();
                let indir_idx = ((lbn - LBN_LARGE) / INDIR_SLOTS) % INDIR_SLOTS;

                let mut indir2_blk = self.get_indirect_blk(&mut indir_table[indir_idx as usize])?;
                let indir2_table = indir2_blk.as_slice_mut::<u32>();
                let indir2_idx = (lbn - LBN_LARGE) % INDIR_SLOTS;
                let blk = self.get_indirect_blk(&mut indir2_table[indir2_idx as usize])?;

                if indir2_idx + 1 != INDIR_SLOTS {
                    *ra = PhysicalBlock::new(indir2_table[indir2_idx as usize + 1]);
                }

                Some(blk)
            }
            _ => unimplemented!("huge files"),
        }
    }

    fn alloc_block_from_fs(dev: DevId) -> Result<PhysicalBlock, BmapError> {
        let buf = fs::global_file_system()
            .alloc(dev)
            .map_err(map_fs_alloc_error)?;
        Ok(buf.b_blkno)
    }

    pub fn open_i(&self, mode: u32) -> Result<(), PosixError> {
        let dev = self.i_addr[0].0 as i16;

        match self.i_mode & InodeMode::IFMT {
            InodeMode::IFCHR => {
                let device = char_device_for_dev(dev).ok_or(PosixError::ENXIO)?;
                device.open(dev, mode as i32).map_err(|_| PosixError::ENXIO)?;
            }
            InodeMode::IFBLK => {
                let device = block_device_for_dev(dev).ok_or(PosixError::ENXIO)?;
                if device.open(dev, mode as i32) < 0 {
                    return Err(PosixError::ENXIO);
                }
            }
            _ => {}
        }

        Ok(())
    }

    pub fn close_i(&self, mode: FileFlags) {
        if self.i_count > 1 {
            return;
        }

        let dev = self.i_addr[0].0 as i16;

        match self.i_mode & InodeMode::IFMT {
            InodeMode::IFCHR => {
                if let Some(device) = char_device_for_dev(dev) {
                    let _ = device.close(dev, mode.bits() as i32);
                }
            }
            InodeMode::IFBLK => {
                if let Some(device) = block_device_for_dev(dev) {
                    let _ = device.close(dev, mode.bits() as i32);
                }
            }
            _ => {}
        }
    }

    pub fn i_update(&self, time: i32) {
        if !self.i_flag.intersects(InodeFlag::IUPD | InodeFlag::IACC) {
            return;
        }

        extern "C" {
            fn compat_fs_readonly(dev: DevId) -> bool;
        }

        if (unsafe { compat_fs_readonly(self.i_dev) }) {
            return;
        }

        // if fs::global_file_system()
        //     .get_fs(self.i_dev)
        //     .is_ok_and(|spb| spb.lock().is_readonly())
        // {
        //     return;
        // }

        let sector = PhysicalBlock(FileSystem::INODE_ZONE_START_SECTOR as u32
            + self.i_number as u32 / FileSystem::INODE_NUMBER_PER_SECTOR as u32);
        let mut buf = global_buffer_manager().bread(self.i_dev, sector).unwrap();

        let disk_inode = DiskInode {
            d_mode: self.i_mode,
            d_nlink: self.i_nlink,
            d_uid: self.i_uid,
            d_gid: self.i_gid,
            d_size: self.i_size,
            d_addr: self.i_addr,
            d_atime: if self.i_flag.contains(InodeFlag::IACC) {
                time
            } else {
                0
            },
            d_mtime: if self.i_flag.contains(InodeFlag::IUPD) {
                time
            } else {
                0
            },
        };

        let offset = self.i_number as usize % FileSystem::INODE_NUMBER_PER_SECTOR;
        buf.as_slice_mut()[offset] = disk_inode;

        global_buffer_manager().bwrite(buf.into_raw()).unwrap();
    }

    pub fn i_trunc(&mut self) {
        todo!()
    }

    fn release_blk(&self, blk: PhysicalBlock) {
        if blk == PhysicalBlock::ZERO {
            return;
        }

        unsafe {
            extern "C" {
                fn compat_filesys_free(dev: DevId, blk: PhysicalBlock);
            }

            compat_filesys_free(self.i_dev, blk);
        }
    }

    fn release_table(&self, table_blk: PhysicalBlock, level: usize) {
        if table_blk == PhysicalBlock::ZERO {
            return;
        }

        let buf = global_buffer_manager().bread(self.i_dev, table_blk).unwrap();
        let table = &buf.as_slice::<PhysicalBlock>()[..128];

        for blk in table.iter().cloned() {
            if blk == PhysicalBlock::ZERO {
                continue;
            }

            if level > 1 {
                self.release_table(blk, level - 1);
            }

            self.release_blk(blk);
        }

        drop(buf);
        self.release_blk(table_blk);
    }

    pub fn release(&mut self) {
        if self.i_mode.intersects(InodeMode::IFCHR | InodeMode::IFBLK) {
            return;
        }

        let addrs = core::mem::take(&mut self.i_addr);

        // Release all the blocks in a FILO way to make Superblock free
        // blocks adjacent to each other.
        for (idx, blk) in addrs.into_iter().enumerate().rev() {
            match idx {
                0..=5 => self.release_blk(blk),
                6..=7 => self.release_table(blk, 1),
                8..=9 => self.release_table(blk, 2),
                _ => unreachable!(),
            }
        }

        self.i_size = 0;
        self.i_flag |= InodeFlag::IUPD;
        self.i_mode &= !(InodeMode::ILARG | InodeMode::IRWXU | InodeMode::IRWXG | InodeMode::IRWXO);
        self.i_nlink = 1;
    }

    fn unlock_and_wake(&mut self) {
        self.i_flag.remove(InodeFlag::ILOCK);
        if self.i_flag.contains(InodeFlag::IWANT) {
            self.i_flag.remove(InodeFlag::IWANT);
            wakeup_all(&*self);
        }
    }

    fn lock_pri(me: &Spin<Self>, pri: u32) -> SpinGuard<'_, Inode, NoContext> {
        loop {
            let mut inode = me.lock();
            if !inode.i_flag.contains(InodeFlag::ILOCK) {
                inode.i_flag.insert(InodeFlag::ILOCK);
                return inode;
            }

            inode.i_flag.insert(InodeFlag::IWANT);
            let chan = (&*inode).channel_addr();

            drop(inode);
            sleep(chan, pri);
        }
    }

    pub fn lock_file(me: &Spin<Self>) -> SpinGuard<'_, Inode, NoContext> {
        Self::lock_pri(me, PRIBIO as u32)
    }

    pub fn lock_pipe(me: &Spin<Self>) -> SpinGuard<'_, Inode, NoContext> {
        Self::lock_pri(me, PPIPE)
    }

    pub fn nf_rele(&mut self) {
        self.unlock_and_wake();
    }

    pub fn prele(&mut self) {
        self.unlock_and_wake();
    }

    pub fn clean(&mut self) {
        /*
         * Inode::Clean()特定用于IAlloc()中清空新分配DiskInode的原有数据，
         * 即旧文件信息。Clean()函数中不应当清除i_dev, i_number, i_flag, i_count,
         * 这是属于内存Inode而非DiskInode包含的旧文件信息，而Inode类构造函数需要
         * 将其初始化为无效值。
         */
        self.i_mode = InodeMode::empty();
        self.i_nlink = 0;
        self.i_uid = -1;
        self.i_gid = -1;
        self.i_size = 0;
        self.i_lastr = -1;
        self.i_addr = [PhysicalBlock(0); 10];
    }

    pub fn channel_read(&self) -> impl Channel + use<'_> {
        let ptr = self as *const Self;

        unsafe {
            &*ptr.add(2)
        }
    }

    pub fn channel_write(&self) -> impl Channel + use<'_> {
        let ptr = self as *const Self;

        unsafe {
            &*ptr.add(1)
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DiskInode {
    pub d_mode: InodeMode,
    pub d_nlink: i32,
    pub d_uid: i16,
    pub d_gid: i16,
    pub d_size: u32,
    pub d_addr: [PhysicalBlock; 10],
    pub d_atime: i32,
    pub d_mtime: i32,
}

define_class_compat! {impl Inode {
    pub fn clean(&mut self) {
        this.clean();
    }

    pub fn file_lock(this: *mut Inode) {
        let this = unsafe { Spin::ref_from_inner(this) };
        Inode::lock_file(unsafe { &*this });
    }

    pub fn pipe_lock(this: *mut Inode) {
        let this = unsafe { Spin::ref_from_inner(this) };
        Inode::lock_pipe(unsafe { &*this });
    }

    pub fn release_lock(&mut self) {
        this.unlock_and_wake();
    }

    pub fn read(&mut self) {
        let IOParameter { m_base, m_count, m_offset } = Userspace::get().ioparam;

        let buffer = unsafe {
            core::slice::from_raw_parts_mut(
                m_base as *mut u8,
                m_count,
            )
        };

        match this.read(buffer, m_offset) {
            Err(err) => Userspace::get().error = Some(err),
            Ok(nread) => {
                let ioparam = &mut Userspace::get().ioparam;
                ioparam.m_count -= nread;
                ioparam.m_base += nread;
                ioparam.m_offset += nread;
            }
        }
    }

    pub fn write(&mut self) {
        let IOParameter { m_base, m_count, m_offset } = Userspace::get().ioparam;

        let buffer = unsafe {
            core::slice::from_raw_parts(
                m_base as *const u8,
                m_count,
            )
        };

        match this.write(buffer, m_offset) {
            Err(err) => Userspace::get().error = Some(err),
            Ok(nwrite) => {
                let ioparam = &mut Userspace::get().ioparam;
                ioparam.m_count -= nwrite;
                ioparam.m_base += nwrite;
                ioparam.m_offset += nwrite;
            }
        }
    }

    pub fn update(&mut self, time: i32) {
        this.i_update(time);
    }

    pub fn release(&mut self) {
        this.release();
    }

    pub fn open(&mut self, mode: u32) {
        if let Err(err) = this.open_i(mode) {
            Userspace::get().error = Some(err);
        }
    }

    pub fn close(&mut self, mode: FileFlags) {
        this.close_i(mode);
    }

    pub fn bmap(&mut self, blkid: u32) -> PhysicalBlock {
        let mut ra = None;
        let buf = this.get_blk(LogicalBlock(blkid), &mut ra);
        buf.map(|buf| buf.phyblk()).unwrap_or(PhysicalBlock::ZERO)
    }

    pub fn get_dev(&mut self) -> DevId {
        DevId(this.i_addr[0].0 as i16)
    }

    pub fn set_dev(&mut self, dev: DevId) {
        this.i_addr[0].0 = dev.0 as _;
    }
}}
