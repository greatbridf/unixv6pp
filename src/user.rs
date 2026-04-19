use alloc::boxed::Box;
use kernel_macros::define_class_compat;

use crate::{
    constants::{PosixError, SIGMAX},
    fs::{DirectoryEntry, IOParameter, Inode, OpenFiles},
};

#[derive(Clone, Copy)]
pub struct Pointer(usize);

pub struct Text;
pub struct Terminal;

#[repr(u32)]
enum ProcessState {
    SNULL= 0,
    SSLEEP= 1,
    SWAIT= 2,
    SRUN= 3,
    SIDL= 4,
    SZOMB= 5,
    SSTOP= 6,
}

#[repr(C)]
pub struct Process {
    uid: u16,
    pid: u32,
    ppid: u32,

    addr: usize,
    size: u32,
    textp: *mut Text,
    stat: ProcessState,
    flag: u32,

    pri: u32,
    cpu: u32,
    nice: u32,
    time: u32,

    wchan: usize,

    sig: u32,
    tty: *const Terminal,
    sigmap: usize,
}

#[repr(C)]
#[derive(Clone)]
pub struct MemoryDescriptor {
    /// Opaque for now...
    data: [usize; 6],
}

#[repr(C)]
pub struct Userspace {
    /// Save esp and ebp
    rsav: [Pointer; 2],
    /// Save esp and ebp AGAIN
    ssav: [Pointer; 2],

    proc: *mut Process,
    mem: MemoryDescriptor,

    ar0: *mut u32,

    /// Used by syscall handlers
    args: [usize; 5],

    pub dirp: *mut u8,

    /// User time elapsed
    utime: u32,
    /// Sys time elapsed
    stime: u32,

    /// Sum of all children's user time
    children_utime: u32,
    /// Sum of all children's sys time
    children_stime: u32,

    /// Pending signals
    signals: [usize; SIGMAX],

    /// Used to jump back to Trap() on signal received
    qsav: [Pointer; 2],

    /// Do we have pending signals?
    signal_pending: bool,

    /// Inode of our working directory
    cwd: *mut Inode,
    // cwd: InodeRef,
    /// Inode of our pwd's parent
    cwd_parent: *mut Inode,
    // cwd_parent: InodeRef,
    /// Dentry of our pwd
    dentry: DirectoryEntry,

    /// Last component of pwd
    cwd_name: [u8; 28],
    /// Full path of pwd
    cwd_full: [u8; 128],

    /// Userspace error code
    pub error: Option<PosixError>,

    uid: u16,
    gid: u16,
    euid: u16,
    egid: u16,

    pub open_files: OpenFiles,
    pub ioparam: IOParameter,
}

impl Userspace {
    pub fn get() -> &'static mut Self {
        const RUST_USER_ADDRESS: usize = 0xc03ff000;
        unsafe { &mut *(RUST_USER_ADDRESS as *mut Self) }
    }

    pub fn set_error(&mut self, errno: PosixError) {
        self.error = Some(errno);
    }

    pub fn set_user_retval(&mut self, retval: u32) {
        unsafe {
            self.ar0.write(retval);
        }
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn io_param_mut(&mut self) -> &mut IOParameter {
        &mut self.ioparam
    }

    fn is_root(&mut self) -> bool {
        if self.uid == 0 {
            return true;
        }

        self.error = Some(PosixError::EPERM);
        false
    }

    fn setuid(&mut self, uid: u16, suid: u16) {
        if self.euid == uid || self.is_root() {
            self.uid = uid;
            self.euid = uid;
            unsafe {
                (&mut *self.proc).uid = uid;
            }
        } else {
            self.error = Some(PosixError::EPERM);
        }
    }

    fn getuid(&self) -> u32 {
        ((self.uid as u32) << 16) | ((self.euid as u32) & 0xff)
    }

    fn setgid(&mut self, gid: u16, sgid: u16) {
        if self.egid == gid || self.is_root() {
            self.gid = gid;
            self.egid = gid;
        } else {
            self.error = Some(PosixError::EPERM);
        }
    }

    fn getgid(&self) -> u32 {
        ((self.gid as u32) << 16) | ((self.egid as u32) & 0xff)
    }

    fn getpwd(&self) -> [u8; 128] {
        self.cwd_full.clone()
    }
}

impl MemoryDescriptor {
    pub const fn new() -> Self {
        Self { data: [0; 6] }
    }
}

macro_rules! define_user_compat {
{ $( $rust_ident:ident: $type:ty => $c_ident:ident := $init:expr; )* } => {
    struct SaveHandle {
        $(
            $rust_ident: $type,
        )*
    }

    define_class_compat!{impl Userspace {
        pub fn before_fork() -> Box<SaveHandle> {
            let user = Userspace::get();

            Box::new(SaveHandle {
                $(
                    $rust_ident: user.$rust_ident.clone(),
                )*
            })
        }

        pub fn after_fork(handle: Box<SaveHandle>) {
            let user = Userspace::get();

            $(
                user.$rust_ident = handle.$rust_ident;
            )*
        }

        pub fn init() {
            let user = Userspace::get();

            unsafe {
                $(
                    (&raw mut user.$rust_ident).write($init);
                )*
            }
        }
    }}

    define_class_compat! {impl User {
        $(
            pub fn $c_ident() -> *mut $type {
                &raw mut Userspace::get().$rust_ident
            }
        )*
    }}
};
}

define_user_compat! {
    signal_pending: bool => get_intflg_ := false;
    signals: [usize; SIGMAX] => get_signal_ := [0; SIGMAX];
    open_files: OpenFiles => get_ofiles_ := OpenFiles::new();
    ioparam: IOParameter => get_IOParam_ := IOParameter::new();
    utime: u32 => get_utime_ := 0;
    stime: u32 => get_stime_ := 0;
    children_utime: u32 => get_cutime_ := 0;
    children_stime: u32 => get_cstime_ := 0;
    uid: u16 => get_uid_ := 0;
    euid: u16 => get_ruid_ := 0;
    gid: u16 => get_gid_ := 0;
    egid: u16 => get_rgid_ := 0;
    args: [usize; 5] => get_arg_ := [0; 5];
    dirp: *mut u8 => get_dirp_ := core::ptr::null_mut();
    cwd_full: [u8; 128] => get_curdir_ := {
        let mut arr = [0; 128]; arr[0] = b'/'; arr
    };
    dentry: DirectoryEntry => get_dent_ := DirectoryEntry::new();
    cwd_name: [u8; 28] => get_dbuf_ := [0; 28];
    mem: MemoryDescriptor => get_MemoryDescriptor_ := MemoryDescriptor::new();
    ar0: *mut u32 => get_ar0_ := core::ptr::null_mut();
    proc: *mut Process => get_procp_ := core::ptr::null_mut();
    cwd: *mut Inode => get_cdir_ := core::ptr::null_mut();
    cwd_parent: *mut Inode => get_pdir_ := core::ptr::null_mut();
    error: Option<PosixError> => get_error_ := None;
    rsav: [Pointer; 2] => get_rsav_ := [Pointer(0); 2];
    ssav: [Pointer; 2] => get_ssav_ := [Pointer(0); 2];
    qsav: [Pointer; 2] => get_qsav_ := [Pointer(0); 2];
}

define_class_compat! {impl Userspace {
    pub fn is_root(&mut self) -> bool {
        this.is_root()
    }
}}
