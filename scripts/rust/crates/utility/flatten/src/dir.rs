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
mod unix {
    use std::ffi::{CString, OsStr};
    use std::io;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use rustix::fs::{AtFlags, Mode, OFlags, open, openat, renameat, unlinkat};

    pub struct Dir(OwnedFd);

    impl Dir {
        pub fn open(path: &Path) -> io::Result<Dir> {
            let name = cstring(path.as_os_str())?;
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
            open(name.as_c_str(), flags, Mode::empty())
                .map(Dir)
                .map_err(io::Error::from)
        }

        pub fn child(&self, name: &OsStr) -> io::Result<Dir> {
            let name = cstring(name)?;
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
            openat(&self.0, name.as_c_str(), flags, Mode::empty())
                .map(Dir)
                .map_err(io::Error::from)
        }

        pub fn move_entry(&self, name: &OsStr, into: &Dir, as_name: &OsStr) -> io::Result<()> {
            let from = cstring(name)?;
            let to = cstring(as_name)?;
            renameat(&self.0, from.as_c_str(), &into.0, to.as_c_str()).map_err(io::Error::from)
        }

        pub fn remove_dir(&self, name: &OsStr) -> io::Result<()> {
            let name = cstring(name)?;
            unlinkat(&self.0, name.as_c_str(), AtFlags::REMOVEDIR).map_err(io::Error::from)
        }
    }

    pub fn directory_not_empty(error: &io::Error) -> bool {
        matches!(
            error.raw_os_error(),
            Some(code) if code == rustix::io::Errno::NOTEMPTY.raw_os_error()
                || code == rustix::io::Errno::EXIST.raw_os_error()
        )
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
