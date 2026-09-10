use super::*;

fn source_fixture(root: &Path) -> PathBuf {
    let source = root.join("source");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/report.txt"), b"important payload\n").unwrap();
    fs::write(source.join("excluded.tmp"), b"do not archive").unwrap();
    source
}

fn options() -> ArchiveOptions {
    ArchiveOptions {
        host: "archie".into(),
        job: "documents".into(),
        category: "documents".into(),
        labels: vec!["personal".into()],
        excludes: vec!["*.tmp".into()],
        ..ArchiveOptions::default()
    }
}

#[test]
fn metadata_authentication_requires_the_signing_secret_and_authenticated_eof() {
    let root = tempfile::tempdir().unwrap();
    let (identity, recipient) = generate_identity();
    let key = b"0123456789abcdef0123456789abcdef";
    let change = MetadataChange {
        format_version: 1,
        archive_id: Uuid::new_v4().to_string(),
        host: "archie".into(),
        archive_sha256: "a".repeat(64),
        revision: Uuid::new_v4().to_string(),
        sequence: 1,
        created_at: Utc::now(),
        encrypted: true,
        category: Some("documents".into()),
        add: vec!["personal".into()],
        remove: Vec::new(),
    };
    let valid = root.path().join("valid.metadata");
    write_metadata_change(&valid, &change, std::slice::from_ref(&recipient), key).unwrap();
    assert_eq!(
        read_metadata_change(&valid, std::slice::from_ref(&identity), key)
            .unwrap()
            .revision,
        change.revision
    );
    assert!(read_metadata_change(&valid, &[], key).is_err());
    let forged = root.path().join("forged.metadata");
    write_metadata_change(
        &forged,
        &change,
        &[recipient],
        b"attacker-controlled-forgery-password",
    )
    .unwrap();
    assert!(
        read_metadata_change(&forged, std::slice::from_ref(&identity), key)
            .unwrap_err()
            .to_string()
            .contains("authentication failed")
    );
    let mut bytes = fs::read(&valid).unwrap();
    bytes.pop();
    fs::write(&valid, bytes).unwrap();
    assert!(read_metadata_change(&valid, &[identity], key).is_err());
}

#[test]
fn metadata_records_bound_claimed_document_size_before_allocation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("oversized.metadata");
    let mut file = File::create(&path).unwrap();
    write_sealed(&mut file, METADATA_MAGIC, &[], |writer| {
        writer.write_all(&(8u64 * 1024 * 1024 * 1024).to_le_bytes())?;
        Ok(())
    })
    .unwrap();
    assert!(
        read_metadata_change(&path, &[], b"0123456789abcdef0123456789abcdef")
            .unwrap_err()
            .to_string()
            .contains("size limit")
    );
}

#[test]
fn malicious_age_header_is_bounded_before_recipient_parsing() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("oversized-header.dcloud");
    let (identity, _) = generate_identity();
    let mut file = File::create(&path).unwrap();
    file.write_all(ARCHIVE_MAGIC).unwrap();
    file.write_all(&[1]).unwrap();
    file.write_all(b"age-encryption.org/v1\n-> X25519 ")
        .unwrap();
    file.write_all(&vec![b'A'; MAX_AGE_HEADER_BYTES as usize + 1])
        .unwrap();
    let error = read_embedded_manifest(&path, &[identity]).unwrap_err();
    assert!(
        format!("{error:#}").contains("header exceeds size limit"),
        "{error:#}"
    );
}

#[test]
fn plain_archive_roundtrip_excludes_and_selective_restore() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let manifest = create(&source, &archive, &options()).unwrap();
    assert_eq!(manifest.total_bytes, 18);
    assert_eq!(manifest.entries.len(), 3);
    assert_eq!(
        read_sidecar(&manifest_path(&archive), &[]).unwrap(),
        manifest
    );
    assert_eq!(verify(&archive, &[]).unwrap(), manifest);
    let restored = root.path().join("restored");
    restore(&archive, &restored, &[], &[]).unwrap();
    assert_eq!(
        fs::read(restored.join("nested/report.txt")).unwrap(),
        b"important payload\n"
    );
    assert!(!restored.join("excluded.tmp").exists());
    assert!(restore(&archive, &restored, &[], &[]).is_err());
    let selected = root.path().join("selected");
    restore(
        &archive,
        &selected,
        &[],
        &[PathBuf::from("nested/report.txt")],
    )
    .unwrap();
    assert_eq!(
        fs::read(selected.join("nested/report.txt")).unwrap(),
        b"important payload\n"
    );
    assert!(
        restore(
            &archive,
            &root.path().join("missing"),
            &[],
            &[PathBuf::from("absent")]
        )
        .is_err()
    );
    assert!(create(&source, &archive, &options()).is_err());
}

