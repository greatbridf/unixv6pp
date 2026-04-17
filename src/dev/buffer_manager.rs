use crate::sync::SuperCell;
use crate::{constants::PosixError, user::Userspace};

use super::{
    block_device::block_device_for_dev,
    buffer::{Buf, BufFlag, DevId, PhysicalBlock},
    device_manager::{set_minor, ROOTDEV},
};

const PRIBIO: i32 = -50;
const PSWP: i32 = -100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferError {
    InvalidDevice,
    BufferUnavailable,
    IoError,
    InvalidBuffer,
}

impl From<BufferError> for PosixError {
    fn from(value: BufferError) -> Self {
        match value {
            BufferError::InvalidDevice => PosixError::ENXIO,
            BufferError::BufferUnavailable => PosixError::EIO,
            BufferError::IoError => PosixError::EIO,
            BufferError::InvalidBuffer => PosixError::EINVAL,
        }
    }
}

pub type BufferResult<T> = Result<T, BufferError>;

pub struct BufferManager {
    b_free_list: Buf,
    swap_buf: Buf,
    buffers: [Buf; Self::NBUF],
    data: [[u8; Self::BUFFER_SIZE]; Self::NBUF],
}

unsafe impl Send for BufferManager {}
unsafe impl Sync for BufferManager {}

impl BufferManager {
    pub const NBUF: usize = 15;
    pub const BUFFER_SIZE: usize = Buf::BLOCK_SIZE;

    pub const fn new() -> Self {
        Self {
            b_free_list: Buf::new(),
            swap_buf: Buf::new(),
            buffers: [const { Buf::new() }; Self::NBUF],
            data: [[0; Self::BUFFER_SIZE]; Self::NBUF],
        }
    }

    pub fn initialize(&mut self) {
        self.initialize_buffers();
    }

    fn initialize_buffers(&mut self) {
        self.b_free_list = Buf::new();
        self.swap_buf = Buf::new();
        unsafe {
            self.init_free_list_sentinel();
        }

        for index in 0..Self::NBUF {
            let bp = &mut self.buffers[index] as *mut Buf;

            unsafe {
                *bp = Buf::new();
                (*bp).b_dev = -1;
                (*bp).b_addr = self.data[index].as_mut_ptr();
                Self::insert_device_front(self.free_list_sentinel(), bp);
                self.push_free_back(bp);
            }
        }
    }

    pub fn get_blk(&mut self, dev: DevId, blkno: PhysicalBlock) -> BufferResult<*mut Buf> {
        self.validate_device(dev)?;

        loop {
            if let Some(bp) = self.in_core(dev, blkno) {
                unsafe {
                    disable_interrupts();
                    if (*bp).b_flags.contains(BufFlag::B_BUSY) {
                        (*bp).b_flags.insert(BufFlag::B_WANTED);
                        sleep_on(bp as usize, PRIBIO);
                        enable_interrupts();
                        continue;
                    }
                    enable_interrupts();
                }

                self.not_avail(bp);
                return Ok(bp);
            }

            let bp = loop {
                unsafe {
                    disable_interrupts();
                    if !self.is_free_list_empty() {
                        let bp = (*self.free_list_sentinel()).av_forw;
                        enable_interrupts();
                        break bp;
                    }

                    self.b_free_list.b_flags.insert(BufFlag::B_WANTED);
                    sleep_on(self.free_list_sentinel() as usize, PRIBIO);
                    enable_interrupts();
                }
            };

            self.not_avail(bp);

            unsafe {
                if (*bp).b_flags.contains(BufFlag::B_DELWRI) {
                    (*bp).b_flags.insert(BufFlag::B_ASYNC);
                    self.bwrite(bp)?;
                    continue;
                }

                (*bp).b_flags = BufFlag::B_BUSY;
                self.remove_from_current_device_queue(bp);
                (*bp).b_dev = dev.0;
                (*bp).b_blkno = blkno;
            }

            self.insert_into_device_queue(dev, bp)?;

            return Ok(bp);
        }
    }

