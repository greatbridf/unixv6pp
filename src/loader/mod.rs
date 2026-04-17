use alloc::boxed::Box;
use eonix_mm::paging::PAGE_SIZE;
use kernel_macros::define_class_compat;

use crate::{compat::{compat_flush_page_directory, compat_inode_read, compat_user_pt}, fs::InodeRefCompat};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct NTHeader {
    sig: usize,
    file_header: FileHeader,
    opt_header: OptionalHeader32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct FileHeader {
    machine: u16,
    section_count: u16,
    timestamp: usize,
    sym_table_addr: usize,
    symbol_count: usize,
    optional_header_len: u16,
    characteristics: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct DataDirectory {
    vaddr: usize,
    len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct OptionalHeader32 {
    _we_dont_care1: [u32; 4],
    entry: usize,
    code_base: usize,
    data_base: usize,
    image_base: usize,
    _we_dont_care2: [u32; 11],
    stack_size: usize,
    _we_dont_care3: u32,
    heap_size: usize,
    _we_dont_care4: [u32; 2],
    data_dir: [DataDirectory; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct SectionHeader {
    name: [u8; 8],
    vsize: usize,
    vaddr: usize,
    _we_dont_care1: u32,
    raw_data_addr: usize,
    _we_dont_care2: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct DOSHeader {
    _we_dont_care: [u32; 15],
    new_header_addr: usize,
}

#[repr(C)]
#[derive(Debug, Default)]
pub struct PEParser {
    entry: usize,
    text: usize,
    text_len: usize,

    data: usize,
    data_len: usize,

    stack_size: usize,
    heap_size: usize,

    pe_addr: usize,
    nt_header: Option<Box<NTHeader>>,
    section_headers: Option<Box<[SectionHeader]>>,
}

trait Ext {
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

const PA_RW: usize = 1 << 1;

impl SectionHeader {
    fn name_stripped(&self) -> &[u8] {
        let end_idx = self.name
            .iter().position(|&c| c == 0)
            .unwrap_or(self.name.len());

        &self.name[..end_idx]
    }
}

impl PEParser {
    pub fn new() -> Self {
        Default::default()
    }

    fn wanted_sections(&self, skip_text: bool) -> impl Iterator<Item = &SectionHeader> {
        const SECTIONS: &[&[u8]] = &[
            b".text", b".data", b".rdata", b".rodata", b".bss",
        ];

        self.section_headers.as_deref().unwrap().iter().filter(move |section| {
            let start_idx = if skip_text { 1 } else { 0 };
            SECTIONS[start_idx..].iter()
                .find(|&&n| n == section.name_stripped()).is_some()
        })
    }

    pub fn relocate(&mut self, inode: InodeRefCompat, shared_text: bool) {
        let text_begin_pfn = self.text >> 12;
        let text_pages = self.text_len >> 12;
        let text_end_pfn = text_begin_pfn + text_pages;

        if !shared_text {
            for i in text_begin_pfn..text_end_pfn {
                compat_user_pt()[i] |= PA_RW;
            }

            compat_flush_page_directory();
        }

        let nt_header = self.nt_header.as_deref().unwrap();
        for section in self.wanted_sections(shared_text) {
            let vstart = nt_header.opt_header.image_base + section.vaddr;
            let len = (section.vsize + PAGE_SIZE - 1) / PAGE_SIZE * PAGE_SIZE;

            unsafe {
                (vstart as *mut u8).write_bytes(0, len);
            }
        }

        for section in self.wanted_sections(shared_text) {
            let dst = unsafe {
                core::slice::from_raw_parts_mut(
                    (nt_header.opt_header.image_base + section.vaddr) as *mut u8,
                    section.vsize,
                )
            };

            compat_inode_read(inode, dst, section.raw_data_addr);
        }

        if !shared_text {
            for i in 0..text_pages {
                compat_user_pt()[i] &= !PA_RW;
            }

            compat_flush_page_directory();
        }
    }

    pub fn load(&mut self, inode: InodeRefCompat) -> bool {
        let mut offset = 0;
        let mut dos_header = DOSHeader::default();
        compat_inode_read(inode, dos_header.as_buffer(), offset);

        offset += dos_header.new_header_addr;
        let mut nt_header: Box<NTHeader> = Box::default();
        compat_inode_read(inode, nt_header.as_buffer(), offset);

        const NT_SIG: usize = 0x4550;
        if nt_header.sig != NT_SIG {
            return false;
        }

        offset += size_of::<NTHeader>();
        let section_count = nt_header.file_header.section_count as usize;
        let mut section_headers: Box<[SectionHeader]> = unsafe {
            Box::new_uninit_slice(section_count).assume_init()
        };
        compat_inode_read(inode, section_headers.as_buffer(), offset);

        let OptionalHeader32 {
            image_base, code_base, data_base, data_dir, stack_size,
            heap_size, entry, ..
        } = &nt_header.opt_header;

        self.text = image_base + code_base;
        self.text_len = data_base - code_base;

        self.data = image_base + data_base;
        self.data_len = data_dir[1].vaddr - data_base;

        self.stack_size = *stack_size;
        self.heap_size = *heap_size;

        self.entry = image_base + entry;

        self.nt_header = Some(nt_header);
        self.section_headers = Some(section_headers);

        true
    }
}

define_class_compat! {impl PEParser {
    pub fn new() -> *mut PEParser {
        Box::into_raw(Box::new(PEParser::new()))
    }

    pub fn free(&mut self) {
        unsafe { Box::from_raw(this as *mut _); }
    }

    pub fn load_header(&mut self, inode: InodeRefCompat) -> bool {
        this.load(inode)
    }

    pub fn relocate(&mut self, inode: InodeRefCompat, shared_text: bool) {
        this.relocate(inode, shared_text);
    }
}}