#[test]
fn age_archive_keeps_manifest_private_and_requires_correct_identity() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let (identity, recipient) = generate_identity();
    let (wrong_identity, _) = generate_identity();
    let options = ArchiveOptions {
        recipients: vec![recipient],
        ..options()
    };
    let manifest = create(&source, &archive, &options).unwrap();
    assert!(manifest.encrypted);
    assert!(
        !String::from_utf8_lossy(&fs::read(manifest_path(&archive)).unwrap())
            .contains("report.txt")
    );
    assert!(read_manifest(&archive, &[]).is_err());
    assert!(verify(&archive, &[wrong_identity]).is_err());
    assert_eq!(
        verify(&archive, std::slice::from_ref(&identity)).unwrap(),
        manifest
    );
    let restored = root.path().join("restored");
    restore(&archive, &restored, &[identity], &[]).unwrap();
    assert_eq!(
        fs::read(restored.join("nested/report.txt")).unwrap(),
        b"important payload\n"
    );
}

#[test]
fn corrupt_archive_never_publishes_restore_and_enforces_limits() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    create(&source, &archive, &options()).unwrap();
    assert!(
        verify_with_limits(
            &archive,
            &[],
            RestoreLimits {
                max_total_bytes: 1,
                max_file_bytes: 1
            }
        )
        .is_err()
    );
    let mut bytes = fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&archive, bytes).unwrap();
    let restored = root.path().join("restored");
    assert!(restore(&archive, &restored, &[], &[]).is_err());
    assert!(!restored.exists());
}

#[test]
fn traversal_manifest_and_selection_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let mut manifest = create(&source, &archive, &options()).unwrap();
    assert!(
        restore(
            &archive,
            &root.path().join("output"),
            &[],
            &[PathBuf::from("../outside")]
        )
        .is_err()
    );
    manifest.entries[0].path = "../outside".into();
    let mut sidecar = File::create(manifest_path(&archive)).unwrap();
    write_sealed(&mut sidecar, MANIFEST_MAGIC, &[], |writer| {
        write_document(writer, &serde_json::to_vec(&manifest)?)
    })
    .unwrap();
    assert!(read_manifest(&archive, &[]).is_err());
    assert!(!root.path().join("outside").exists());
}

#[test]
fn single_file_archive_and_logical_id_are_supported() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("file.txt");
    fs::write(&source, b"one file").unwrap();
    let archive = root.path().join("file.dcloud");
    let id = Uuid::new_v4().to_string();
    let options = ArchiveOptions {
        id: Some(id.clone()),
        ..options()
    };
    let manifest = create(&source, &archive, &options).unwrap();
    assert_eq!(manifest.id, id);
    assert!(!manifest.source_is_dir);
    let restored = root.path().join("restored");
    restore(&archive, &restored, &[], &[]).unwrap();
    assert_eq!(fs::read(restored.join("file.txt")).unwrap(), b"one file");
}

#[cfg(unix)]
#[test]
fn safe_symlinks_and_metadata_roundtrip_but_escaping_links_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let file = source.join("nested/report.txt");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o750)).unwrap();
    let modified = FileTime::from_unix_time(1_700_000_000, 123_456_789);
    filetime::set_file_mtime(&file, modified).unwrap();
    symlink("nested/report.txt", source.join("report-link")).unwrap();
    let archive = root.path().join("object.dcloud");
    create(&source, &archive, &options()).unwrap();
    let restored = root.path().join("restored");
    restore(&archive, &restored, &[], &[]).unwrap();
    assert_eq!(
        fs::read_link(restored.join("report-link")).unwrap(),
        PathBuf::from("nested/report.txt")
    );
    let metadata = fs::metadata(restored.join("nested/report.txt")).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o750);
    assert_eq!(FileTime::from_last_modification_time(&metadata), modified);
    symlink("../../outside", source.join("unsafe-link")).unwrap();
    assert!(create(&source, &root.path().join("unsafe.dcloud"), &options()).is_err());
}

