//! Secure cross-platform runtime-directory resolver (plan §10.1).
//!
//! All sockets, action tokens, descriptors, and kernel-lock files live under
//! one verified per-user runtime directory. On Linux that is
//! `<base>/dmux` where `<base>` is `$XDG_RUNTIME_DIR` when the login path
//! exported one and the DERIVED `/run/user/<geteuid()>` when it did not (see
//! `linux_base_dir` for the precedence table and why the derivation is not a
//! guess); on macOS `<_CS_DARWIN_USER_TEMP_DIR>/dmux` via `confstr(3)` —
//! never a launchd-exported `XDG_RUNTIME_DIR`, never a blindly trusted
//! `$TMPDIR`, and never a `/run/user` fallback (macOS has no such tree).
//! Persistent registry/snapshots never live here.
//!
//! One thing sits above the platform resolution: the owner-side test seam
//! `DMUX_RUNTIME_DIR` (ADR 009 §6). [`dmux_runtime_dir`] returns it verbatim
//! when it is exported, so a test suite can point every socket, token,
//! descriptor and kernel lock of every process it spawns — the CLI, the
//! `_agent`/`_attach` endpoints, the bootstrap helper, scratch mux servers —
//! at a scratch directory through one variable, and never takes a kernel
//! lock in the directory the live service is using (ADR 012 §3.2).
//! [`platform_runtime_dir`] is the seam-blind resolution, exposed only so
//! the isolation guard can name the directory the seam must keep the suite
//! out of.
//!
//! This resolver performs the metadata-level checks (ownership, mode,
//! symlink rejection). Descriptor-relative no-follow opens for individual
//! endpoints arrive with the P5 runtime broker; callers must still create
//! entries 0600 and revalidate after exec (plan §10.1).
//!
//! Root-owned (plan §19).

use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

const SUBDIR: &str = "dmux";

/// The owner-side test seam: when exported non-empty, its value IS the
/// runtime directory (see [`runtime_dir_seam`] for the exact rule).
pub const RUNTIME_DIR_SEAM_ENV: &str = "DMUX_RUNTIME_DIR";

/// The dmux runtime directory: the [`RUNTIME_DIR_SEAM_ENV`] seam when the
/// owner exported one, otherwise the verified platform directory
/// ([`platform_runtime_dir`]), created 0700 if missing.
///
/// Every production path that needs the runtime directory — kernel-lock
/// directories (`OperationEnv::production`, `RegistryConfig::production`),
/// the managed-mux socket and descriptor, the bridge key, the bootstrap
/// FIFO directory — resolves through this one function, so honouring the
/// seam here is what makes `DMUX_RUNTIME_DIR` mean the same thing to every
/// process a test spawns. The bootstrap helper binary applies the identical
/// rule in its own resolver (`_pane-bootstrap`'s `resolve_runtime_dir`).
pub fn dmux_runtime_dir() -> io::Result<PathBuf> {
    match runtime_dir_seam()? {
        Some(dir) => Ok(dir),
        None => platform_runtime_dir(),
    }
}

/// The verified platform runtime directory, ignoring the seam: the
/// `confstr`/`XDG_RUNTIME_DIR`/derived `/run/user/<euid>` resolution the
/// module doc describes, created 0700 if missing.
///
/// This is the directory the live service is using. Production code must
/// not call it — it would re-create the very bypass `dmux_runtime_dir`
/// exists to close — it is public so the isolation guard
/// (`tests/runtime_dir_seam.rs`, `tests/run-isolated.sh`) can snapshot the
/// directory a seam-exporting suite run must leave unchanged.
pub fn platform_runtime_dir() -> io::Result<PathBuf> {
    secured_runtime_subdir(&platform_base_dir()?)
}

/// What the [`RUNTIME_DIR_SEAM_ENV`] seam says: `Ok(None)` when it is unset
/// or empty (production — nothing ever sets it except a test, the GUI
/// launcher and the mux start script, which export the verified platform
/// value itself so their descendants agree with them), `Ok(Some(dir))` when
/// it names an absolute path, and an error when it is set to a relative one.
///
/// The value is used verbatim — not created, not canonicalised, not held to
/// the ownership/mode checks the platform path gets — because the owner of
/// the process owns the directory; this is exactly the trust the hidden
/// `--data-dir`/`--lock-dir` flags already extend, and `_agent`/`_attach`
/// take those from the same command line a remote peer writes. A peer who
/// can export this variable has a shell as this user and needs no seam; an
/// sshd forced command drops client environment altogether. Keep the path
/// SHORT: scratch mux servers bind `<dir>/wez-dmux.sock`, and `sun_path` is
/// 104 bytes on macOS (108 on Linux) — a deep scratch path fails every
/// socket-binding test with "File name too long".
pub fn runtime_dir_seam() -> io::Result<Option<PathBuf>> {
    runtime_dir_seam_from(std::env::var_os(RUNTIME_DIR_SEAM_ENV).as_deref())
}

/// `value` is passed in (rather than read here) so the rule is unit-testable
/// without touching the process environment.
fn runtime_dir_seam_from(value: Option<&std::ffi::OsStr>) -> io::Result<Option<PathBuf>> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let dir = PathBuf::from(value);
    if !dir.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{RUNTIME_DIR_SEAM_ENV} must be an absolute path"),
        ));
    }
    Ok(Some(dir))
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
    let xdg = std::env::var_os("XDG_RUNTIME_DIR");
    linux_base_dir(xdg.as_deref(), Path::new(LINUX_RUNTIME_ROOT), unsafe {
        libc::geteuid()
    })
}