    pub fn brelse(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            if (*bp).b_flags.contains(BufFlag::B_WANTED) {
                wakeup_all(bp as usize);
            }

            if self.b_free_list.b_flags.contains(BufFlag::B_WANTED) {
                self.b_free_list.b_flags.remove(BufFlag::B_WANTED);
                wakeup_all(self.free_list_sentinel() as usize);
            }

            if (*bp).b_flags.contains(BufFlag::B_ERROR) {
                (*bp).b_dev = set_minor((*bp).b_dev, -1);
            }

            disable_interrupts();
            (*bp)
                .b_flags
                .remove(BufFlag::B_WANTED | BufFlag::B_BUSY | BufFlag::B_ASYNC);

            if !(*bp).is_on_free_list() {
                self.push_free_back(bp);
            }
            enable_interrupts();
        }
    }

    pub fn io_wait(&mut self, bp: *mut Buf) -> BufferResult<()> {
        if bp.is_null() {
            return Err(BufferError::InvalidBuffer);
        }

        unsafe {
            disable_interrupts();
            while !(*bp).b_flags.contains(BufFlag::B_DONE) {
                (*bp).b_flags.insert(BufFlag::B_WANTED);
                sleep_on(bp as usize, PRIBIO);
            }
            enable_interrupts();
        }

        self.get_error(bp)
    }

    pub fn io_done(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            (*bp).b_flags.insert(BufFlag::B_DONE);
            if (*bp).b_flags.contains(BufFlag::B_ASYNC) {
                self.brelse(bp);
            } else {
                (*bp).b_flags.remove(BufFlag::B_WANTED);
                wakeup_all(bp as usize);
            }
        }
    }

    pub fn bread(&mut self, dev: DevId, blkno: PhysicalBlock) -> BufferResult<*mut Buf> {
        let bp = self.get_blk(dev, blkno)?;

        unsafe {
            if (*bp).b_flags.contains(BufFlag::B_DONE) {
                return Ok(bp);
            }

            (*bp).b_flags.insert(BufFlag::B_READ);
            (*bp).b_wcount = Self::BUFFER_SIZE as i32;
        }

        let device = block_device_for_dev(dev.0).ok_or(BufferError::InvalidDevice)?;
        device.strategy(bp);
        self.io_wait(bp)?;

        Ok(bp)
    }

    pub fn breada(
        &mut self,
        dev: DevId,
        blkno: PhysicalBlock,
        read_ahead_blkno: Option<PhysicalBlock>,
    ) -> BufferResult<*mut Buf> {
        self.validate_device(dev)?;

        let mut bp = None;
        let mut should_read_ahead = read_ahead_blkno;

        if self.in_core(dev, blkno).is_none() {
            let new_bp = self.get_blk(dev, blkno)?;

            unsafe {
                if !(*new_bp).b_flags.contains(BufFlag::B_DONE) {
                    (*new_bp).b_flags.insert(BufFlag::B_READ);
                    (*new_bp).b_wcount = Self::BUFFER_SIZE as i32;

                    let device = block_device_for_dev(dev.0).ok_or(BufferError::InvalidDevice)?;
                    device.strategy(new_bp);
                }
            }

            bp = Some(new_bp);
        } else {
            should_read_ahead = None;
        }

        if let Some(read_ahead_blkno) = should_read_ahead {
            if self.in_core(dev, read_ahead_blkno).is_none() {
                let read_ahead_bp = self.get_blk(dev, read_ahead_blkno)?;

                unsafe {
                    if (*read_ahead_bp).b_flags.contains(BufFlag::B_DONE) {
                        self.brelse(read_ahead_bp);
                    } else {
                        (*read_ahead_bp)
                            .b_flags
                            .insert(BufFlag::B_READ | BufFlag::B_ASYNC);
                        (*read_ahead_bp).b_wcount = Self::BUFFER_SIZE as i32;

                        let device =
                            block_device_for_dev(dev.0).ok_or(BufferError::InvalidDevice)?;
                        device.strategy(read_ahead_bp);
                    }
                }
            }
        }

        match bp {
            Some(bp) => {
                self.io_wait(bp)?;
                Ok(bp)
            }
            None => self.bread(dev, blkno),
        }
    }

    pub fn bwrite(&mut self, bp: *mut Buf) -> BufferResult<()> {
        if bp.is_null() {
            return Err(BufferError::InvalidBuffer);
        }

        let old_flags = unsafe {
            let bp_ref = &mut *bp;
            let old_flags = bp_ref.b_flags;
            bp_ref
                .b_flags
                .remove(BufFlag::B_READ | BufFlag::B_DONE | BufFlag::B_ERROR | BufFlag::B_DELWRI);
            bp_ref.b_wcount = Self::BUFFER_SIZE as i32;
            old_flags
        };

        let dev = unsafe { (*bp).b_dev };
        let device = block_device_for_dev(dev).ok_or(BufferError::InvalidDevice)?;
        device.strategy(bp);

        if !old_flags.contains(BufFlag::B_ASYNC) {
            self.io_wait(bp)?;
            self.brelse(bp);
        } else if !old_flags.contains(BufFlag::B_DELWRI) {
            self.get_error(bp)?;
        }

        Ok(())
    }

    pub fn bdwrite(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            (*bp).b_flags.insert(BufFlag::B_DELWRI | BufFlag::B_DONE);
        }
        self.brelse(bp);
    }

    pub fn bawrite(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            (*bp).b_flags.insert(BufFlag::B_ASYNC);
        }
        let _ = self.bwrite(bp);
    }

    pub fn clr_buf(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            (*bp).as_slice_mut().fill(0);
        }
    }

    pub fn bflush(&mut self, dev: Option<DevId>) -> BufferResult<()> {
        while let Some(bp) = self.find_delayed_write_buffer(dev) {
            unsafe {
                (*bp).b_flags.insert(BufFlag::B_ASYNC);
            }
            self.not_avail(bp);
            self.bwrite(bp)?;
        }

        Ok(())
    }

    pub fn swap(
        &mut self,
        blkno: PhysicalBlock,
        addr: usize,
        count: usize,
        flag: BufFlag,
    ) -> BufferResult<()> {
        unsafe {
            disable_interrupts();
            while self.swap_buf.b_flags.contains(BufFlag::B_BUSY) {
                self.swap_buf.b_flags.insert(BufFlag::B_WANTED);
                sleep_on(self.swap_buf_ptr() as usize, PSWP);
            }
            enable_interrupts();
        }

        self.swap_buf.b_flags = BufFlag::B_BUSY | flag;
        self.swap_buf.b_dev = ROOTDEV;
        self.swap_buf.b_wcount = count as i32;
        self.swap_buf.b_blkno = blkno;
        self.swap_buf.b_addr = addr as *mut u8;

        let bp = self.swap_buf_ptr();
        let device = block_device_for_dev(ROOTDEV).ok_or(BufferError::InvalidDevice)?;
        device.strategy(bp);

        unsafe {
            disable_interrupts();
            while !self.swap_buf.b_flags.contains(BufFlag::B_DONE) {
                self.swap_buf.b_flags.insert(BufFlag::B_WANTED);
                sleep_on(self.swap_buf_ptr() as usize, PSWP);
            }
            enable_interrupts();
        }

        self.finish_swap()
    }

    fn finish_swap(&mut self) -> BufferResult<()> {
        let bp = self.swap_buf_ptr();

        if self.swap_buf.b_flags.contains(BufFlag::B_WANTED) {
            unsafe {
                wakeup_all(bp as usize);
            }
        }

        self.swap_buf
            .b_flags
            .remove(BufFlag::B_BUSY | BufFlag::B_WANTED);

        self.get_error(bp)
    }

    pub fn free_list(&self) -> &Buf {
        &self.b_free_list
    }

    pub fn free_list_mut(&mut self) -> &mut Buf {
        &mut self.b_free_list
    }

    pub fn swap_buf(&self) -> &Buf {
        &self.swap_buf
    }

    pub fn swap_buf_mut(&mut self) -> &mut Buf {
        &mut self.swap_buf
    }

    pub fn buffers(&self) -> &[Buf; Self::NBUF] {
        &self.buffers
    }

    pub fn buffers_mut(&mut self) -> &mut [Buf; Self::NBUF] {
        &mut self.buffers
    }

    pub fn data(&self) -> &[[u8; Self::BUFFER_SIZE]; Self::NBUF] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [[u8; Self::BUFFER_SIZE]; Self::NBUF] {
        &mut self.data
    }

    fn get_error(&mut self, bp: *mut Buf) -> BufferResult<()> {
        if bp.is_null() {
            return Err(BufferError::InvalidBuffer);
        }

        unsafe {
            if (*bp).b_flags.contains(BufFlag::B_ERROR) {
                Err(BufferError::IoError)
            } else {
                Ok(())
            }
        }
    }

    fn not_avail(&mut self, bp: *mut Buf) {
        if bp.is_null() {
            return;
        }

        unsafe {
            disable_interrupts();
            if (*bp).is_on_free_list() {
                self.remove_from_free_list(bp);
            }
            (*bp).b_flags.insert(BufFlag::B_BUSY);
            enable_interrupts();
        }
    }

    fn in_core(&self, dev: DevId, blkno: PhysicalBlock) -> Option<*mut Buf> {
        if dev.0 < 0 {
            return self.in_device_list(self.free_list_sentinel_const(), blkno, dev.0);
        }

        let device = block_device_for_dev(dev.0)?;
        device.devtab().with(|devtab| {
            if devtab.b_forw.is_null() {
                None
            } else {
                self.in_device_list(devtab.sentinel_const(), blkno, dev.0)
            }
        })
    }

    fn validate_device(&self, dev: DevId) -> BufferResult<()> {
        if dev.0 < 0 || block_device_for_dev(dev.0).is_some() {
            Ok(())
        } else {
            Err(BufferError::InvalidDevice)
        }
    }

    fn find_delayed_write_buffer(&self, dev: Option<DevId>) -> Option<*mut Buf> {
        unsafe {
            let sentinel = self.free_list_sentinel_const();
            let mut bp = (*sentinel).av_forw;

            while bp != sentinel {
                let matches_dev = dev.is_none_or(|dev| (*bp).b_dev == dev.0);

                if matches_dev && (*bp).b_flags.contains(BufFlag::B_DELWRI) {
                    return Some(bp);
                }

                bp = (*bp).av_forw;
            }
        }

        None
    }

    fn insert_into_device_queue(&self, dev: DevId, bp: *mut Buf) -> BufferResult<()> {
        if dev.0 < 0 {
            unsafe {
                Self::insert_device_front(self.free_list_sentinel_const(), bp);
            }
            return Ok(());
        }

        let device = block_device_for_dev(dev.0).ok_or(BufferError::InvalidDevice)?;
        device.devtab().with_mut(|devtab| unsafe {
            devtab.ensure_buffer_list();
            Self::insert_device_front(devtab.sentinel(), bp);
        });

        Ok(())
    }

    unsafe fn remove_from_current_device_queue(&self, bp: *mut Buf) {
        if !(*bp).is_on_device_list() {
            return;
        }

        Self::remove_from_device_list(bp);
    }

    fn in_device_list(
        &self,
        sentinel: *mut Buf,
        blkno: PhysicalBlock,
        dev: i16,
    ) -> Option<*mut Buf> {
        unsafe {
            let mut bp = (*sentinel).b_forw;

            while bp != sentinel {
                if (*bp).b_blkno == blkno && (*bp).b_dev == dev {
                    return Some(bp);
                }

                bp = (*bp).b_forw;
            }
        }

        None
    }

    unsafe fn init_free_list_sentinel(&mut self) {
        let sentinel = self.free_list_sentinel();
        (*sentinel).b_forw = sentinel;
        (*sentinel).b_back = sentinel;
        (*sentinel).av_forw = sentinel;
        (*sentinel).av_back = sentinel;
    }

    unsafe fn insert_device_front(sentinel: *mut Buf, bp: *mut Buf) {
        if (*bp).is_on_device_list() {
            return;
        }

        (*bp).b_forw = (*sentinel).b_forw;
        (*bp).b_back = sentinel;
        (*(*sentinel).b_forw).b_back = bp;
        (*sentinel).b_forw = bp;
    }

    unsafe fn remove_from_device_list(bp: *mut Buf) {
        (*(*bp).b_back).b_forw = (*bp).b_forw;
        (*(*bp).b_forw).b_back = (*bp).b_back;
        (*bp).b_forw = core::ptr::null_mut();
        (*bp).b_back = core::ptr::null_mut();
    }

    unsafe fn push_free_back(&mut self, bp: *mut Buf) {
        let sentinel = self.free_list_sentinel();
        let tail = (*sentinel).av_back;

        (*tail).av_forw = bp;
        (*bp).av_back = tail;
        (*bp).av_forw = sentinel;
        (*sentinel).av_back = bp;
    }

    unsafe fn remove_from_free_list(&mut self, bp: *mut Buf) {
        (*(*bp).av_back).av_forw = (*bp).av_forw;
        (*(*bp).av_forw).av_back = (*bp).av_back;
        (*bp).av_forw = core::ptr::null_mut();
        (*bp).av_back = core::ptr::null_mut();
    }

    fn is_free_list_empty(&self) -> bool {
        let sentinel = self.free_list_sentinel_const();
        unsafe { (*sentinel).av_forw == sentinel }
    }

    fn free_list_sentinel(&mut self) -> *mut Buf {
        &mut self.b_free_list as *mut Buf
    }

    fn free_list_sentinel_const(&self) -> *mut Buf {
        &self.b_free_list as *const Buf as *mut Buf
    }

    fn swap_buf_ptr(&mut self) -> *mut Buf {
        &mut self.swap_buf as *mut Buf
    }
}

