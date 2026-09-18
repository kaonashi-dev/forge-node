//! Descriptor-relative moves of validated entries without replacing the destination.
//! Parents are reopened from the checkout without following a raced-in symlink.

use std::ffi::{c_char, c_int, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};

unsafe extern "C" {
    fn openat(fd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(fd: c_int, path: *const c_char, mode: Mode) -> c_int;
}

#[cfg(target_os = "macos")]
type Mode = u16;
#[cfg(target_os = "linux")]
type Mode = u32;

#[cfg(target_os = "macos")]
const DIRECTORY_FLAGS: c_int = 0x100000 | 0x100 | 0x1000000; // DIRECTORY | NOFOLLOW | CLOEXEC
#[cfg(target_os = "linux")]
const DIRECTORY_FLAGS: c_int = 0x10000 | 0x20000 | 0x80000;
#[cfg(target_os = "macos")]
const RENAME_EXCLUSIVE: u32 = 4;
#[cfg(target_os = "linux")]
const RENAME_EXCLUSIVE: u32 = 1;

fn parent(root: &File, relative: &Path, create: bool) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::other("invalid directory component"));
        };
        let name = CString::new(name.as_bytes())?;
        // The descriptors and C string remain live; NOFOLLOW rejects replaced parents.
        let mut fd = unsafe { openat(directory.as_raw_fd(), name.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 && create && io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
            let created = unsafe { mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o777) };
            if created < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(io::Error::last_os_error());
            }
            fd = unsafe { openat(directory.as_raw_fd(), name.as_ptr(), DIRECTORY_FLAGS) };
        }
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // A successful openat transfers this descriptor to File's sole ownership.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

pub(super) fn rename(root: &Path, source: &Path, target: &Path) -> io::Result<()> {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| io::Error::other("move escapes workspace"))
    };
    let source = relative(source)?;
    let target = relative(target)?;
    let root = File::options()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS)
        .open(root)?;
    let source_parent = parent(&root, source.parent().unwrap_or(Path::new("")), false)?;
    let target_parent = parent(&root, target.parent().unwrap_or(Path::new("")), true)?;
    let name = |path: &Path| -> io::Result<CString> {
        CString::new(
            path.file_name()
                .ok_or_else(|| io::Error::other("move names root"))?
                .as_bytes(),
        )
        .map_err(Into::into)
    };
    let source = name(&source)?;
    let target = name(&target)?;
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn renameatx_np(
            from_fd: c_int,
            from: *const c_char,
            to_fd: c_int,
            to: *const c_char,
            flags: u32,
        ) -> c_int;
    }
    #[cfg(target_os = "linux")]
    unsafe extern "C" {
        fn renameat2(
            from_fd: c_int,
            from: *const c_char,
            to_fd: c_int,
            to: *const c_char,
            flags: u32,
        ) -> c_int;
    }
    // Parent descriptors anchor both lookups; EXCL/NOREPLACE protects the final entry.
    #[cfg(target_os = "macos")]
    let result = unsafe {
        renameatx_np(
            source_parent.as_raw_fd(),
            source.as_ptr(),
            target_parent.as_raw_fd(),
            target.as_ptr(),
            RENAME_EXCLUSIVE,
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        renameat2(
            source_parent.as_raw_fd(),
            source.as_ptr(),
            target_parent.as_raw_fd(),
            target.as_ptr(),
            RENAME_EXCLUSIVE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