/// Where `pam_systemd`/`systemd-logind` places per-user runtime directories.
/// A literal, never assembled from the environment.
#[cfg(any(target_os = "linux", test))]
const LINUX_RUNTIME_ROOT: &str = "/run/user";

/// Linux base-directory precedence, in order (plan §10.1):
///
/// 1. `DMUX_RUNTIME_DIR` — the explicit owner-side TEST seam. It wins over
///    everything, and it is consulted one level ABOVE this resolver, in
///    [`dmux_runtime_dir`] (and identically in `_pane-bootstrap`'s
///    `resolve_runtime_dir`), used verbatim because the test owns the
///    directory. It used to be read only by the helper, on the argument that
///    `dmux _agent` runs as `ssh <route> dmux _agent …` and the peer chooses
///    that command line; but that same command line already carries the
///    hidden `--data-dir`/`--lock-dir` seams `_agent` honours verbatim, so
///    the variable adds nothing a peer did not have — while every owner-side
///    process that ignored it took its kernel locks in the live service's
///    directory (ADR 012 §3.2). See [`runtime_dir_seam`].
/// 2. `$XDG_RUNTIME_DIR`, when the login path exported a non-empty value.
///    Used as given (and then held to the same checks as every other base by
///    `secured_runtime_subdir`). An explicitly set value is never *replaced*
///    by the derivation below — a set-but-unusable value fails closed, so a
///    caller cannot steer resolution by pointing it somewhere that fails.
/// 3. The derived `/run/user/<geteuid()>`, when `$XDG_RUNTIME_DIR` is absent
///    or empty. This is the case Tailscale SSH creates: `tailscaled`
///    terminates the session itself, so no `pam_systemd` runs, no
///    `XDG_SESSION_ID`/`XDG_RUNTIME_DIR` is exported — while logind's
///    directory for this very uid is sitting there, holding the socket the
///    agent needs. Requiring one PAM stack to have run is the wrong
///    contract; the path is a function of the euid, which is not
///    caller-controllable, and it is verified before it is trusted.
///
/// `xdg` is passed in (rather than read here) so the whole table above,
/// including its rejections, is unit-testable off a Linux host.
#[cfg(any(target_os = "linux", test))]
fn linux_base_dir(
    xdg: Option<&std::ffi::OsStr>,
    runtime_root: &Path,
    euid: u32,
) -> io::Result<PathBuf> {
    match xdg.filter(|value| !value.is_empty()) {
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "XDG_RUNTIME_DIR must be an absolute path",
                ));
            }
            Ok(dir)
        }
        None => derived_linux_base_dir(runtime_root, euid),
    }
}

/// `/run/user/<euid>`, verified to the crate's private-directory standard
/// (`recovery::validate_private_directory`): it must exist, be a real
/// directory rather than a symlink, be owned by this euid, and be exactly
/// mode 0700. Any failure is the typed error — there is no further fallback,
/// because the only thing left to fall back to would be a guess.
#[cfg(any(target_os = "linux", test))]
fn derived_linux_base_dir(runtime_root: &Path, euid: u32) -> io::Result<PathBuf> {
    let dir = runtime_root.join(euid.to_string());
    let meta = fs::symlink_metadata(&dir).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "XDG_RUNTIME_DIR is not set and the derived runtime dir {} does not exist",
                    dir.display()
                ),
            )
        } else {
            e
        }
    })?;
    if !meta.is_dir() {
        return Err(reject(&dir, "is not a directory (or is a symlink)"));
    }
    if meta.uid() != euid {
        return Err(reject(&dir, "is not owned by the current user"));
    }
    if meta.mode() & 0o7777 != 0o700 {
        return Err(reject(&dir, "must be mode 0700"));
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
pub const WEZ_SOCKET_FILE: &str = "wez-dmux.sock";
const MAX_EXACT_JSON_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WezMuxDescriptor {
    pub descriptor_version: u32,
    pub state: String,
    pub epoch: String,
    pub pid: u32,
    pub socket: String,
    pub start_token: String,
    #[serde(default)]
    pub boot_id: Option<String>,
    #[serde(default)]
    pub socket_dev: Option<u64>,
    #[serde(default)]
    pub socket_ino: Option<u64>,
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
    pub sentinel_fallback: Option<bool>,
    #[serde(default)]
    pub recovery_manifest_id: Option<String>,
    #[serde(default)]
    pub written_by: Option<String>,
    #[serde(default)]
    pub written_at: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWezServiceIdentity {
    pub pid: u32,
    pub start_token: String,
    pub boot_id: String,
    pub socket_dev: u64,
    pub socket_ino: u64,
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
        let boot_nonce = self.boot_nonce.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor has no boot_nonce",
            )
        })?;
        parse_descriptor_uuid("boot_nonce", boot_nonce)?;
        if self.sentinel_fallback != Some(false) || self.error.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux ready descriptor has no exact native sentinel witness",
            ));
        }
        exact_json_id("sentinel_window_id", self.sentinel_window_id)?;
        exact_json_id("sentinel_tab_id", self.sentinel_tab_id)?;
        exact_json_id("sentinel_pane_id", self.sentinel_pane_id)?;
        if let Some(generation) = self.recovery_generation.as_deref() {
            parse_descriptor_uuid("recovery_generation", generation)?;
        }
        if self
            .recovery_manifest_id
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 256)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor has invalid recovery_manifest_id",
            ));
        }
        self.require_native_identity_fields()?;
        Ok(())
    }

    fn require_native_identity_fields(&self) -> io::Result<(String, u64, u64)> {
        let boot_id = self.boot_id.clone().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor has no boot_id",
            )
        })?;
        validate_boot_token(&boot_id)?;
        validate_process_start_token(&self.start_token)?;
        let dev = exact_positive_json_id("socket_dev", self.socket_dev)?;
        let ino = exact_positive_json_id("socket_ino", self.socket_ino)?;
        if self.written_by.as_deref() != Some("mux-startup") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "managed Wez mux descriptor was not written by mux-startup",
            ));
        }
        validate_written_at(self.written_at.as_deref())?;
        Ok((boot_id, dev, ino))
    }

    fn require_recovery_authority_fields(&self) -> io::Result<(String, u64, u64)> {
        if self.state != "recovering"
            || self.error.is_some()
            || self.sentinel_fallback != Some(false)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "managed Wez mux descriptor has not entered exact recovering authority",
            ));
        }
        exact_json_id("sentinel_window_id", self.sentinel_window_id)?;
        exact_json_id("sentinel_tab_id", self.sentinel_tab_id)?;
        exact_json_id("sentinel_pane_id", self.sentinel_pane_id)?;
        self.require_native_identity_fields()
    }
}

