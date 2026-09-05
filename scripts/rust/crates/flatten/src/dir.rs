use std::ffi::OsStr;
use std::io;
use std::path::Path;

#[cfg(unix)]
pub use unix::{Dir, directory_not_empty};

#[cfg(not(unix))]
pub use portable::{Dir, directory_not_empty};

fn unusable(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unusable name: {}", Path::new(name).display()),
    )
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix {
    use std::ffi::{CString, OsStr};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub struct Dir(OwnedFd);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Dir> {
            let name = cstring(path.as_os_str())?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
            // SAFETY: the name is a valid NUL-terminated string that outlives
            // the call, and the call either returns a descriptor this takes
            // ownership of or reports why it could not.
            let fd = unsafe { libc::open(name.as_ptr(), flags) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` is a fresh descriptor nothing else owns.
            Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        pub fn child(&self, name: &OsStr) -> io::Result<Dir> {
            let name = cstring(name)?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
            // SAFETY: as `open` above, with this directory's descriptor as
            // the name's starting point.
            let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` is a fresh descriptor nothing else owns.
            Ok(Dir(unsafe { OwnedFd::from_raw_fd(fd) }))
        }

        pub fn move_entry(&self, name: &OsStr, into: &Dir, as_name: &OsStr) -> io::Result<()> {
            let from = cstring(name)?;
            let to = cstring(as_name)?;
            // SAFETY: both names are valid NUL-terminated strings that
            // outlive the call, and both descriptors are open directories.
            let status = unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    from.as_ptr(),
                    into.0.as_raw_fd(),
                    to.as_ptr(),
                )
            };
            result(status)
        }

        pub fn remove_dir(&self, name: &OsStr) -> io::Result<()> {
            let name = cstring(name)?;
            // SAFETY: the name is a valid NUL-terminated string that outlives
            // the call, and the descriptor is an open directory.
            let status =
                unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
            result(status)
        }
    }

    pub fn directory_not_empty(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(libc::ENOTEMPTY) | Some(libc::EEXIST)
        )
    }

    fn result(status: libc::c_int) -> io::Result<()> {
        if status < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn cstring(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| super::unusable(name))
    }
}

#[cfg(not(unix))]
mod portable {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Dir> {
            let metadata = fs::metadata(path)?;
            if !metadata.is_dir() {
                return Err(io::Error::from(io::ErrorKind::NotADirectory));
            }
            Ok(Dir(path.to_path_buf()))
        }

        pub fn child(&self, name: &OsStr) -> io::Result<Dir> {
            let path = self.join(name)?;
            if !fs::symlink_metadata(&path)?.is_dir() {
                return Err(io::Error::from(io::ErrorKind::NotADirectory));
            }
            Ok(Dir(path))
        }

        pub fn move_entry(&self, name: &OsStr, into: &Dir, as_name: &OsStr) -> io::Result<()> {
            fs::rename(self.join(name)?, into.join(as_name)?)
        }

        pub fn remove_dir(&self, name: &OsStr) -> io::Result<()> {
            fs::remove_dir(self.join(name)?)
        }

        fn join(&self, name: &OsStr) -> io::Result<PathBuf> {
            let mut parts = Path::new(name).components();
            match (parts.next(), parts.next()) {
                (Some(_), None) => Ok(self.0.join(name)),
                _ => Err(super::unusable(name)),
            }
        }
    }

    pub fn directory_not_empty(error: &io::Error) -> bool {
        error.kind() == io::ErrorKind::DirectoryNotEmpty
    }
}
