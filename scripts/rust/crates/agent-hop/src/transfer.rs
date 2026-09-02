use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Command, Output};

use hostkit::Host;
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::remote::shell_quote;

const ATTEMPTS: usize = 3;
const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
const TRANSPORT: &str = "ssh -o ConnectTimeout=8 -o LogLevel=ERROR";

pub struct Snapshot {
    file: NamedTempFile,
}

impl Snapshot {
    pub fn create(source: &Path) -> Result<Snapshot, String> {
        let mut last_error = String::new();
        for _ in 0..ATTEMPTS {
            match snapshot_once(source) {
                Ok(file) => return Ok(Snapshot { file }),
                Err(error) => last_error = error,
            }
        }
        Err(format!(
            "could not make a valid snapshot of {} after {ATTEMPTS} attempts: {last_error}",
            source.display()
        ))
    }

    pub fn path(&self) -> &Path {
        self.file.path()
    }

    pub(crate) fn from_temporary(mut file: NamedTempFile) -> Result<Snapshot, String> {
        finish_snapshot(&mut file)?;
        Ok(Snapshot { file })
    }
}

pub fn copy_transcript(peer: Host, source: &Path, destination: &Path) -> Result<(), String> {
    run_rsync(
        &file_arguments(peer, source, destination)?,
        "session transcript",
    )
}

pub fn copy_companion(peer: Host, source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("could not inspect {}: {error}", source.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "the session attachments are not a safe directory: {}",
            source.display()
        ));
    }
    run_rsync(
        &directory_arguments(peer, source, destination)?,
        "session attachments",
    )
}

pub fn file_arguments(
    peer: Host,
    source: &Path,
    destination: &Path,
) -> Result<Vec<OsString>, String> {
    Ok(arguments(
        source.as_os_str().to_os_string(),
        remote_target(peer, destination, false)?,
    ))
}

pub fn directory_arguments(
    peer: Host,
    source: &Path,
    destination: &Path,
) -> Result<Vec<OsString>, String> {
    Ok(arguments(
        with_slash(source.as_os_str()),
        remote_target(peer, destination, true)?,
    ))
}

#[cfg(test)]
pub fn valid_jsonl(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    let mut count = 0;
    lines.all(|line| {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            return false;
        }
        count += 1;
        serde_json::from_slice::<Value>(line).is_ok_and(|value| value.is_object())
    }) && count > 0
}

fn snapshot_once(source: &Path) -> Result<NamedTempFile, String> {
    let input = File::open(source)
        .map_err(|error| format!("could not read {}: {error}", source.display()))?;
    let mut snapshot = NamedTempFile::new()
        .map_err(|error| format!("could not create a temporary snapshot: {error}"))?;
    std::io::copy(&mut BufReader::new(input), snapshot.as_file_mut())
        .map_err(|error| format!("could not copy {}: {error}", source.display()))?;
    finish_snapshot(&mut snapshot)?;
    Ok(snapshot)
}

fn finish_snapshot(snapshot: &mut NamedTempFile) -> Result<(), String> {
    snapshot
        .as_file_mut()
        .flush()
        .map_err(|error| format!("could not finish the session snapshot: {error}"))?;
    snapshot
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not inspect the session snapshot: {error}"))?;
    let mut reader = BufReader::new(snapshot.as_file_mut());
    let mut line = Vec::new();
    let mut records = 0usize;
    loop {
        line.clear();
        let bytes = reader
            .by_ref()
            .take((MAX_RECORD_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("could not inspect the session snapshot: {error}"))?;
        if bytes == 0 {
            break;
        }
        if bytes > MAX_RECORD_BYTES {
            return Err("the copied transcript contains an unreasonably large record".to_string());
        }
        let record = line.strip_suffix(b"\n").unwrap_or(&line);
        let record = record.strip_suffix(b"\r").unwrap_or(record);
        if record.is_empty()
            || !serde_json::from_slice::<Value>(record).is_ok_and(|value| value.is_object())
        {
            return Err("the copied transcript is not valid JSONL".to_string());
        }
        records += 1;
    }
    if records == 0 {
        return Err("the copied transcript is not valid JSONL".to_string());
    }
    drop(reader);
    snapshot
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not rewind the session snapshot: {error}"))?;
    Ok(())
}

fn arguments(source: OsString, destination: OsString) -> Vec<OsString> {
    vec![
        OsString::from("-a"),
        OsString::from("-e"),
        OsString::from(TRANSPORT),
        OsString::from("--"),
        source,
        destination,
    ]
}

fn remote_target(peer: Host, path: &Path, directory: bool) -> Result<OsString, String> {
    let path = path
        .to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))?;
    let slash = if directory { "/" } else { "" };
    Ok(OsString::from(format!(
        "{}:{}{slash}",
        peer.name(),
        shell_quote(path)
    )))
}

fn with_slash(path: &OsStr) -> OsString {
    let mut value = path.to_os_string();
    value.push(std::path::MAIN_SEPARATOR_STR);
    value
}