fn exact_json_id(field: &str, value: Option<u64>) -> io::Result<u64> {
    match value {
        Some(value @ 0..=MAX_EXACT_JSON_INTEGER) => Ok(value),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed Wez mux descriptor {field} is not an exact JSON integer"),
        )),
    }
}

fn exact_positive_json_id(field: &str, value: Option<u64>) -> io::Result<u64> {
    match value {
        Some(value @ 1..=MAX_EXACT_JSON_INTEGER) => Ok(value),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed Wez mux descriptor {field} is not a positive exact JSON integer"),
        )),
    }
}

fn parse_descriptor_uuid(field: &str, value: &str) -> io::Result<uuid::Uuid> {
    let parsed = value.parse::<uuid::Uuid>().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed Wez mux descriptor {field}: {e}"),
        )
    })?;
    if parsed.to_string() != value {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("managed Wez mux descriptor {field} is not a canonical lowercase UUID"),
        ));
    }
    Ok(parsed)
}

fn validate_written_at(value: Option<&str>) -> io::Result<()> {
    let Some(value) = value else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Wez mux descriptor has no written_at",
        ));
    };
    let bytes = value.as_bytes();
    let punctuation = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    let valid = bytes.len() == 20
        && punctuation
            .iter()
            .all(|(index, expected)| bytes[*index] == *expected)
        && bytes.iter().enumerate().all(|(index, byte)| {
            punctuation.iter().any(|(at, _)| *at == index) || byte.is_ascii_digit()
        });
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Wez mux descriptor has invalid written_at",
        ))
    }
}

/// Read the managed-mux descriptor from an explicitly resolved runtime
/// directory. `Ok(None)` when the service has never written one
/// (stopped/uninstalled). `runtime_dir` is re-verified here (owner, mode,
/// no symlink) before the descriptor is opened, so callers pass the
/// directory they resolved — [`dmux_runtime_dir`] for the local service, the
/// peer's seam for a remote agent, a scratch directory in tests — and never
/// a path an environment variable was allowed to choose behind their back.
/// There is deliberately no fixed-runtime wrapper: the directory a caller
/// read from is part of what it later compares against (ADR 012 WS-E.3 row 14).
pub fn read_wez_descriptor_in(runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>> {
    let directory = open_descriptor_runtime(runtime_dir)?;
    read_wez_descriptor_from_directory(runtime_dir, &directory)
}

fn open_descriptor_runtime(runtime_dir: &Path) -> io::Result<File> {
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(runtime_dir)?;
    validate_runtime_binding(runtime_dir, &directory)?;
    Ok(directory)
}

fn validate_runtime_binding(runtime_dir: &Path, directory: &File) -> io::Result<()> {
    let euid = unsafe { libc::geteuid() };
    let held = directory.metadata()?;
    let current = fs::symlink_metadata(runtime_dir)?;
    if !held.is_dir()
        || held.uid() != euid
        || held.mode() & 0o7777 != 0o700
        || !current.is_dir()
        || current.uid() != euid
        || current.mode() & 0o7777 != 0o700
        || held.dev() != current.dev()
        || held.ino() != current.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "descriptor runtime {} is not the same held current-user-owned mode-0700 directory",
                runtime_dir.display()
            ),
        ));
    }
    Ok(())
}

fn read_wez_descriptor_from_directory(
    runtime_dir: &Path,
    directory: &File,
) -> io::Result<Option<WezMuxDescriptor>> {
    let path = runtime_dir.join(WEZ_DESCRIPTOR_FILE);
    let euid = unsafe { libc::geteuid() };
    validate_runtime_binding(runtime_dir, directory)?;

    let name = CString::new(WEZ_DESCRIPTOR_FILE).expect("fixed descriptor name has no NUL");
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(error);
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let before = file.metadata()?;
    validate_descriptor_metadata(&path, &before, euid)?;
    if before.len() > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("descriptor {} exceeds 64 KiB", path.display()),
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    Read::by_ref(&mut file)
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("descriptor {} exceeds 64 KiB", path.display()),
        ));
    }
    let after = file.metadata()?;
    validate_descriptor_metadata(&path, &after, euid)?;
    let current_path = fs::symlink_metadata(&path)?;
    validate_descriptor_metadata(&path, &current_path, euid)?;
    if descriptor_fingerprint(&before) != descriptor_fingerprint(&after)
        || before.dev() != current_path.dev()
        || before.ino() != current_path.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("descriptor {} changed while it was read", path.display()),
        ));
    }
    validate_runtime_binding(runtime_dir, directory)?;
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

