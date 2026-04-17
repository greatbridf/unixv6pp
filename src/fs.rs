use alloc::sync::Arc;
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

pub use file::{File, IOParameter, OpenFiles, InodeRefCompat};
pub use file_manager::DirectoryEntry;
pub use file_system::SuperBlock;
pub use inode::Inode;

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
