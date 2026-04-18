use crate::{
    dev::{
        block_device::block_device_for_dev,
        buffer::{Buf, DevId, LogicalBlock, PhysicalBlock},
        char_device::char_device_for_dev,
    },
    fs::{
        self, FileRef, InodeRef, file::{FileFlags, FileRefCompat, InodeRefCompat}, file_system::FileSystem
    }, proc::{Channel, wakeup_all}, sync::SpinExt
};
use alloc::sync::Arc;
use bitflags::bitflags;
use core::fmt::Error;
use eonix_spin::Spin;

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

    pub fn read_i(&mut self, m_count: usize, m_offset: usize) -> Result<(), Error> {
        // TODO: user args
        //let u = kernel::get_user();

        if m_count == 0 {
            return Ok(());
        }

        self.i_flag |= InodeFlag::IACC;

        // char dev
        if (self.i_mode & InodeMode::IFMT) == InodeMode::IFCHR {
            let dev = self.i_addr[0].0 as i16;
            // TODO: dev manager
            //kernel::get_device_manager()
            //    .get_char_device(major)
            //    .read(dev);
            return Ok(());
        }

        let is_blk = (self.i_mode & InodeMode::IFMT) == InodeMode::IFBLK;

        // TODO: user error
        while
        /*u.u_error == KernelError::NoError &&*/
        m_count != 0 {
            let lbn = m_offset / Self::BLOCK_SIZE;
            let offset = m_offset % Self::BLOCK_SIZE;
            let mut nbytes = (Self::BLOCK_SIZE - offset).min(m_count);

            let (dev, bn, rablock) = if !is_blk {
                let remain = self.i_size.saturating_sub(m_offset as _);
                if remain == 0 {
                    return Ok(());
                }
                nbytes = nbytes.min(remain as usize);

                // unsafe
                let bmap_result = self.bmap(LogicalBlock(lbn as u32)).unwrap();
                if bmap_result.phyblk == PhysicalBlock(0) {
                    return Ok(());
                }
                let rablock = bmap_result.rablock.unwrap_or(PhysicalBlock(0));
                (self.i_dev, bmap_result.phyblk, rablock)
            } else {
                let dev = self.i_addr[0].0 as i16;
                let bn = PhysicalBlock(lbn as u32);
                let ra = PhysicalBlock(lbn as u32 + 1);
                (DevId(dev), bn, ra)
            };

            // TODO: buffer manager
            let buf = if self.i_lastr + 1 == lbn as i32 {
                //kernel::buf_breada(dev, bn, rablock)
            } else {
                //kernel::buf_bread(dev, bn)
            };

            self.i_lastr = lbn as i32;

            // TODO: user args and io move
            /*let src = &buf.as_slice()[offset..offset + nbytes];
            u.io_param.copy_to_user(src);

            u.io_param.m_base += nbytes;
            u.io_param.m_offset += nbytes as u64;
            u.io_param.m_count -= nbytes as u64;*/
        }

        Ok(())
    }

    pub fn write_i(&mut self, m_count: usize, m_offset: usize) -> Result<(), Error> {
        self.i_flag |= InodeFlag::IACC | InodeFlag::IUPD;

        // char device
        if (self.i_mode & InodeMode::IFMT) == InodeMode::IFCHR {
            let dev = self.i_addr[0].0 as i16;
            // TODO: dev manager
            // kernel::get_device_manager().get_char_device(major).write(dev);
            return Ok(());
        }

        if m_count == 0 {
            return Ok(());
        }

        let is_blk = (self.i_mode & InodeMode::IFMT) == InodeMode::IFBLK;
        let mut m_count = m_count;
        let mut m_offset = m_offset;

        while m_count != 0 {
            let lbn = m_offset / Self::BLOCK_SIZE;
            let offset = m_offset % Self::BLOCK_SIZE;
            let nbytes = (Self::BLOCK_SIZE - offset).min(m_count);

            let (_dev, _bn) = if !is_blk {
                let result = self.bmap(LogicalBlock(lbn as u32)).unwrap();
                if result.phyblk == PhysicalBlock(0) {
                    return Ok(());
                }
                (self.i_dev, result.phyblk)
            } else {
                (DevId(self.i_addr[0].0 as i16), PhysicalBlock(lbn as u32))
            };

            // TODO: buffer manager
            // let mut buf = if nbytes == Self::BLOCK_SIZE {
            //     kernel::buf_get_blk(_dev, _bn)
            // } else {
            //     kernel::buf_bread(_dev, _bn)
            // };

            // TODO: io move
            // let dst = &mut buf.as_slice_mut()[offset..offset + nbytes];
            // io.copy_from_user(dst);

            m_offset += nbytes;
            m_count -= nbytes;

            // TODO: buffer manager
            // if      has_error          { drop(buf);           }
            // else if m_offset % BLOCK_SIZE == 0 { buf.into_bawrite(); }
            // else                       { buffer_manager.bdwrite(buf); }

            if !is_blk && self.i_size < m_offset as _ {
                self.i_size = m_offset as _;
            }

            self.i_flag |= InodeFlag::IUPD;
        }

        Ok(())
    }

    // map lbn to pbn
    pub fn bmap(&mut self, lbn: LogicalBlock) -> Result<BmapResult, BmapError> {
        let lbn = lbn.0 as usize;

        match IndexKind::from_lbn(lbn)? {
            // small
            IndexKind::Direct { slot } => {
                let phy = self.get_or_alloc_direct(slot)?;

                let rablock = if slot <= 4 {
                    let next = self.i_addr[slot + 1].0;
                    if next != 0 {
                        Some(PhysicalBlock(next))
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(BmapResult {
                    phyblk: phy,
                    rablock,
                })
            }

            // large
            IndexKind::Indirect {
                i_addr_slot: _,
                inner: _,
            } => {
                // TODO: wire BufferManager::bread/bdwrite before translating indirect blocks.
                Err(BmapError::AllocFailed)
            }

            // huge
            IndexKind::DoubleIndirect {
                i_addr_slot: _,
                mid: _,
                inner: _,
            } => {
                // TODO: wire BufferManager::bread/bdwrite before translating double-indirect blocks.
                Err(BmapError::AllocFailed)
            }
        }
    }

    /// 直接索引：返回物理块号，为 0 则按需分配
    fn get_or_alloc_direct(&mut self, slot: usize) -> Result<PhysicalBlock, BmapError> {
        let phy = self.i_addr[slot];
        if phy.0 != 0 {
            return Ok(PhysicalBlock(phy.0));
        }
        let blkno = Self::alloc_block_from_fs(self.i_dev)?;
        self.i_addr[slot] = blkno;
        self.i_flag |= InodeFlag::IUPD;
        Ok(blkno)
    }

    fn alloc_block_from_fs(dev: DevId) -> Result<PhysicalBlock, BmapError> {
        let buf = fs::global_file_system()
            .alloc(dev)
            .map_err(map_fs_alloc_error)?;
        Ok(buf.b_blkno)
    }

    pub fn open_i(&self, mode: u32) -> Result<(), OpenError> {
        let dev = self.i_addr[0].0 as i16;

        match self.i_mode & InodeMode::IFMT {
            InodeMode::IFCHR => {
                let device = char_device_for_dev(dev).ok_or(OpenError::NoSuchDevice)?;
                device
                    .open(dev, mode as i32)
                    .map_err(|_| OpenError::NoSuchDevice)?;
            }
            InodeMode::IFBLK => {
                let device = block_device_for_dev(dev).ok_or(OpenError::NoSuchDevice)?;
                if device.open(dev, mode as i32) < 0 {
                    return Err(OpenError::NoSuchDevice);
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

        if fs::global_file_system()
            .get_fs(self.i_dev)
            .is_ok_and(|spb| spb.lock().is_readonly())
        {
            return;
        }

        let _sector = FileSystem::INODE_ZONE_START_SECTOR
            + self.i_number as usize / FileSystem::INODE_NUMBER_PER_SECTOR;

        //let mut buf = kernel::buf_bread(self.i_dev, PhysicalBlock(sector as u32));

        let _d_inode = DiskInode {
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

        let _offset =
            (self.i_number as usize % FileSystem::INODE_NUMBER_PER_SECTOR) * size_of::<DiskInode>();
        //let dst = &mut buf.as_slice_mut()[offset..offset + size_of::<DiskInode>()];
        //let src = unsafe {
        //    slice::from_raw_parts(
        //        &d_inode as *const DiskInode as *const u8,
        //        size_of::<DiskInode>(),
        //    )
        //};
        //dst.copy_from_slice(src);

        // TODO: buffer manager
        //kernel::buf_bwrite(buf);
    }

    pub fn i_trunc(&mut self) {
        if self.i_mode.intersects(InodeMode::IFCHR | InodeMode::IFBLK) {
            return;
        }

        for i in (0..10).rev() {
            let blk = self.i_addr[i];
            if blk == PhysicalBlock(0) {
                continue;
            }

            if i >= 6 {
                // TODO: buffer manager
                //let first_buf = kernel::buf_bread(self.i_dev, blk);
                //let first_table = *first_buf.read_table();

                //for j in (0..128).rev() {
                //    if first_table[j] == 0 {
                //        continue;
                //    }
                //    if i >= 8 {
                //        let second_buf =
                //            kernel::buf_bread(self.i_dev, PhysicalBlock(first_table[j] as u32));
                //        for k in (0..128).rev() {
                //            let b = second_buf.read_table()[k];
                //            if b != 0 {
                //                let _ = fs::global_file_system().free(self.i_dev, b);
                //            }
                //        }
                //    }
                //    let _ = fs::global_file_system().free(self.i_dev, first_table[j]);
                //}
            }

            let _ = fs::global_file_system().free(self.i_dev, blk.0 as i32);
            self.i_addr[i] = PhysicalBlock(0);
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

    fn lock_with_priority(&mut self, priority: i32) {
        // TODO: user args
        //let u = kernel::get_user();
        while self.i_flag.contains(InodeFlag::ILOCK) {
            self.i_flag.insert(InodeFlag::IWANT);
            //u.u_procp.sleep(self as *mut _ as usize, priority);
        }
        self.i_flag.insert(InodeFlag::ILOCK);
    }

    pub fn nf_rele(&mut self) {
        self.unlock_and_wake();
    }

    pub fn nf_lock(&mut self) {
        self.lock_with_priority(0);
    }

    pub fn prele(&mut self) {
        self.unlock_and_wake();
    }

    pub fn plock(&mut self) {
        self.lock_with_priority(0);
    }

    pub fn clean(&mut self) {
        self.i_mode = InodeMode::empty();
        self.i_nlink = 0;
        self.i_uid = -1;
        self.i_gid = -1;
        self.i_size = 0;
        self.i_lastr = -1;
        self.i_addr = [PhysicalBlock(0); 10];
    }

    pub fn i_copy(&mut self, buf: &Buf, inumber: usize) {
        let offset = (inumber % FileSystem::INODE_NUMBER_PER_SECTOR) * size_of::<DiskInode>();

        let src = &buf.as_slice()[offset..offset + size_of::<DiskInode>()];

        // SAFETY: DiskInode is #[repr(C)] Plain Old Data
        let d_inode: DiskInode =
            unsafe { core::ptr::read_unaligned(src.as_ptr() as *const DiskInode) };

        self.i_mode = d_inode.d_mode;
        self.i_nlink = d_inode.d_nlink;
        self.i_uid = d_inode.d_uid;
        self.i_gid = d_inode.d_gid;
        self.i_size = d_inode.d_size;
        self.i_addr = d_inode.d_addr;
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