fn validate_descriptor_metadata(path: &Path, metadata: &fs::Metadata, euid: u32) -> io::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "descriptor {} must be a current-user-owned single-link non-symlink file with mode 0600",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn descriptor_fingerprint(
    metadata: &fs::Metadata,
) -> (u64, u64, u64, u32, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.nlink(),
        metadata.mode(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

/// Read and prove the ready service descriptor in an explicitly resolved
/// runtime directory against the current OS boot, process incarnation,
/// socket inode/device, current-UID socket peer, and (when supplied) the
/// registry backend instance and epoch. Callers receive descriptor bytes only
/// after every identity check succeeds.
///
/// Every caller resolves `runtime_dir` itself — [`dmux_runtime_dir`] for the
/// local service, the peer's seam for a remote agent, a scratch directory in
/// tests — and passes the instance and epoch it already holds from the
/// registry. Both are mandatory: there is no form of this read that skips a
/// comparison, because a descriptor nothing verified against the registry is
/// exactly the laundering ADR 012 closes (every production caller sources
/// both from an `ok_or_else` on the registry row). There is deliberately no
/// fixed-runtime wrapper either (WS-E.3 row 14).
pub fn read_verified_ready_wez_descriptor_in(
    runtime_dir: &Path,
    expected_instance: uuid::Uuid,
    expected_epoch: uuid::Uuid,
) -> io::Result<Option<WezMuxDescriptor>> {
    let directory = open_descriptor_runtime(runtime_dir)?;
    let Some(descriptor) = read_wez_descriptor_from_directory(runtime_dir, &directory)? else {
        return Ok(None);
    };
    descriptor.require_ready()?;
    let instance = parse_descriptor_uuid(
        "backend_instance_uid",
        descriptor
            .backend_instance_uid
            .as_deref()
            .expect("require_ready checked backend_instance_uid"),
    )?;
    let epoch = parse_descriptor_uuid("epoch", &descriptor.epoch)?;
    if expected_instance != instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor names a different backend instance",
        ));
    }
    if expected_epoch != epoch {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor names a different server epoch",
        ));
    }
    verify_live_service_identity(runtime_dir, &directory, &descriptor)?;
    Ok(Some(descriptor))
}

/// Prove that a recovery coordinator is the direct child of the fixed live
/// mux service incarnation before it can acquire a fence or publish registry
/// identity. Descriptor bytes are only claims; every OS-verifiable field is
/// independently re-read here, including the fixed socket's peer PID.
pub fn verify_recovery_service_authority(
    runtime_dir: &Path,
    backend_instance: uuid::Uuid,
    server_epoch: uuid::Uuid,
    server_pid: i64,
    server_start_token: &str,
) -> io::Result<VerifiedWezServiceIdentity> {
    let fixed_runtime = dmux_runtime_dir()?;
    if runtime_dir != fixed_runtime {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "recovery runtime {} is not fixed service runtime {}",
                runtime_dir.display(),
                fixed_runtime.display()
            ),
        ));
    }
    let parent_pid = unsafe { libc::getppid() };
    verify_recovery_service_authority_in(
        runtime_dir,
        backend_instance,
        server_epoch,
        server_pid,
        server_start_token,
        parent_pid,
    )
}

/// Prove that the snapshot helper is a direct child of the exact fixed ready
/// mux incarnation. This is the persistent-manifest counterpart to recovery
/// authorization and is repeated under the backend fence by its caller.
pub fn verify_snapshot_service_authority(
    runtime_dir: &Path,
    backend_instance: uuid::Uuid,
    server_epoch: uuid::Uuid,
    server_pid: i64,
    server_start_token: &str,
) -> io::Result<VerifiedWezServiceIdentity> {
    let fixed_runtime = dmux_runtime_dir()?;
    if runtime_dir != fixed_runtime {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "snapshot helper does not name the fixed service runtime",
        ));
    }
    let parent_pid = unsafe { libc::getppid() };
    let pid = u32::try_from(server_pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "snapshot server PID is not a positive u32",
        )
    })?;
    if pid == 0 || parent_pid <= 0 || u32::try_from(parent_pid).ok() != Some(pid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "snapshot helper is not the exact mux-server child",
        ));
    }
    let directory = open_descriptor_runtime(runtime_dir)?;
    let descriptor =
        read_wez_descriptor_from_directory(runtime_dir, &directory)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "managed Wez mux descriptor is absent",
            )
        })?;
    descriptor.require_ready()?;
    let descriptor_instance = parse_descriptor_uuid(
        "backend_instance_uid",
        descriptor
            .backend_instance_uid
            .as_deref()
            .expect("require_ready checked backend_instance_uid"),
    )?;
    let descriptor_epoch = parse_descriptor_uuid("epoch", &descriptor.epoch)?;
    if descriptor_instance != backend_instance
        || descriptor_epoch != server_epoch
        || descriptor.pid != pid
        || descriptor.start_token != server_start_token
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "snapshot arguments do not match the ready service incarnation",
        ));
    }
    verify_live_service_identity(runtime_dir, &directory, &descriptor)
}

