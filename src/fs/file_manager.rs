use kernel_macros::define_class_compat;

use crate::{
    constants::PosixError, dev::buffer::{Buffer, LogicalBlock}, fs::{
        GLOBAL_INODE_TABLE, Inode, InodeRef, InodeRefCompat, InodeRefGuard, InodeRefPutExt, inode::{InodeFlag, InodeMode, inoderef_leak}
    }, sync::SpinExt, user::Userspace
};

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirSearchMode {
    Open = 0,
    Create = 1,
    Delete = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DirectoryEntry {
    pub m_ino: i32,
    pub m_name: [u8; 28],
}

#[repr(C)]
pub struct FileManager {
    root_inode: InodeRefCompat,
}

impl DirectoryEntry {
    pub const DIRSIZ: usize = 28;

    pub const fn new() -> Self {
        Self {
            m_ino: 0,
            m_name: [0; Self::DIRSIZ],
        }
    }

    pub fn name(&self) -> &[u8] {
        let end = self.m_name.iter().position(|&s| s == 0).expect("Invalid string");
        &self.m_name[..end]
    }
}

trait InodeRefExt {
    fn has_access(&self, mode: InodeMode) -> bool;
    fn readblk(&self, lbn: LogicalBlock) -> Buffer;
    fn search(
        &self, name: &[u8], create: bool, remove: bool
    ) -> Result<Option<InodeRefGuard>, PosixError>;
}

impl InodeRefExt for InodeRef {
    fn has_access(&self, mode: InodeMode) -> bool {
        extern "C" {
            fn compat_fm_access(inode: InodeRefCompat, mode: InodeMode) -> bool;
        }

        unsafe {
            !compat_fm_access(inoderef_leak(self.clone()), mode)
        }
    }

    fn readblk(&self, lbn: LogicalBlock) -> Buffer {
        let mut ra = None;
        self.lock().get_blk(lbn, &mut ra).unwrap()
    }

    fn search(
        &self, name: &[u8], create: bool, remove: bool
    ) -> Result<Option<InodeRefGuard>, PosixError> {
        const DENTRY_SIZE: usize = size_of::<DirectoryEntry>();
        let mut count = self.lock().i_size as usize / DENTRY_SIZE;
        let mut offset = 0;
        let mut free_offset = None;

        let mut buffer = self.readblk(LogicalBlock(0));
        while count != 0 {
            let blkoff = offset % Inode::BLOCK_SIZE;
            let idx = blkoff / DENTRY_SIZE;

            if offset != 0 && blkoff == 0 {
                buffer = self.readblk(LogicalBlock((offset / Inode::BLOCK_SIZE) as u32));
            }

            let dentry = &buffer.as_slice::<DirectoryEntry>()[idx];

            if dentry.m_ino == 0 {
                free_offset.get_or_insert(offset);
            }

            if dentry.m_ino != 0 && dentry.name() == name {
                Userspace::get().dentry = *dentry;
                break;
            }

            offset += DENTRY_SIZE;
            count -= 1;
        }

        let mut ret = None;
        let mut err = None;
        loop {
            if count == 0 {
                if !create {
                    err = Some(PosixError::ENOENT);
                    break;
                }

                if !self.has_access(InodeMode::IWRITE) {
                    err = Some(PosixError::EACCES);
                    break;
                }

                Userspace::get().set_cwd_parent(self.clone());

                if free_offset.is_none() {
                    free_offset.insert(offset);
                    self.lock().i_flag.insert(InodeFlag::IUPD);
                }

                offset = free_offset.unwrap();
                break;
            }

            if remove {
                if !self.has_access(InodeMode::IWRITE) {
                    err = Some(PosixError::EACCES);
                }
                break;
            }

            let dev = self.lock().i_dev;
            let ino = Userspace::get().dentry.m_ino;

            crate::println_debug!("ino: {ino}");

            match GLOBAL_INODE_TABLE.lock().i_get(dev, ino) {
                Err(e) => err = Some(e),
                Ok(iref) => ret = Some(iref),
            }

            break;
        }

        Userspace::get().ioparam.m_offset = offset;
        Userspace::get().ioparam.m_count = count;

        match (err, ret) {
            (Some(err), _) => Err(err),
            (None, ret) => Ok(ret),
        }
    }
}

impl FileManager {
    pub fn find(&self, mut path: &[u8], mode: DirSearchMode) -> Result<Option<InodeRef>, PosixError> {
        let mut iref;

        crate::println_debug!("{:?}", unsafe {
            core::str::from_utf8_unchecked(path)
        });

        if let Some(b'/') = path.first() {
            iref = self.root_inode.own().with_i_put();
        } else {
            iref = Userspace::get().getcwd().with_i_put();
        }

        while let Some(b'/') = path.first() {
            path = &path[1..];
        }

        while Userspace::get().error.is_none() && !path.is_empty() {
            if (iref.lock().i_mode & InodeMode::IFMT) != InodeMode::IFDIR {
                return Err(PosixError::ENOTDIR);
            }

            if !iref.has_access(InodeMode::IEXEC) {
                return Err(PosixError::EACCES);
            }

            let next_idx = path.iter().position(|&c| c == b'/').unwrap_or(path.len());
            let name = &path[..next_idx];
            path = &path[next_idx..];

            if name.len() >= DirectoryEntry::DIRSIZ {
                panic!("Name too long");
            }

            {
                let dbuf = Userspace::get().argdir_mut();
                dbuf[..name.len()].copy_from_slice(name);
                dbuf[name.len()..].fill(0);
            }

            while let Some(b'/') = path.first() {
                path = &path[1..];
            }

            if let Some(i) = iref.search(name,
                mode == DirSearchMode::Create && path.is_empty(),
                mode == DirSearchMode::Delete && path.is_empty(),
            )? {
                iref = i;
            } else {
                return Ok(None);
            }
        }

        Ok(Some(iref.into_inner()))
    }
}

define_class_compat! {impl FileManager {
    pub fn namei(&mut self, mode: DirSearchMode) -> Option<InodeRefCompat> {
        let path = Userspace::get().argdir();
        match this.find(path, mode) {
            Ok(Some(iref)) => Some(inoderef_leak(iref)),
            Ok(None) => None,
            Err(err) => {
                Userspace::get().error = Some(err);
                None
            }
        }
    }
}}