impl Default for BufferManager {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_BUFFER_MANAGER: SuperCell<BufferManager> = SuperCell::new(BufferManager::new());

pub(crate) fn global_buffer_manager() -> &'static mut BufferManager {
    GLOBAL_BUFFER_MANAGER.get_mut()
}

unsafe fn sleep_on(chan: usize, pri: i32) {
    process_sleep(chan, pri);
}

unsafe fn wakeup_all(chan: usize) {
    process_wakeup_all(chan);
}

fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

unsafe extern "C" {
    fn cpp_process_sleep(chan: usize, pri: i32);
    fn cpp_process_wakeup_all(chan: usize);
}

pub fn process_sleep(chan: usize, pri: i32) {
    unsafe {
        cpp_process_sleep(chan, pri);
    }
}

pub fn process_wakeup_all(chan: usize) {
    unsafe {
        cpp_process_wakeup_all(chan);
    }
}

fn set_buffer_error(err: BufferError) {
    Userspace::get().set_error(err.into());
}

#[no_mangle]
pub extern "C" fn buffer_manager_initialize() {
    global_buffer_manager().initialize();
}

#[no_mangle]
pub extern "C" fn buffer_get_blk(dev: i16, blkno: i32) -> *mut Buf {
    match global_buffer_manager().get_blk(DevId(dev), PhysicalBlock(blkno as u32)) {
        Ok(bp) => bp,
        Err(error) => {
            set_buffer_error(error);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn buffer_brelse(bp: *mut Buf) {
    global_buffer_manager().brelse(bp);
}

#[no_mangle]
pub extern "C" fn buffer_io_wait(bp: *mut Buf) {
    if let Err(error) = global_buffer_manager().io_wait(bp) {
        set_buffer_error(error);
    }
}

#[no_mangle]
pub extern "C" fn buffer_io_done(bp: *mut Buf) {
    global_buffer_manager().io_done(bp);
}

#[no_mangle]
pub extern "C" fn buffer_bread(dev: i16, blkno: i32) -> *mut Buf {
    match global_buffer_manager().bread(DevId(dev), PhysicalBlock(blkno as u32)) {
        Ok(bp) => bp,
        Err(error) => {
            set_buffer_error(error);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn buffer_breada(dev: i16, blkno: i32, read_ahead_blkno: i32) -> *mut Buf {
    let read_ahead_blkno =
        (read_ahead_blkno != 0).then_some(PhysicalBlock(read_ahead_blkno as u32));
    match global_buffer_manager().breada(DevId(dev), PhysicalBlock(blkno as u32), read_ahead_blkno)
    {
        Ok(bp) => bp,
        Err(error) => {
            set_buffer_error(error);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "C" fn buffer_bwrite(bp: *mut Buf) {
    if let Err(error) = global_buffer_manager().bwrite(bp) {
        set_buffer_error(error);
    }
}

#[no_mangle]
pub extern "C" fn buffer_bdwrite(bp: *mut Buf) {
    global_buffer_manager().bdwrite(bp);
}

#[no_mangle]
pub extern "C" fn buffer_bawrite(bp: *mut Buf) {
    global_buffer_manager().bawrite(bp);
}

#[no_mangle]
pub extern "C" fn buffer_clr_buf(bp: *mut Buf) {
    global_buffer_manager().clr_buf(bp);
}

#[no_mangle]
pub extern "C" fn buffer_bflush(dev: i16) {
    let dev = (dev >= 0).then_some(DevId(dev));
    if let Err(error) = global_buffer_manager().bflush(dev) {
        set_buffer_error(error);
    }
}

#[no_mangle]
pub extern "C" fn buffer_swap(blkno: i32, addr: usize, count: i32, flag: u32) -> bool {
    let flag = BufFlag::from_bits_retain(flag);
    match global_buffer_manager().swap(
        PhysicalBlock(blkno as u32),
        addr,
        count.max(0) as usize,
        flag,
    ) {
        Ok(()) => true,
        Err(error) => {
            set_buffer_error(error);
            false
        }
    }
}

#[no_mangle]
pub extern "C" fn buffer_get_swap_buf() -> *mut Buf {
    global_buffer_manager().swap_buf_mut() as *mut Buf
}

#[no_mangle]
pub extern "C" fn buffer_get_b_free_list() -> *mut Buf {
    global_buffer_manager().free_list_mut() as *mut Buf
}
