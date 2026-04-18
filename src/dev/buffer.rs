use core::ptr::NonNull;

use alloc::slice;
use bitflags::bitflags;

use crate::dev::buffer_manager::global_buffer_manager;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevId(pub i16);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalBlock(pub u32);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PhysicalBlock(pub u32);

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct BufFlag: u32 {
        const B_WRITE  = 0x1;  // 写操作
        const B_READ   = 0x2;  // 读操作
        const B_DONE   = 0x4;  // I/O操作结束
        const B_ERROR  = 0x8;  // I/O因出错而终止
        const B_BUSY   = 0x10; // 缓存正在使用中
        const B_WANTED = 0x20; // 有进程等待该缓存，清B_BUSY时需唤醒
        const B_ASYNC  = 0x40; // 异步I/O，不需等待结束
        const B_DELWRI = 0x80; // 延迟写
    }
}

impl PhysicalBlock {
    pub const ZERO: Self = Self(0);

    pub const fn new(blkid: u32) -> Option<Self> {
        if blkid != 0 {
            Some(Self(blkid))
        } else {
            None
        }
    }
}

/// [`Buffer`] holds a reference to the underlying buffer.
///
/// Dropping the [`Buffer`] calls `bm.release(buf)`.
pub struct Buffer {
    bp: NonNull<Buf>,
}

unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

impl Buffer {
    pub const unsafe fn new(bp: *mut Buf) -> Self {
        Self {
            bp: NonNull::new_unchecked(bp),
        }
    }

    fn deref(&self) -> &Buf {
        unsafe { self.bp.as_ref() }
    }

    pub fn as_slice_mut<T: Copy>(&mut self) -> &mut [T] {
        let buffer = self.deref();
        let addr = buffer.b_addr as *mut T;
        let len_in_bytes = buffer.b_wcount as usize;

        assert!(addr.is_aligned(), "Unaligned pointer");
        assert!(len_in_bytes % size_of::<T>() == 0, "Wrong type");

        unsafe {
            core::slice::from_raw_parts_mut(
                addr as *mut T, len_in_bytes / size_of::<T>(),
            )
        }
    }

    pub fn as_slice<T: Copy>(&self) -> &[T] {
        let buffer = self.deref();
        let addr = buffer.b_addr as *mut T;
        let len_in_bytes = buffer.b_wcount as usize;

        assert!(addr.is_aligned(), "Unaligned pointer");
        assert!(len_in_bytes % size_of::<T>() == 0, "Wrong type");

        unsafe {
            core::slice::from_raw_parts(
                addr as *mut T, len_in_bytes / size_of::<T>(),
            )
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.as_slice::<u8>()
    }

    pub fn phyblk(&self) -> PhysicalBlock {
        self.deref().b_blkno
    }

    pub fn into_raw(self) -> *mut Buf {
        self.bp.as_ptr()
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        global_buffer_manager().brelse(self.bp.as_ptr());
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct Buf {
    pub b_flags: BufFlag,
    pub padding: i32,
    pub b_forw: *mut Buf,
    pub b_back: *mut Buf,
    pub av_forw: *mut Buf,
    pub av_back: *mut Buf,
    pub b_dev: i16,             // 高8位主设备号，低8位次设备号
    pub b_wcount: i32,          // 需传送的字节数
    pub b_addr: *mut u8,        // 所管理缓冲区的首地址
    pub b_blkno: PhysicalBlock, // 磁盘物理块号
    pub b_error: i32,           // I/O出错信息
    pub b_resid: i32,           // 出错时尚未传送的剩余字节数
}

impl Buf {
    pub const BLOCK_SIZE: usize = 512;

    pub const fn new() -> Self {
        Self {
            b_flags: BufFlag::empty(),
            padding: 0,
            b_forw: core::ptr::null_mut(),
            b_back: core::ptr::null_mut(),
            av_forw: core::ptr::null_mut(),
            av_back: core::ptr::null_mut(),
            b_dev: -1,
            b_wcount: 0,
            b_addr: core::ptr::null_mut(),
            b_blkno: PhysicalBlock(0),
            b_error: 0,
            b_resid: 0,
        }
    }

    pub fn read_table(&self) -> &[i32; 128] {
        unsafe { &*(self.b_addr as *const [i32; 128]) }
    }

    pub fn write_table(&mut self) -> &mut [i32; 128] {
        unsafe { &mut *(self.b_addr as *mut [i32; 128]) }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.b_addr, Self::BLOCK_SIZE) }
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.b_addr, Self::BLOCK_SIZE) }
    }

    pub fn is_busy(&self) -> bool {
        self.b_flags.contains(BufFlag::B_BUSY)
    }

    pub fn is_done(&self) -> bool {
        self.b_flags.contains(BufFlag::B_DONE)
    }

    pub fn has_error(&self) -> bool {
        self.b_flags.contains(BufFlag::B_ERROR)
    }

    pub fn is_async(&self) -> bool {
        self.b_flags.contains(BufFlag::B_ASYNC)
    }

    pub fn is_delwri(&self) -> bool {
        self.b_flags.contains(BufFlag::B_DELWRI)
    }

    pub fn is_on_device_list(&self) -> bool {
        !self.b_forw.is_null() && !self.b_back.is_null()
    }

    pub fn is_on_free_list(&self) -> bool {
        !self.av_back.is_null()
    }
}
