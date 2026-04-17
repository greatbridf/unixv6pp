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
