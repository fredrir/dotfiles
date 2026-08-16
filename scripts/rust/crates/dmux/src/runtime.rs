//! Secure cross-platform runtime-directory resolver (plan §10.1).
//!
//! All sockets, action tokens, descriptors, and kernel-lock files live under
//! one verified per-user runtime directory. On Linux that is
//! `$XDG_RUNTIME_DIR/dmux`; on macOS `<_CS_DARWIN_USER_TEMP_DIR>/dmux` via
//! `confstr(3)` — never a launchd-exported `XDG_RUNTIME_DIR` and never a
//! blindly trusted `$TMPDIR`. Persistent registry/snapshots never live here.
//!
//! This resolver performs the metadata-level checks (ownership, mode,
//! symlink rejection). Descriptor-relative no-follow opens for individual
//! endpoints arrive with the P5 runtime broker; callers must still create
//! entries 0600 and revalidate after exec (plan §10.1).
//!
//! Root-owned (plan §19).

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const SUBDIR: &str = "dmux";

/// The verified dmux runtime directory, created 0700 if missing.
pub fn dmux_runtime_dir() -> io::Result<PathBuf> {
    secured_runtime_subdir(&platform_base_dir()?)
}

#[cfg(target_os = "macos")]
fn platform_base_dir() -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut buf = vec![0u8; 1024];
    loop {
        let n = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "confstr(_CS_DARWIN_USER_TEMP_DIR) is unavailable",
            ));
        }
        if n <= buf.len() {
            buf.truncate(n - 1); // drop the trailing NUL
            return Ok(PathBuf::from(OsString::from_vec(buf)));
        }
        buf.resize(n, 0);
    }
}

#[cfg(target_os = "linux")]
fn platform_base_dir() -> io::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XDG_RUNTIME_DIR must be an absolute path",
        ));
    }
    Ok(dir)
}

/// Validates `base` (current-UID-owned, non-symlink, not group/world
/// writable directory) and returns `base/dmux` created/verified mode 0700.
/// Exposed at this granularity so tests can drive it with scratch bases.
pub fn secured_runtime_subdir(base: &Path) -> io::Result<PathBuf> {
    let euid = unsafe { libc::geteuid() };

    let meta = fs::symlink_metadata(base)?;
    if !meta.is_dir() {
        return Err(reject(base, "is not a directory (or is a symlink)"));
    }
    if meta.uid() != euid {
        return Err(reject(base, "is not owned by the current user"));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(reject(base, "is group- or world-writable"));
    }

    let dir = base.join(SUBDIR);
    match fs::symlink_metadata(&dir) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(reject(
                    &dir,
                    "exists but is not a directory (or is a symlink)",
                ));
            }
            if meta.uid() != euid {
                return Err(reject(&dir, "is not owned by the current user"));
            }
            if meta.mode() & 0o777 != 0o700 {
                return Err(reject(&dir, "must be mode 0700"));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
            // Re-verify: create() would have failed on an existing entry, but
            // the entry could have been raced in; check what is there now.
            let meta = fs::symlink_metadata(&dir)?;
            if !meta.is_dir() || meta.uid() != euid || meta.mode() & 0o777 != 0o700 {
                return Err(reject(&dir, "failed post-create verification"));
            }
        }
        Err(e) => return Err(e),
    }
    Ok(dir)
}

fn reject(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("runtime dir {}: {}", path.display(), why),
    )
}

/// Owner-side Wez CLI selection for managed-mux operations (bin + config).
/// Provisioning owns these paths; revisited at P9/P11 when the GUI bridge
/// lands. In the lib (not the binary) so the remote owner agent can build
/// a Wez provider (P8b).
pub fn production_wez_paths() -> (String, String) {
    // DMUX_WEZ_BIN / DMUX_WEZ_CONFIG are owner-side TEST SEAMS (like
    // DMUX_RUNTIME_DIR): scratch servers substitute their bin/config
    // without touching production resolution.
    let bin = std::env::var("DMUX_WEZ_BIN").unwrap_or_else(|_| {
        ["/opt/homebrew/bin/wezterm", "/usr/bin/wezterm"]
            .iter()
            .find(|p| Path::new(p).exists())
            .map(|p| p.to_string())
            .unwrap_or_else(|| "wezterm".to_string())
    });
    let config = std::env::var("DMUX_WEZ_CONFIG").unwrap_or_else(|_| {
        format!(
            "{}/dotfiles/shared/wezterm/mux/dmux-mux.lua",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    (bin, config)
}

// ---------------------------------------------------------------------------
// Managed-mux runtime descriptor (plan §15.1; ADR 002). Written by
// `mux-startup` in the service config; owner-side callers read it to learn
// the exact service socket and current epoch. Reading is advisory — every
// consumer still verifies the endpoint identity through the normal
// strict-selection checks before trusting a scan.

pub const WEZ_DESCRIPTOR_FILE: &str = "wez-dmux.json";

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WezMuxDescriptor {
    pub descriptor_version: u32,
    pub state: String,
    pub epoch: String,
    pub pid: u32,
    pub socket: String,
    pub start_token: String,
    #[serde(default)]
    pub boot_nonce: Option<String>,
    #[serde(default)]
    pub backend_instance_uid: Option<String>,
    #[serde(default)]
    pub recovery_generation: Option<String>,
    #[serde(default)]
    pub sentinel_window_id: Option<u64>,
    #[serde(default)]
    pub sentinel_tab_id: Option<u64>,
    #[serde(default)]
    pub sentinel_pane_id: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl WezMuxDescriptor {
    /// Presentation/recovery callers must not treat a syntactically valid
    /// `starting`/`failed` descriptor as a usable managed server. This
    /// validates the identity-bearing fields; the provider still performs
    /// the exact-socket + sentinel scan before any native-ID action.
    pub fn require_ready(&self) -> io::Result<()> {
        if self.state != "ready" {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "managed Wez mux is {}{}",
                    self.state,
                    self.error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                ),
            ));
        }
        parse_descriptor_uuid("epoch", &self.epoch)?;
        let instance = self
            .backend_instance_uid
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "managed Wez mux descriptor has no backend_instance_uid",
                )
            })?;
        parse_descriptor_uuid("backend_instance_uid", instance)?;
        if !Path::new(&self.socket).is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor socket is not absolute",
            ));
        }
        if self.start_token.is_empty() || self.pid == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor has no process witness",
            ));
        }
        Ok(())
    }
}

