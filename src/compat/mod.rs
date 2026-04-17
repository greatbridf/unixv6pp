use core::ptr::NonNull;

use crate::{fs::InodeRefCompat, user::Userspace};

extern "C" {
    fn inode_read(inode: InodeRefCompat);
}

// Assume that reads will never fail...
pub fn compat_inode_read(inode: InodeRefCompat, buffer: &mut [u8], offset: usize) {
    let user = Userspace::get();
    user.ioparam.m_base = buffer.as_ptr().addr();
    user.ioparam.m_offset = offset;
    user.ioparam.m_count = buffer.len();

    unsafe {
        inode_read(inode);
    }
}

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