fn run_rsync(arguments: &[OsString], what: &str) -> Result<(), String> {
    let output = Command::new("rsync")
        .args(arguments)
        .output()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "rsync is required".to_string()
            } else {
                format!("rsync: {error}")
            }
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("could not copy {what}: {}", output_error(&output)))
    }
}

fn output_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Some(reason) = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("rsync error:"))
        .or_else(|| stderr.lines().map(str::trim).find(|line| !line.is_empty()))
    {
        reason.to_string()
    } else {
        match output.status.code() {
            Some(code) => format!("rsync exited with status {code}"),
            None => "rsync was interrupted".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn jsonl_requires_one_complete_object_on_every_line() {
        assert!(valid_jsonl(
            br#"{"type":"one"}
{"type":"two","payload":{"id":1}}
"#
        ));
        assert!(valid_jsonl(br#"{"type":"one"}"#));
        assert!(!valid_jsonl(b""));
        assert!(!valid_jsonl(b"\n"));
        assert!(!valid_jsonl(b"{}\n\n"));
        assert!(!valid_jsonl(b"{}\n{\n"));
        assert!(!valid_jsonl(b"[]\n"));
        assert!(!valid_jsonl(&[0xff, b'\n']));
    }

    #[test]
    fn a_snapshot_is_an_exact_independent_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("session.jsonl");
        let body = b"{\"type\":\"one\"}\n{\"type\":\"two\"}\n";
        fs::write(&source, body).unwrap();
        let snapshot = Snapshot::create(&source).unwrap();
        assert_eq!(fs::read(snapshot.path()).unwrap(), body);
        fs::write(&source, "{\"changed\":true}\n").unwrap();
        assert_eq!(fs::read(snapshot.path()).unwrap(), body);
    }

    #[test]
    fn invalid_jsonl_is_rejected_after_the_retry_budget() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("session.jsonl");
        fs::write(&source, "{\"unfinished\":").unwrap();
        let error = Snapshot::create(&source).err().unwrap();
        assert!(error.contains("after 3 attempts"));
        assert!(error.contains("not valid JSONL"));
    }

    #[test]
    fn file_transfer_arguments_keep_each_path_one_process_argument() {
        assert_eq!(
            file_arguments(
                Host::Archie,
                Path::new("/tmp/agent hop.snapshot"),
                Path::new("/home/fred rir/.codex/a'b.jsonl"),
            )
            .unwrap(),
            [
                "-a",
                "-e",
                "ssh -o ConnectTimeout=8 -o LogLevel=ERROR",
                "--",
                "/tmp/agent hop.snapshot",
                "archie:'/home/fred rir/.codex/a'\\''b.jsonl'",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn attachment_transfer_copies_directory_contents() {
        assert_eq!(
            directory_arguments(
                Host::Macie,
                Path::new("/tmp/attachments"),
                Path::new("/Users/fredrir/.claude/project/id"),
            )
            .unwrap(),
            [
                "-a",
                "-e",
                "ssh -o ConnectTimeout=8 -o LogLevel=ERROR",
                "--",
                "/tmp/attachments/",
                "macie:'/Users/fredrir/.claude/project/id'/",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn transcript_and_companion_builders_are_independent_operations() {
        let transcript = file_arguments(
            Host::Archie,
            Path::new("/tmp/session.jsonl"),
            Path::new("/home/fredrir/session.jsonl"),
        )
        .unwrap();
        let companion = directory_arguments(
            Host::Archie,
            Path::new("/tmp/session"),
            Path::new("/home/fredrir/session"),
        )
        .unwrap();
        assert_eq!(transcript[4], "/tmp/session.jsonl");
        assert_eq!(transcript[5], "archie:'/home/fredrir/session.jsonl'");
        assert_eq!(companion[4], "/tmp/session/");
        assert_eq!(companion[5], "archie:'/home/fredrir/session'/");
    }

    #[test]
    fn a_disappearing_companion_is_reported_before_transfer() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let error =
            copy_companion(Host::Archie, &missing, Path::new("/home/fredrir/session")).unwrap_err();
        assert!(error.contains("could not inspect"));
    }

    #[test]
    fn snapshot_paths_can_be_passed_directly_to_the_file_builder() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("session.jsonl");
        fs::write(&source, "{}\n").unwrap();
        let snapshot = Snapshot::create(&source).unwrap();
        let arguments = file_arguments(
            Host::Archie,
            snapshot.path(),
            Path::new("/home/fredrir/session.jsonl"),
        )
        .unwrap();
        assert_eq!(arguments[4], snapshot.path().as_os_str());
    }

    #[test]
    fn remote_targets_follow_the_named_peer() {
        let local = PathBuf::from("/tmp/session");
        let destination = PathBuf::from("/Users/fredrir/session");
        assert_eq!(
            file_arguments(Host::Macie, &local, &destination).unwrap()[5],
            "macie:'/Users/fredrir/session'"
        );
        assert_eq!(
            file_arguments(Host::Archie, &local, &destination).unwrap()[5],
            "archie:'/Users/fredrir/session'"
        );
    }
}