fn verify_recovery_service_authority_in(
    runtime_dir: &Path,
    backend_instance: uuid::Uuid,
    server_epoch: uuid::Uuid,
    server_pid: i64,
    server_start_token: &str,
    parent_pid: libc::pid_t,
) -> io::Result<VerifiedWezServiceIdentity> {
    let directory = open_descriptor_runtime(runtime_dir)?;
    let descriptor =
        read_wez_descriptor_from_directory(runtime_dir, &directory)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "managed Wez mux descriptor is absent",
            )
        })?;
    if descriptor.state != "recovering" {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "recovery coordinator requires recovering descriptor, found {}",
                descriptor.state
            ),
        ));
    }
    let descriptor_epoch = parse_descriptor_uuid("epoch", &descriptor.epoch)?;
    let descriptor_instance = parse_descriptor_uuid(
        "backend_instance_uid",
        descriptor
            .backend_instance_uid
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing backend UID"))?,
    )?;
    if descriptor_epoch != server_epoch || descriptor_instance != backend_instance {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "recovery arguments do not match the service descriptor incarnation",
        ));
    }
    let pid = u32::try_from(server_pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "recovery server PID is not a positive u32",
        )
    })?;
    if pid == 0
        || descriptor.pid != pid
        || parent_pid <= 0
        || u32::try_from(parent_pid).ok() != Some(pid)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "recovery coordinator is not the exact mux-server child",
        ));
    }
    if descriptor.start_token != server_start_token {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "recovery start token does not match the service descriptor",
        ));
    }
    descriptor.require_recovery_authority_fields()?;
    let verified = verify_live_service_identity(runtime_dir, &directory, &descriptor)?;
    if verified.pid != pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor socket peer differs from recovery server PID",
        ));
    }
    Ok(verified)
}

fn verify_live_service_identity(
    runtime_dir: &Path,
    directory: &File,
    descriptor: &WezMuxDescriptor,
) -> io::Result<VerifiedWezServiceIdentity> {
    let pid = descriptor.pid;
    if pid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Wez descriptor has zero process PID",
        ));
    }
    let (boot_id, descriptor_dev, descriptor_ino) = descriptor.require_native_identity_fields()?;
    let current_boot = current_boot_token()?;
    let current_start = process_start_token(pid)?;
    if boot_id != current_boot || descriptor.start_token != current_start {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor process incarnation is no longer live",
        ));
    }
    let fixed_socket = runtime_dir.join(WEZ_SOCKET_FILE);
    if Path::new(&descriptor.socket) != fixed_socket {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor does not name the fixed service socket",
        ));
    }
    validate_runtime_binding(runtime_dir, directory)?;
    let (socket_dev, socket_ino, peer_pid) = socket_identity(&fixed_socket, directory)?;
    validate_runtime_binding(runtime_dir, directory)?;
    if socket_dev != descriptor_dev || socket_ino != descriptor_ino || peer_pid != pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez descriptor socket identity or peer PID changed",
        ));
    }
    Ok(VerifiedWezServiceIdentity {
        pid,
        start_token: current_start,
        boot_id: current_boot,
        socket_dev,
        socket_ino,
    })
}

fn socket_identity(path: &Path, directory: &File) -> io::Result<(u64, u64, u32)> {
    let metadata = fs::symlink_metadata(path)?;
    let held = socket_stat_at(directory)?;
    let euid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_socket()
        || metadata.uid() != euid
        || metadata.mode() & 0o077 != 0
        || held.st_mode & libc::S_IFMT != libc::S_IFSOCK
        || held.st_uid != euid
        || held.st_mode & 0o077 != 0
        || u64::try_from(held.st_dev).ok() != Some(metadata.dev())
        || u64::try_from(held.st_ino).ok() != Some(metadata.ino())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "managed Wez socket {} is not a private current-user-owned socket",
                path.display()
            ),
        ));
    }
    let dev = exact_positive_json_id("live socket dev", Some(metadata.dev()))?;
    let ino = exact_positive_json_id("live socket ino", Some(metadata.ino()))?;
    let stream = UnixStream::connect(path)?;
    let after = fs::symlink_metadata(path)?;
    let held_after = socket_stat_at(directory)?;
    if !after.file_type().is_socket()
        || after.uid() != euid
        || after.mode() & 0o077 != 0
        || after.dev() != metadata.dev()
        || after.ino() != metadata.ino()
        || u64::try_from(held_after.st_dev).ok() != Some(metadata.dev())
        || u64::try_from(held_after.st_ino).ok() != Some(metadata.ino())
    {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "managed Wez socket changed while its peer was verified",
        ));
    }
    let (peer_pid, peer_uid) = socket_peer_identity(&stream)?;
    if peer_uid != euid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed Wez socket peer is not the current user",
        ));
    }
    Ok((dev, ino, peer_pid))
}

fn socket_stat_at(directory: &File) -> io::Result<libc::stat> {
    let name = CString::new(WEZ_SOCKET_FILE).expect("fixed socket name has no NUL");
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(stat)
}

#[cfg(target_os = "macos")]
fn socket_peer_identity(stream: &UnixStream) -> io::Result<(u32, u32)> {
    use std::os::fd::AsRawFd;

    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if rc != 0 || pid <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((pid as u32, uid))
}

#[cfg(target_os = "linux")]
fn socket_peer_identity(stream: &UnixStream) -> io::Result<(u32, u32)> {
    use std::os::fd::AsRawFd;

    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 || cred.pid <= 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((cred.pid as u32, cred.uid))
}

