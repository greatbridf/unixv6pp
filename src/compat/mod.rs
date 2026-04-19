use core::{ffi::CStr, pin::Pin, ptr::NonNull};

use alloc::{borrow::ToOwned, boxed::Box};
use kernel_macros::define_class_compat;

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

define_class_compat! {impl Utils{
    pub fn get_path() -> *mut u8 {
        let dirp = Userspace::get().dirp;
        let cstr = unsafe { CStr::from_ptr(dirp as *const i8) };
        let bytes = cstr.to_bytes_with_nul();

        let boxed = Box::<[u8]>::new_uninit_slice(bytes.len() + size_of::<usize>());
        let mut boxed = unsafe { boxed.assume_init() };

        let head_ptr = boxed.as_mut_ptr() as *mut usize;
        Box::into_raw(boxed);

        let data_ptr = head_ptr.wrapping_add(1) as *mut u8;

        unsafe {
            head_ptr.write(bytes.len());
            data_ptr.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
        }

        data_ptr
    }

    pub fn put_path(path: *mut u8) {
        let head_ptr = (path as *mut usize).wrapping_sub(1);

        unsafe {
            let alloc_len = *head_ptr + size_of::<usize>();
            Box::from_raw(core::ptr::slice_from_raw_parts_mut(head_ptr as *mut u8, alloc_len));
        }
    }
}}