#[test]
fn encrypted_truncation_is_rejected_after_checksum_is_recomputed() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let (identity, recipient) = generate_identity();
    let options = ArchiveOptions {
        recipients: vec![recipient],
        ..options()
    };
    let mut manifest = create(&source, &archive, &options).unwrap();
    let file = OpenOptions::new().write(true).open(&archive).unwrap();
    file.set_len(file.metadata().unwrap().len() - 1).unwrap();
    manifest.archive_sha256 = Some(hash_file(&archive).unwrap());
    let mut sidecar = File::create(manifest_path(&archive)).unwrap();
    write_sealed(
        &mut sidecar,
        MANIFEST_MAGIC,
        &options.recipients,
        |writer| write_document(writer, &serde_json::to_vec(&manifest)?),
    )
    .unwrap();
    assert!(restore(&archive, &root.path().join("restored"), &[identity], &[]).is_err());
    assert!(!root.path().join("restored").exists());
}

#[test]
fn plaintext_sidecar_cannot_claim_to_be_encrypted() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let mut manifest = create(&source, &archive, &options()).unwrap();
    manifest.encrypted = true;
    let mut sidecar = File::create(manifest_path(&archive)).unwrap();
    write_sealed(&mut sidecar, MANIFEST_MAGIC, &[], |writer| {
        write_document(writer, &serde_json::to_vec(&manifest)?)
    })
    .unwrap();
    assert!(read_manifest(&archive, &[]).is_err());
}

#[test]
fn output_limit_leaves_no_published_partial_files() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    assert!(
        create(
            &source,
            &archive,
            &ArchiveOptions {
                max_output_bytes: 64,
                ..options()
            }
        )
        .is_err()
    );
    assert!(!archive.exists());
    assert!(!manifest_path(&archive).exists());
}

#[cfg(unix)]
#[test]
fn symlink_parent_traversal_cannot_escape_through_another_symlink() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    std::os::unix::fs::symlink(".", source.join("back-to-root")).unwrap();
    std::os::unix::fs::symlink("back-to-root/../outside", source.join("escape")).unwrap();
    assert!(create(&source, &root.path().join("object.dcloud"), &options()).is_err());
}

#[test]
fn tar_extensions_are_bounded_before_payload_allocation() {
    let root = tempfile::tempdir().unwrap();
    let source = source_fixture(root.path());
    let archive = root.path().join("object.dcloud");
    let mut manifest = create(&source, &archive, &options()).unwrap();
    manifest.archive_sha256 = None;
    let mut header = tar::Header::new_gnu();
    header.set_path("././@LongLink").unwrap();
    header.set_entry_type(tar::EntryType::GNULongName);
    header.set_size(8 * 1024 * 1024 * 1024);
    header.set_cksum();
    let mut output = File::create(&archive).unwrap();
    write_sealed(&mut output, ARCHIVE_MAGIC, &[], |writer| {
        write_document(writer, &serde_json::to_vec(&manifest)?)?;
        let mut compressor = zstd::stream::write::Encoder::new(writer, 3)?;
        compressor.write_all(header.as_bytes())?;
        compressor.write_all(&[0u8; 1024])?;
        compressor.finish()?;
        Ok(())
    })
    .unwrap();
    manifest.archive_sha256 = Some(hash_file(&archive).unwrap());
    let mut sidecar = File::create(manifest_path(&archive)).unwrap();
    write_sealed(&mut sidecar, MANIFEST_MAGIC, &[], |writer| {
        write_document(writer, &serde_json::to_vec(&manifest)?)
    })
    .unwrap();
    assert!(
        verify(&archive, &[])
            .unwrap_err()
            .to_string()
            .contains("path extension exceeds")
    );
}

#[test]
fn long_paths_and_embedded_manifest_recovery_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let long_name = "a".repeat(150);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join(&long_name), "long path payload").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&long_name, source.join("link")).unwrap();
    let archive = root.path().join("object.dcloud");
    let manifest = create(&source, &archive, &options()).unwrap();
    fs::remove_file(manifest_path(&archive)).unwrap();
    assert_eq!(
        read_embedded_manifest(&archive, &[]).unwrap().id,
        manifest.id
    );
    let restored = root.path().join("restored");
    restore(&archive, &restored, &[], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(restored.join(&long_name)).unwrap(),
        "long path payload"
    );
}