fn parse_descriptor_uuid(field: &str, value: &str) -> io::Result<uuid::Uuid> {
    value.parse::<uuid::Uuid>().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed Wez mux descriptor {field}: {e}"),
        )
    })
}

/// Read the managed-mux descriptor from the verified runtime dir. `Ok(None)`
/// when the service has never written one (stopped/uninstalled).
pub fn read_wez_descriptor() -> io::Result<Option<WezMuxDescriptor>> {
    read_wez_descriptor_in(&dmux_runtime_dir()?)
}

pub fn read_wez_descriptor_in(runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>> {
    let path = runtime_dir.join(WEZ_DESCRIPTOR_FILE);
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let metadata = file.metadata()?;
    let euid = unsafe { libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != euid || metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "descriptor {} must be a current-user-owned non-symlink file with mode 0600",
                path.display()
            ),
        ));
    }
    if metadata.len() > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("descriptor {} exceeds 64 KiB", path.display()),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes)?;
    let descriptor: WezMuxDescriptor = serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("descriptor {}: {e}", path.display()),
        )
    })?;
    if descriptor.descriptor_version != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "descriptor {}: unsupported version {}",
                path.display(),
                descriptor.descriptor_version
            ),
        ));
    }
    Ok(Some(descriptor))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn scratch_base() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn creates_and_reuses_a_0700_subdir() {
        let base = scratch_base();
        let dir = secured_runtime_subdir(base.path()).unwrap();
        assert_eq!(dir, base.path().join("dmux"));
        let mode = fs::symlink_metadata(&dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
        // Idempotent on the second call.
        assert_eq!(secured_runtime_subdir(base.path()).unwrap(), dir);
    }

    #[test]
    fn rejects_world_writable_base() {
        let base = scratch_base();
        fs::set_permissions(base.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let err = secured_runtime_subdir(base.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }

    #[test]
    fn rejects_symlink_base_and_symlink_subdir() {
        let holder = scratch_base();
        let real = holder.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = holder.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(secured_runtime_subdir(&link).is_err());

        let base = scratch_base();
        std::os::unix::fs::symlink(&real, base.path().join("dmux")).unwrap();
        assert!(secured_runtime_subdir(base.path()).is_err());
    }

    #[test]
    fn rejects_file_where_subdir_should_be() {
        let base = scratch_base();
        fs::write(base.path().join("dmux"), b"nope").unwrap();
        assert!(secured_runtime_subdir(base.path()).is_err());
    }

    #[test]
    fn rejects_wrong_mode_existing_subdir() {
        let base = scratch_base();
        let dir = base.path().join("dmux");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(secured_runtime_subdir(base.path()).is_err());
    }

    #[test]
    fn descriptor_requires_private_file_and_ready_identity() {
        let base = scratch_base();
        let path = base.path().join(WEZ_DESCRIPTOR_FILE);
        let epoch = uuid::Uuid::new_v4();
        let instance = uuid::Uuid::new_v4();
        fs::write(
            &path,
            serde_json::json!({
                "descriptor_version": 1,
                "state": "ready",
                "epoch": epoch,
                "pid": 42,
                "socket": "/tmp/dmux-test.sock",
                "start_token": "42-token",
                "backend_instance_uid": instance,
                "boot_nonce": uuid::Uuid::new_v4(),
            })
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let descriptor = read_wez_descriptor_in(base.path()).unwrap().unwrap();
        descriptor.require_ready().unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            read_wez_descriptor_in(base.path()).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn starting_descriptor_is_not_ready() {
        let descriptor = WezMuxDescriptor {
            descriptor_version: 1,
            state: "starting".into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            pid: 42,
            socket: "/tmp/dmux-test.sock".into(),
            start_token: "42-token".into(),
            boot_nonce: None,
            backend_instance_uid: Some(uuid::Uuid::new_v4().to_string()),
            recovery_generation: Some("generation-1".into()),
            sentinel_window_id: None,
            sentinel_tab_id: None,
            sentinel_pane_id: None,
            error: None,
        };
        assert_eq!(
            descriptor.require_ready().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn platform_resolver_yields_usable_dir() {
        let dir = dmux_runtime_dir().unwrap();
        assert!(dir.is_dir());
        assert!(dir.ends_with("dmux"));
    }
}