#[cfg(target_os = "linux")]
fn current_boot_token() -> io::Result<String> {
    let value = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let value = value.trim();
    let uuid = parse_descriptor_uuid("Linux boot_id", value)?;
    if uuid.to_string() != value {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Linux boot_id is not canonical lowercase UUID",
        ));
    }
    Ok(format!("linux:{value}"))
}

/// OS-verifiable current boot witness used by maintained-fork integration
/// fixtures and service producers. It returns the exact descriptor format.
#[doc(hidden)]
pub fn current_boot_id() -> io::Result<String> {
    current_boot_token()
}

#[cfg(target_os = "macos")]
fn current_boot_token() -> io::Result<String> {
    use std::ffi::CString;

    let name = CString::new("kern.boottime").unwrap();
    let mut value: libc::timeval = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::timeval>();
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut libc::timeval).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if length != std::mem::size_of::<libc::timeval>()
        || value.tv_sec <= 0
        || !(0..=999_999).contains(&value.tv_usec)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "kern.boottime returned an invalid value",
        ));
    }
    Ok(format!("macos:{}:{}", value.tv_sec, value.tv_usec))
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_token(pid: u32) -> io::Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat.rfind(')').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "process stat has no comm terminator",
        )
    })?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    let ticks = fields.get(19).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "process stat omits start ticks")
    })?;
    let ticks = ticks.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "process start ticks are not an integer",
        )
    })?;
    if ticks == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "process start ticks are zero",
        ));
    }
    Ok(format!("linux:{ticks}"))
}

#[cfg(target_os = "macos")]
pub(crate) fn process_start_token(pid: u32) -> io::Result<String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size as libc::c_int,
        )
    };
    if read != size as libc::c_int
        || info.pbi_pid != pid
        || info.pbi_uid != unsafe { libc::geteuid() }
        || info.pbi_start_tvsec == 0
        || info.pbi_start_tvusec > 999_999
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("cannot verify process {pid} start identity"),
        ));
    }
    Ok(format!(
        "macos:{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

/// OS-verifiable process start witness in the exact descriptor format.
#[doc(hidden)]
pub fn process_start_token_for_pid(pid: u32) -> io::Result<String> {
    process_start_token(pid)
}

fn validate_boot_token(value: &str) -> io::Result<()> {
    if let Some(uuid) = value.strip_prefix("linux:") {
        let parsed = parse_descriptor_uuid("boot_id", uuid)?;
        if parsed.to_string() == uuid {
            return Ok(());
        }
    } else if let Some(rest) = value.strip_prefix("macos:") {
        let Some((seconds, micros)) = rest.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid macOS boot_id",
            ));
        };
        if seconds.parse::<u64>().is_ok_and(|value| value > 0)
            && micros.parse::<u32>().is_ok_and(|value| value <= 999_999)
        {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "managed Wez mux descriptor has invalid boot_id",
    ))
}

