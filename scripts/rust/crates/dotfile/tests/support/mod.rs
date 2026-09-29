#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const HOST: &str = "machine";
pub const VAULT: &str = "Test";
pub const ITEM: &str = "MACHINE_SOPS_AGE_KEY_FILE";

/// A fake `op` and `op-bridge` first on PATH, with items kept as JSON in a temporary store.
pub struct OnePassword {
    pub bin: PathBuf,
    pub store: PathBuf,
    pub log: PathBuf,
}

impl OnePassword {
    pub fn install(directory: &Path) -> Self {
        let bin = directory.join("fake-bin");
        let store = directory.join("fake-1password");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&store).unwrap();
        let log = directory.join("fake-op.log");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fake-op"),
            bin.join("op"),
        )
        .unwrap();
        fs::write(
            bin.join("op-bridge"),
            format!(
                "#!/bin/sh\necho \"bridge $*\" >> '{}'\nexit \"${{FAKE_OP_BRIDGE_EXIT:-0}}\"\n",
                log.display()
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["op", "op-bridge"] {
                fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        Self { bin, store, log }
    }

    pub fn path(&self) -> OsString {
        self.path_with(&[])
    }

    pub fn path_with(&self, first: &[&Path]) -> OsString {
        let mut entries: Vec<PathBuf> = first.iter().map(|path| path.to_path_buf()).collect();
        entries.push(self.bin.clone());
        entries.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        std::env::join_paths(entries).unwrap()
    }

    pub fn environment(&self) -> Vec<(&'static str, OsString)> {
        vec![
            ("PATH", self.path()),
            ("FAKE_OP_STORE", self.store.clone().into_os_string()),
            ("FAKE_OP_LOG", self.log.clone().into_os_string()),
            ("SYSINFO_HOST", HOST.into()),
        ]
    }

    pub fn configure(&self, command: &mut Command) {
        command.envs(self.environment());
    }

    pub fn field(&self, field: &str) -> Option<String> {
        let text = fs::read_to_string(self.store.join(VAULT).join(format!("{ITEM}.json"))).ok()?;
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        document["fields"]
            .as_array()?
            .iter()
            .find(|entry| entry["id"] == field)
            .and_then(|entry| entry["value"].as_str())
            .map(str::to_string)
    }

    pub fn seed(&self, secret: &str, public: &str) {
        fs::create_dir_all(self.store.join(VAULT)).unwrap();
        fs::write(
            self.store.join(VAULT).join(format!("{ITEM}.json")),
            serde_json::json!({
                "title": ITEM,
                "category": "API_CREDENTIAL",
                "fields": [
                    {"id": "username", "type": "STRING", "value": public},
                    {"id": "credential", "type": "CONCEALED", "value": secret},
                ],
            })
            .to_string(),
        )
        .unwrap();
    }

    /// The item's current secret, written to a file sops can use directly.
    pub fn identity_file(&self, destination: &Path) -> PathBuf {
        fs::write(destination, self.field("credential").unwrap()).unwrap();
        destination.to_path_buf()
    }

    pub fn calls(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

/// Names this machine in config/hosts.dotfile and points it at the fake item.
pub fn declare_host(root: &Path) {
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/hosts.dotfile"),
        format!("{HOST} {{\n  hostnames = {HOST}\n}}\n"),
    )
    .unwrap();
    fs::write(
        root.join("config/keys.dotfile"),
        format!("identities {{\n  {HOST} = op://{VAULT}/{ITEM}\n}}\n"),
    )
    .unwrap();
}
