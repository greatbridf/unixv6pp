use crate::{fs::InodeRefCompat, user::Userspace};

pub fn compat_flush_page_directory() {
    unsafe {
        core::arch::asm!(
            "mov {}, %cr3",
            in (reg) 0x200000,
            options(att_syntax),
        );
    }
}

pub fn compat_user_pt() -> &'static mut [usize; 2048] {
    unsafe {
        &mut *(0xc0202000 as *mut [usize; 2048])
    }
}

pub fn compat_get_time() -> u32 {
    extern "C" {
        fn get_time() -> u32;
    }

    unsafe {
        get_time()
    }
}
