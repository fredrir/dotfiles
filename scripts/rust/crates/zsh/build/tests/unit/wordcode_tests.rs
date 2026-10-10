use std::time::{Duration, SystemTime};

use super::*;

fn header(magic: [u8; 4], version: &str) -> Vec<u8> {
    let mut bytes = magic.to_vec();
    bytes.extend([1, 0x24, 0xa0, 0x0a]);
    bytes.extend(version.as_bytes());
    bytes.extend([0; 12]);
    bytes
}

fn touch(path: &Path, time: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

#[test]
fn the_version_is_read_from_either_byte_order() {
    let dir = tempfile::tempdir().unwrap();
    let wordcode = dir.path().join("a.zwc");
    for magic in [[7, 6, 5, 4], [4, 5, 6, 7]] {
        fs::write(&wordcode, header(magic, "5.9")).unwrap();
        assert_eq!(version(&wordcode).as_deref(), Some("5.9"));
    }
    fs::write(&wordcode, header([0, 0, 0, 0], "5.9")).unwrap();
    assert_eq!(version(&wordcode), None);
    fs::write(&wordcode, [7, 6, 5, 4, 1]).unwrap();
    assert_eq!(version(&wordcode), None);
}

#[test]
fn compiled_wordcode_ends_up_newer_than_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("a.zsh");
    fs::write(&source, "print a\n").unwrap();
    touch(&source, SystemTime::now() + Duration::from_secs(60));
    let zsh = dir.path().join("zsh");
    testkit::executable(
        &zsh,
        "#!/bin/sh\nprintf wordcode > \"$4\"\nchmod 444 \"$4\"\ntouch -t 200001010000 \"$4\"\n",
    );
    compile(&zsh, &source).unwrap();
    let wordcode = path(&source);
    assert_eq!(fs::read_to_string(&wordcode).unwrap(), "wordcode");
    assert!(modified(&wordcode).unwrap() > modified(&source).unwrap());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);

    testkit::executable(&zsh, "#!/bin/sh\necho 'no luck' >&2\nexit 1\n");
    let error = compile(&zsh, &source).unwrap_err();
    assert!(error.ends_with(": no luck"), "{error}");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
}

#[test]
fn wordcode_is_current_when_newer_and_from_the_same_version() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("a.zsh");
    let wordcode = path(&source);
    assert_eq!(wordcode, dir.path().join("a.zsh.zwc"));
    fs::write(&source, "print a\n").unwrap();
    assert!(!current(&source, &wordcode, "5.9"));

    fs::write(&wordcode, header([7, 6, 5, 4], "5.9")).unwrap();
    let now = SystemTime::now();
    touch(&source, now);
    touch(&wordcode, now + Duration::from_millis(1));
    assert!(current(&source, &wordcode, "5.9"));
    assert!(!current(&source, &wordcode, "5.9.1"));

    touch(&wordcode, now);
    assert!(!current(&source, &wordcode, "5.9"));
}
