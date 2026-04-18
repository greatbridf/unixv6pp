#![no_std]

extern crate alloc;

mod compat;
mod constants;
mod dev;
mod fs;
mod loader;
mod mm;
mod print;
pub mod proc;
mod serial;
pub mod sync;
mod tty;
mod user;
mod vesa;

use core::panic::PanicInfo;

pub trait Ext {
    fn as_buffer(&mut self) -> &mut [u8];
}

impl<T> Ext for T where T: Copy {
    fn as_buffer(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self as *mut Self as *mut u8,
                core::mem::size_of::<Self>(),
            )
        }
    }
}

impl<T> Ext for [T] where T: Copy {
    fn as_buffer(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self as *mut Self as *mut u8,
                self.len() * core::mem::size_of::<T>(),
            )
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let msg = info.message();

    if let Some(msg) = msg.as_str() {
        println_fatal!("KERNEL PANIC: {}", msg);
    } else {
        println_fatal!("KERNEL PANIC: Unknown");
    }

    if let Some(loc) = info.location() {
        println_fatal!("Panicked at {}:{}", loc.file(), loc.line());
    }

    loop {}
}