fn validate_process_start_token(value: &str) -> io::Result<()> {
    let valid = value
        .strip_prefix("linux:")
        .is_some_and(|ticks| ticks.parse::<u64>().is_ok_and(|value| value > 0))
        || value.strip_prefix("macos:").is_some_and(|rest| {
            rest.split_once(':').is_some_and(|(seconds, micros)| {
                seconds.parse::<u64>().is_ok_and(|value| value > 0)
                    && micros.parse::<u32>().is_ok_and(|value| value <= 999_999)
            })
        });
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "managed Wez mux descriptor has invalid process start_token",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    use super::*;

    fn scratch_base() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[cfg(target_os = "macos")]
    fn sample_process_witness() -> (&'static str, &'static str) {
        ("macos:1:0", "macos:2:0")
    }

    #[cfg(target_os = "linux")]
    fn sample_process_witness() -> (&'static str, &'static str) {
        ("linux:00000000-0000-4000-8000-000000000001", "linux:2")
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
    fn held_runtime_binding_rejects_path_replacement() {
        let base = scratch_base();
        let original = base.path().to_path_buf();
        let moved = original.with_extension("moved");
        let directory = open_descriptor_runtime(&original).unwrap();
        fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &original).unwrap();
        let error = validate_runtime_binding(&original, &directory).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_file(&original).unwrap();
        fs::rename(&moved, &original).unwrap();
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
        let (boot_id, start_token) = sample_process_witness();
        fs::write(
            &path,
            serde_json::json!({
                "descriptor_version": 1,
                "state": "ready",
                "epoch": epoch,
                "pid": 42,
                "socket": "/tmp/dmux-test.sock",
                "start_token": start_token,
                "boot_id": boot_id,
                "socket_dev": 1,
                "socket_ino": 2,
                "backend_instance_uid": instance,
                "boot_nonce": uuid::Uuid::new_v4(),
                "sentinel_window_id": 0,
                "sentinel_tab_id": 0,
                "sentinel_pane_id": 0,
                "sentinel_fallback": false,
                "written_by": "mux-startup",
                "written_at": "2026-08-17T00:00:00Z",
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
    fn verified_ready_reader_binds_descriptor_to_live_fixed_socket_and_process() {
        let base = scratch_base();
        let socket = base.path().join(WEZ_SOCKET_FILE);
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let socket_metadata = fs::symlink_metadata(&socket).unwrap();
        let epoch = uuid::Uuid::new_v4();
        let instance = uuid::Uuid::new_v4();
        let path = base.path().join(WEZ_DESCRIPTOR_FILE);
        fs::write(
            &path,
            serde_json::json!({
                "descriptor_version": 1,
                "state": "ready",
                "epoch": epoch,
                "pid": std::process::id(),
                "socket": socket,
                "start_token": process_start_token(std::process::id()).unwrap(),
                "boot_id": current_boot_token().unwrap(),
                "socket_dev": socket_metadata.dev(),
                "socket_ino": socket_metadata.ino(),
                "backend_instance_uid": instance,
                "boot_nonce": uuid::Uuid::new_v4(),
                "sentinel_window_id": 0,
                "sentinel_tab_id": 0,
                "sentinel_pane_id": 0,
                "sentinel_fallback": false,
                "written_by": "mux-startup",
                "written_at": "2026-08-17T00:00:00Z",
            })
            .to_string(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let descriptor = read_verified_ready_wez_descriptor_in(base.path(), instance, epoch)
            .unwrap()
            .unwrap();
        assert_eq!(descriptor.pid, std::process::id());

        let error =
            read_verified_ready_wez_descriptor_in(base.path(), instance, uuid::Uuid::new_v4())
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn starting_descriptor_is_not_ready() {
        let (boot_id, start_token) = sample_process_witness();
        let descriptor = WezMuxDescriptor {
            descriptor_version: 1,
            state: "starting".into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            pid: 42,
            socket: "/tmp/dmux-test.sock".into(),
            start_token: start_token.into(),
            boot_id: Some(boot_id.into()),
            socket_dev: None,
            socket_ino: None,
            boot_nonce: None,
            backend_instance_uid: Some(uuid::Uuid::new_v4().to_string()),
            recovery_generation: Some("generation-1".into()),
            sentinel_window_id: None,
            sentinel_tab_id: None,
            sentinel_pane_id: None,
            sentinel_fallback: None,
            recovery_manifest_id: None,
            written_by: Some("mux-startup".into()),
            written_at: Some("2026-08-17T00:00:00Z".into()),
            error: None,
        };
        assert_eq!(
            descriptor.require_ready().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );

        let mut starting_with_sentinel = descriptor.clone();
        starting_with_sentinel.socket_dev = Some(1);
        starting_with_sentinel.socket_ino = Some(2);
        starting_with_sentinel.sentinel_window_id = Some(0);
        starting_with_sentinel.sentinel_tab_id = Some(0);
        starting_with_sentinel.sentinel_pane_id = Some(0);
        starting_with_sentinel.sentinel_fallback = Some(false);
        assert_eq!(
            starting_with_sentinel
                .require_recovery_authority_fields()
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied,
            "a complete sentinel must not grant recovery authority while state=starting"
        );
        starting_with_sentinel.state = "recovering".into();
        starting_with_sentinel
            .require_recovery_authority_fields()
            .unwrap();
    }

    // ---------------------------------------------------------------------
    // Linux base-dir precedence table (`linux_base_dir`). Every input the
    // resolver has — the `XDG_RUNTIME_DIR` value, the runtime root and the
    // euid — is injected, so the whole table including its rejections is
    // provable on any host; only the two facts that are genuinely about the
    // running system (the root literal, and the real `/run/user/<euid>`) are
    // asserted separately, the second of them Linux-only.

    fn current_euid() -> u32 {
        unsafe { libc::geteuid() }
    }

    /// A scratch stand-in for `/run/user` holding a well-formed `<euid>`.
    fn scratch_runtime_root(euid: u32) -> tempfile::TempDir {
        let root = scratch_base();
        let dir = root.path().join(euid.to_string());
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    #[test]
    fn derivation_targets_run_user_euid() {
        assert_eq!(Path::new(LINUX_RUNTIME_ROOT), Path::new("/run/user"));
        let euid = current_euid();
        let root = scratch_runtime_root(euid);
        assert_eq!(
            derived_linux_base_dir(root.path(), euid).unwrap(),
            root.path().join(euid.to_string()),
            "the derived dir is <root>/<euid>, never a caller-supplied name"
        );
    }

    #[test]
    fn an_exported_xdg_runtime_dir_wins_over_the_derivation() {
        let euid = current_euid();
        let root = scratch_runtime_root(euid);
        let exported = scratch_base();
        assert_eq!(
            linux_base_dir(Some(exported.path().as_os_str()), root.path(), euid).unwrap(),
            exported.path(),
        );
    }

    #[test]
    fn a_set_but_unusable_xdg_runtime_dir_fails_closed() {
        let euid = current_euid();
        let root = scratch_runtime_root(euid);
        // Relative: refused outright, never quietly replaced by the
        // derivation (that would let a caller *choose* the derived dir).
        let error = linux_base_dir(
            Some(std::ffi::OsStr::new("relative/dir")),
            root.path(),
            euid,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{error}");
        // Absolute but absent: returned as given, so the caller's value is
        // what fails the checks downstream rather than being swapped out.
        let absent = root.path().join("absent");
        assert_eq!(
            linux_base_dir(Some(absent.as_os_str()), root.path(), euid).unwrap(),
            absent,
        );
        assert!(secured_runtime_subdir(&absent).is_err());
    }

    #[test]
    fn an_absent_or_empty_xdg_runtime_dir_derives() {
        let euid = current_euid();
        let root = scratch_runtime_root(euid);
        let derived = root.path().join(euid.to_string());
        assert_eq!(linux_base_dir(None, root.path(), euid).unwrap(), derived);
        assert_eq!(
            linux_base_dir(Some(std::ffi::OsStr::new("")), root.path(), euid).unwrap(),
            derived,
            "an exported-but-empty value is the same 'no session' case",
        );
        // And the derived base is usable as a base: `<derived>/dmux` 0700.
        assert_eq!(
            secured_runtime_subdir(&derived).unwrap(),
            derived.join("dmux")
        );
    }

    #[test]
    fn the_derived_dir_must_exist() {
        let euid = current_euid();
        let root = scratch_base(); // no `<euid>` entry at all
        let error = derived_linux_base_dir(root.path(), euid).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
        assert!(
            error.to_string().contains(&euid.to_string()),
            "the message names the derived path: {error}"
        );
    }

    #[test]
    fn the_derived_dir_must_be_owned_by_this_euid() {
        // The check is `metadata.uid() != euid`; a scratch dir this test owns
        // is by construction NOT owned by any other uid, so the foreign-owner
        // case is driven from the other side — ask for the runtime dir of a
        // uid that is not us.
        let other = current_euid() + 1;
        let root = scratch_runtime_root(other);
        let error = derived_linux_base_dir(root.path(), other).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(
            error.to_string().contains("not owned by the current user"),
            "{error}"
        );
    }

    #[test]
    fn the_derived_dir_must_be_mode_0700() {
        let euid = current_euid();
        for mode in [0o755, 0o750, 0o701, 0o600, 0o1700] {
            let root = scratch_runtime_root(euid);
            let dir = root.path().join(euid.to_string());
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
            let error = derived_linux_base_dir(root.path(), euid).unwrap_err();
            assert_eq!(
                error.kind(),
                io::ErrorKind::PermissionDenied,
                "mode {mode:o}: {error}"
            );
            // Restore so the TempDir can clean itself up.
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[test]
    fn the_derived_dir_must_not_be_a_symlink_or_a_file() {
        let euid = current_euid();

        let root = scratch_base();
        let real = scratch_base();
        std::os::unix::fs::symlink(real.path(), root.path().join(euid.to_string())).unwrap();
        let error = derived_linux_base_dir(root.path(), euid).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(
            error.to_string().contains("is not a directory"),
            "a symlink to a perfectly good 0700 dir is still refused: {error}"
        );

        let root = scratch_base();
        fs::write(root.path().join(euid.to_string()), b"nope").unwrap();
        assert_eq!(
            derived_linux_base_dir(root.path(), euid)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    /// The Linux half of `platform_resolver_yields_usable_dir`: on a host
    /// that has `/run/user/<euid>`, the derivation resolves it; on one that
    /// does not, it fails closed with the typed error naming it. Nothing in
    /// between — never a guess, never a different tree.
    #[cfg(target_os = "linux")]
    #[test]
    fn production_derivation_resolves_or_fails_closed() {
        let euid = current_euid();
        let expected = Path::new(LINUX_RUNTIME_ROOT).join(euid.to_string());
        match derived_linux_base_dir(Path::new(LINUX_RUNTIME_ROOT), euid) {
            Ok(dir) => assert_eq!(dir, expected),
            Err(error) => assert!(
                error.to_string().contains(&expected.display().to_string()),
                "{error}"
            ),
        }
    }

    /// A session with no `XDG_RUNTIME_DIR` (Tailscale SSH, `su`, a cron-like
    /// context) still resolves the same directory a pam_systemd login would
    /// have exported.
    #[cfg(target_os = "linux")]
    #[test]
    fn platform_resolver_yields_usable_dir_without_a_pam_session() {
        let euid = current_euid();
        let expected = Path::new(LINUX_RUNTIME_ROOT).join(euid.to_string());
        if !expected.is_dir() {
            eprintln!("SKIP: this host has no {}", expected.display());
            return;
        }
        let dir = linux_base_dir(None, Path::new(LINUX_RUNTIME_ROOT), euid).unwrap();
        assert_eq!(dir, expected);
        let dmux = secured_runtime_subdir(&dir).unwrap();
        assert!(dmux.is_dir());
        assert!(dmux.ends_with("dmux"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn platform_resolver_yields_usable_dir() {
        let dir = platform_runtime_dir().unwrap();
        assert!(dir.is_dir());
        assert!(dir.ends_with("dmux"));
        // The public resolver is the platform dir exactly when no seam is
        // exported, and the seam verbatim when one is (the isolated suite
        // runner exports one; bare `cargo test` does not).
        let resolved = dmux_runtime_dir().unwrap();
        match runtime_dir_seam().unwrap() {
            Some(seam) => assert_eq!(resolved, seam),
            None => assert_eq!(resolved, dir),
        }
    }

    #[test]
    fn runtime_dir_seam_is_verbatim_when_absolute_and_absent_when_empty() {
        use std::ffi::OsStr;

        assert_eq!(runtime_dir_seam_from(None).unwrap(), None);
        assert_eq!(runtime_dir_seam_from(Some(OsStr::new(""))).unwrap(), None);
        // Verbatim: not created, not canonicalised — a path that does not
        // exist comes back as given, and so does a trailing slash.
        let missing = "/nonexistent/dmux-seam-e1/never-created";
        assert_eq!(
            runtime_dir_seam_from(Some(OsStr::new(missing))).unwrap(),
            Some(PathBuf::from(missing))
        );
        assert_eq!(
            runtime_dir_seam_from(Some(OsStr::new("/tmp/dmux-seam/"))).unwrap(),
            Some(PathBuf::from("/tmp/dmux-seam/"))
        );
        // A relative value fails closed instead of resolving against the
        // working directory: a test that set it would otherwise scatter
        // lock files through whatever directory it ran from.
        let err = runtime_dir_seam_from(Some(OsStr::new("relative/dir"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("DMUX_RUNTIME_DIR"), "{err}");
    }
}
