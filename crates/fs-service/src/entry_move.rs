//! Descriptor-relative moves and copies of validated entries without replacing the destination.
//! Parents are reopened from the checkout without following a raced-in symlink.

use std::ffi::{c_char, c_int, CString};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path};

unsafe extern "C" {
    fn openat(fd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(fd: c_int, path: *const c_char, mode: Mode) -> c_int;
    fn readlinkat(fd: c_int, path: *const c_char, buf: *mut c_char, size: usize) -> isize;
    fn symlinkat(target: *const c_char, fd: c_int, path: *const c_char) -> c_int;
    fn unlinkat(fd: c_int, path: *const c_char, flags: c_int) -> c_int;
}

#[cfg(target_os = "macos")]
type Mode = u16;
#[cfg(target_os = "linux")]
type Mode = u32;

#[cfg(target_os = "macos")]
const DIRECTORY_FLAGS: c_int = 0x100000 | 0x100 | 0x1000000; // DIRECTORY | NOFOLLOW | CLOEXEC
#[cfg(target_os = "linux")]
const DIRECTORY_FLAGS: c_int = 0x10000 | 0x20000 | 0x80000;
// NONBLOCK so a fifo cannot stall the open; NOFOLLOW so the final entry is the one named.
#[cfg(target_os = "macos")]
const FILE_READ_FLAGS: c_int = 0x4 | 0x100 | 0x1000000;
#[cfg(target_os = "linux")]
const FILE_READ_FLAGS: c_int = 0x800 | 0x20000 | 0x80000;
// EXCL refuses a destination that appeared after the caller's existence check.
#[cfg(target_os = "macos")]
const FILE_CREATE_FLAGS: c_int = 0x1 | 0x200 | 0x800 | 0x100 | 0x1000000;
#[cfg(target_os = "linux")]
const FILE_CREATE_FLAGS: c_int = 0x1 | 0x40 | 0x80 | 0x20000 | 0x80000;
// `O_NOFOLLOW` reports `ELOOP` when the final component is a symlink.
// `ErrorKind::FilesystemLoop` is still unstable on the pinned toolchain.
#[cfg(target_os = "macos")]
const ELOOP: i32 = 62;
#[cfg(target_os = "linux")]
const ELOOP: i32 = 40;
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

pub(super) enum CopyError {
    Io(io::Error),
    TooLarge { size: u64 },
    NotAFile,
}

impl From<io::Error> for CopyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Copy one entry. A symlink is copied as a symlink; a directory is refused.
pub(super) fn copy(root: &Path, source: &Path, target: &Path, limit: u64) -> Result<(), CopyError> {
    let relative = |path: &Path| {
        path.strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| io::Error::other("copy escapes workspace"))
    };
    let source = relative(source)?;
    let target = relative(target)?;
    let root = File::options()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS)
        .open(root)?;
    let source_parent = parent(&root, source.parent().unwrap_or(Path::new("")), false)?;
    let source_name = entry_name(&source)?;
    let target_name = entry_name(&target)?;
    match open_nofollow(&source_parent, &source_name) {
        Ok(file) => copy_regular(file, &root, &target, &target_name, limit),
        Err(error) if error.raw_os_error() == Some(ELOOP) => {
            copy_symlink(&source_parent, &source_name, &root, &target, &target_name)
        }
        Err(error) => Err(error.into()),
    }
}

fn entry_name(path: &Path) -> io::Result<CString> {
    CString::new(
        path.file_name()
            .ok_or_else(|| io::Error::other("copy names root"))?
            .as_bytes(),
    )
    .map_err(Into::into)
}

fn open_nofollow(parent: &File, name: &CString) -> io::Result<File> {
    let fd = unsafe { openat(parent.as_raw_fd(), name.as_ptr(), FILE_READ_FLAGS) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // `openat` transferred this descriptor; `File` is its only owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn copy_regular(
    source: File,
    root: &File,
    target: &Path,
    target_name: &CString,
    limit: u64,
) -> Result<(), CopyError> {
    let meta = source.metadata()?;
    if !meta.is_file() {
        return Err(CopyError::NotAFile);
    }
    if meta.len() > limit {
        return Err(CopyError::TooLarge { size: meta.len() });
    }
    let target_parent = parent(root, target.parent().unwrap_or(Path::new("")), true)?;
    let fd = unsafe {
        openat(
            target_parent.as_raw_fd(),
            target_name.as_ptr(),
            FILE_CREATE_FLAGS,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut dest = unsafe { File::from_raw_fd(fd) };
    let copied = match io::copy(&mut source.take(limit + 1), &mut dest) {
        Ok(copied) => copied,
        Err(error) => {
            remove_entry(&target_parent, target_name);
            return Err(error.into());
        }
    };
    if copied > limit {
        remove_entry(&target_parent, target_name);
        return Err(CopyError::TooLarge { size: copied });
    }
    let mut perms = meta.permissions();
    perms.set_mode(perms.mode() & 0o777);
    if let Err(error) = dest.set_permissions(perms).and_then(|()| dest.sync_all()) {
        remove_entry(&target_parent, target_name);
        return Err(error.into());
    }
    Ok(())
}

fn copy_symlink(
    source_parent: &File,
    source_name: &CString,
    root: &File,
    target: &Path,
    target_name: &CString,
) -> Result<(), CopyError> {
    let link = read_link(source_parent, source_name)?;
    let target_parent = parent(root, target.parent().unwrap_or(Path::new("")), true)?;
    let result = unsafe {
        symlinkat(
            link.as_ptr(),
            target_parent.as_raw_fd(),
            target_name.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().into())
    }
}

fn read_link(parent: &File, name: &CString) -> io::Result<CString> {
    let mut buf = vec![0u8; 4096];
    let read = unsafe {
        readlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    let read = usize::try_from(read).map_err(|_| io::Error::other("symlink target length"))?;
    // A full buffer is a truncated target, not a target that happened to fit.
    if read == buf.len() {
        return Err(io::Error::other("symlink target is too long"));
    }
    buf.truncate(read);
    CString::new(buf).map_err(|_| io::Error::other("symlink target contains a NUL"))
}

fn remove_entry(parent: &File, name: &CString) {
    let _ = unsafe { unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
}
