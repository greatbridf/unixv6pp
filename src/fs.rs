use alloc::sync::Arc;
use core::ops::Deref;
use eonix_spin::{NoContext, Spin, SpinGuard};
use eonix_sync_base::LazyLock;
use file_system::FileSystem;
use open_file_manager::{InodeTable, OpenFileTable};

mod file;
mod file_manager;
mod file_system;
mod inode;
mod open_file_manager;

pub(crate) type FileRef = Arc<Spin<File>>;
pub(crate) type InodeRef = Arc<Spin<Inode>>;
pub(crate) type SuperBlockRef = Arc<Spin<SuperBlock>>;

pub(crate) struct InodeRefGuard(Option<InodeRef>);

impl InodeRefGuard {
    pub fn new(inode: InodeRef) -> Self {
        Self(Some(inode))
    }

    pub fn into_inner(mut self) -> InodeRef {
        self.0.take().expect("inode guard already consumed")
    }
}

impl Drop for InodeRefGuard {
    fn drop(&mut self) {
        if let Some(inode) = self.0.take() {
            global_inode_table().i_put(inode);
        }
    }
}

impl Deref for InodeRefGuard {
    type Target = InodeRef;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("inode guard already consumed")
    }
}

pub(crate) trait InodeRefPutExt {
    fn with_i_put(self) -> InodeRefGuard;
}

impl InodeRefPutExt for InodeRef {
    fn with_i_put(self) -> InodeRefGuard {
        InodeRefGuard::new(self)
    }
}

pub use file::{File, IOParameter, InodeRefCompat, OpenFiles};
pub use file_manager::DirectoryEntry;
pub use file_system::SuperBlock;
pub use inode::{inoderef_leak, Inode};

static GLOBAL_OPENFILE_TABLE: LazyLock<Spin<OpenFileTable>> =
    LazyLock::new(|| Spin::new(OpenFileTable::new()));

fn global_open_file_table() -> SpinGuard<'static, OpenFileTable, NoContext> {
    GLOBAL_OPENFILE_TABLE.lock_ctx::<NoContext>()
}

static GLOBAL_INODE_TABLE: LazyLock<Spin<InodeTable>> =
    LazyLock::new(|| Spin::new(InodeTable::new()));

fn global_inode_table() -> SpinGuard<'static, InodeTable, NoContext> {
    GLOBAL_INODE_TABLE.lock_ctx::<NoContext>()
}

static GLOBAL_FILE_SYSTEM: LazyLock<Spin<FileSystem>> =
    LazyLock::new(|| Spin::new(FileSystem::new()));

pub(crate) fn global_file_system() -> SpinGuard<'static, FileSystem, NoContext> {
    GLOBAL_FILE_SYSTEM.lock_ctx::<NoContext>()
}
