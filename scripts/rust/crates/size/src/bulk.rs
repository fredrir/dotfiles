//! Bulk directory reads, on the platform that offers them.
//!
//! `getattrlistbulk` hands back the names, types, permissions and sizes of a
//! whole batch of entries per call, where `read_dir` plus a `stat` each spends
//! a syscall per entry. That difference does not parallelise away: concurrent
//! metadata lookups inside one directory serialise in the kernel, so throwing
//! threads at the `stat` walk buys wall clock at several times the CPU. On a
//! 285k-entry tree this is around 2,900 calls rather than 285k, and one thread
//! finishes roughly four times sooner than the pooled `stat` walk did.
//!
//! Not every filesystem implements it. A directory that will not answer falls
//! back to the portable walk on its own, so an unsupporting mount costs only
//! its own subtree.

use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::FromRawFd;
use std::path::Path;

use rayon::prelude::*;

use super::{Link, Measure, Options, Row, Walked, count_lines_in};

// `enum vtype`, from <sys/vnode.h>. Anything else is an "other" to us.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

/// Big enough that a directory of any ordinary size comes back in a handful of
/// calls, small enough to sit on the heap once per walk.
const BATCH: usize = 256 * 1024;

/// An owned directory descriptor.
struct Dir(libc::c_int);

impl Dir {
    fn open(path: &Path) -> Option<Dir> {
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `path` is a valid NUL-terminated string for the call.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        (fd >= 0).then_some(Dir(fd))
    }

    /// A subdirectory, opened relative to this one: no path to rebuild, and
    /// `O_NOFOLLOW` so a symlink swapped in mid-walk cannot redirect us.
    fn open_child(&self, name: &CStr) -> Option<Dir> {
        // SAFETY: `self.0` is an open directory and `name` is NUL-terminated.
        let fd = unsafe {
            libc::openat(
                self.0,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        (fd >= 0).then_some(Dir(fd))
    }

    /// What this directory says about itself once it is open. A mount point is
    /// an ordinary directory of the parent filesystem in the batch reply that
    /// names it — the reply describes the entry, not what is mounted over it —
    /// so both the device `-x` compares and the blocks the directory itself
    /// costs have to be read from the open descriptor.
    fn status(&self) -> Option<Status> {
        // SAFETY: `stat` is plain data, `self.0` is an open descriptor, and
        // the fields are only read once the call reports success.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let read = unsafe { libc::fstat(self.0, &mut stat) };
        (read == 0).then(|| Status {
            device: stat.st_dev as u64,
            bytes: stat.st_blocks as u64 * 512,
        })
    }

    fn open_file(&self, name: &CStr) -> Option<File> {
        // SAFETY: as above; the descriptor is handed straight to `File`, which
        // takes ownership of closing it.
        let fd = unsafe {
            libc::openat(
                self.0,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        (fd >= 0).then(|| unsafe { File::from_raw_fd(fd) })
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        // SAFETY: we own this descriptor and are the last to touch it.
        unsafe { libc::close(self.0) };
    }
}

/// What an open directory says about itself, as against what the entry naming
/// it said.
struct Status {
    device: u64,
    bytes: u64,
}

/// What one entry of a batch says about itself.
struct Entry<'a> {
    name: &'a CStr,
    devid: i32,
    objtype: u32,
    accessmask: u32,
    fileid: u64,
    linkcount: u32,
    /// The space it occupies, and what every mode but `-A` measures.
    allocated: u64,
    /// The length it reports, which `-A` measures and which a sparse file can
    /// inflate without limit.
    bytes: u64,
}

fn attributes() -> libc::attrlist {
    // SAFETY: `attrlist` is plain data; an all-zero one is a valid empty list.
    let mut list: libc::attrlist = unsafe { std::mem::zeroed() };
    list.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    list.commonattr = libc::ATTR_CMN_RETURNED_ATTRS
        | libc::ATTR_CMN_NAME
        | libc::ATTR_CMN_DEVID
        | libc::ATTR_CMN_OBJTYPE
        | libc::ATTR_CMN_ACCESSMASK
        | libc::ATTR_CMN_FILEID;
    // Both sizes on every entry: the extra sixteen bytes a reply carries are
    // nothing beside a second pass for whichever one the flags turn out to
    // want.
    list.dirattr = libc::ATTR_DIR_ALLOCSIZE;
    list.fileattr =
        libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
    list
}

/// Walks one entry of the reply. Fields arrive by attribute group — common,
/// then directory, then file — and by bit within a group, each present only if
/// `ATTR_CMN_RETURNED_ATTRS` says so, and packed without padding — hence the
/// unaligned reads.
///
/// # Safety
///
/// `entry` must point at the start of an entry the kernel wrote.
unsafe fn decode<'a>(entry: *const u8) -> (Entry<'a>, usize) {
    let mut field = entry;
    let length = unsafe { (field as *const u32).read_unaligned() } as usize;
    field = unsafe { field.add(size_of::<u32>()) };
    let returned = unsafe { (field as *const libc::attribute_set_t).read_unaligned() };
    field = unsafe { field.add(size_of::<libc::attribute_set_t>()) };

    let mut name = c"";
    if returned.commonattr & libc::ATTR_CMN_NAME != 0 {
        let reference = unsafe { (field as *const libc::attrreference_t).read_unaligned() };
        let start = unsafe { field.offset(reference.attr_dataoffset as isize) };
        name = unsafe { CStr::from_ptr(start as *const libc::c_char) };
        field = unsafe { field.add(size_of::<libc::attrreference_t>()) };
    }
    let mut devid = 0;
    if returned.commonattr & libc::ATTR_CMN_DEVID != 0 {
        devid = unsafe { (field as *const i32).read_unaligned() };
        field = unsafe { field.add(size_of::<i32>()) };
    }
    let mut objtype = 0;
    if returned.commonattr & libc::ATTR_CMN_OBJTYPE != 0 {
        objtype = unsafe { (field as *const u32).read_unaligned() };
        field = unsafe { field.add(size_of::<u32>()) };
    }
    let mut accessmask = 0;
    if returned.commonattr & libc::ATTR_CMN_ACCESSMASK != 0 {
        accessmask = unsafe { (field as *const u32).read_unaligned() };
        field = unsafe { field.add(size_of::<u32>()) };
    }
    let mut fileid = 0;
    if returned.commonattr & libc::ATTR_CMN_FILEID != 0 {
        fileid = unsafe { (field as *const u64).read_unaligned() };
        field = unsafe { field.add(size_of::<u64>()) };
    }
    // Directory attributes precede file ones, and only one of the two groups
    // ever arrives for a given entry.
    let mut allocated = None;
    if returned.dirattr & libc::ATTR_DIR_ALLOCSIZE != 0 {
        allocated = Some(unsafe { (field as *const i64).read_unaligned() } as u64);
        field = unsafe { field.add(size_of::<i64>()) };
    }
    let mut linkcount = 0;
    if returned.fileattr & libc::ATTR_FILE_LINKCOUNT != 0 {
        linkcount = unsafe { (field as *const u32).read_unaligned() };
        field = unsafe { field.add(size_of::<u32>()) };
    }
    if returned.fileattr & libc::ATTR_FILE_ALLOCSIZE != 0 {
        allocated = Some(unsafe { (field as *const i64).read_unaligned() } as u64);
        field = unsafe { field.add(size_of::<i64>()) };
    }
    // A directory's own contents are what we sum underneath it, so no data
    // length arrives for one.
    let mut bytes = 0;
    if returned.fileattr & libc::ATTR_FILE_DATALENGTH != 0 {
        bytes = unsafe { (field as *const i64).read_unaligned() } as u64;
    }

    let entry = Entry {
        name,
        devid,
        objtype,
        accessmask,
        fileid,
        linkcount,
        // A filesystem that answers bulk requests without offering an
        // allocated size leaves the length as the only thing to go on.
        allocated: allocated.unwrap_or(bytes),
        bytes,
    };
    (entry, length)
}

/// The whole tree under `target`, or `None` if this filesystem will not answer
/// bulk requests and the portable walk should take it instead.
pub fn walk(options: &Options, target: &Path) -> Option<Walked> {
    let dir = Dir::open(target)?;
    read(options, &dir, target, Path::new(""), 0)
}

/// A subdirectory, held back until the whole directory has been read: its own
/// name is all we need to open it again relative to the descriptor we have.
struct Child {
    name: CString,
    visible: bool,
    depth: usize,
    /// Its own blocks as the batch reported them — what a directory that will
    /// not open leaves us to go on.
    bytes: u64,
}

fn read(
    options: &Options,
    dir: &Dir,
    full: &Path,
    relative: &Path,
    depth: usize,
) -> Option<Walked> {
    let mut list = attributes();
    let mut buffer = vec![0u8; BATCH];
    let mut walked = Walked::default();
    let mut children: Vec<Child> = Vec::new();

    loop {
        // SAFETY: `dir` is open, and the kernel writes at most `BATCH` bytes
        // into a buffer of exactly that size.
        let count = unsafe {
            libc::getattrlistbulk(
                dir.0,
                &mut list as *mut _ as *mut libc::c_void,
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
                0,
            )
        };
        if count == 0 {
            break;
        }
        if count < 0 {
            return None;
        }

        // Decode the batch first. The names borrow the buffer, so everything
        // below must finish with them before the next call overwrites it.
        let mut batch = Vec::with_capacity(count as usize);
        let mut entry = buffer.as_ptr();
        for _ in 0..count {
            // SAFETY: the kernel wrote `count` consecutive entries, and each
            // step advances by the length that entry declared.
            let (found, length) = unsafe { decode(entry) };
            entry = unsafe { entry.add(length) };
            batch.push(found);
        }

        // Settle what is out before anything reads a file: an entry that is
        // out stays out of the line counting too. The device each entry
        // reports came back with the batch, so `-x` costs nothing extra here.
        let skipped: Vec<bool> = if options.ignore.is_empty() && options.device.is_none() {
            Vec::new()
        } else {
            batch
                .iter()
                .map(|found| {
                    if options.skips_device(found.devid as u64) {
                        return true;
                    }
                    if options.ignore.skips_name(&found.name.to_string_lossy()) {
                        return true;
                    }
                    options.ignore.wants_paths() && {
                        let name = Path::new(OsStr::from_bytes(found.name.to_bytes()));
                        options
                            .ignore
                            .skips_path(&relative.join(name).to_string_lossy())
                    }
                })
                .collect()
        };
        let is_skipped = |index: usize| skipped.get(index).copied().unwrap_or(false);
        // Built once per batch rather than per link: a tree of build output can
        // be almost all hardlinks, and each one would otherwise cost a PathBuf
        // and a String to say where it sits.
        let here = relative.to_string_lossy();

        // Counting lines means reading every file through, and file reads do
        // parallelise — unlike the metadata lookups the walk itself makes.
        let counted: Vec<Option<u64>> = if options.lines {
            batch
                .par_iter()
                .enumerate()
                .map(|(index, found)| {
                    if found.objtype != VREG || is_skipped(index) {
                        return Some(0);
                    }
                    dir.open_file(found.name)
                        .and_then(|mut file| count_lines_in(&mut file))
                })
                .collect()
        } else {
            Vec::new()
        };

        for (index, found) in batch.iter().enumerate() {
            if is_skipped(index) {
                continue;
            }
            // The portable walk's `hidden`, without rendering a String first.
            let is_hidden = found.name.to_bytes().first() == Some(&b'.');
            let visible = depth < options.display_depth && (options.all || !is_hidden);

            if found.objtype == VDIR {
                children.push(Child {
                    name: found.name.to_owned(),
                    visible,
                    // A hidden directory keeps its children out of the listing
                    // even with depth left over, so spend the budget here.
                    depth: if visible {
                        depth + 1
                    } else {
                        options.display_depth
                    },
                    bytes: found.allocated,
                });
                continue;
            }

            let found_measure = measure_entry(found, options, counted.get(index).copied());
            walked.measure.add(found_measure);
            let name = Path::new(OsStr::from_bytes(found.name.to_bytes()));
            if found.linkcount > 1 {
                let leaf = name.to_string_lossy();
                walked.links.push(Link {
                    file: (found.devid as u64, found.fileid),
                    path: if here.is_empty() {
                        leaf.into_owned()
                    } else {
                        format!("{here}/{leaf}")
                    },
                    bytes: found_measure.bytes,
                    lines: found_measure.lines,
                });
            }
            if visible {
                walked.rows.push(Row {
                    name: relative.join(name).to_string_lossy().to_string(),
                    kind: kind_of(found.objtype),
                    executable: found.accessmask & 0o111 != 0,
                    measure: found_measure,
                });
            }
        }
    }

    // Subdirectories in parallel: separate directories do not contend the way
    // repeated lookups inside one of them do, and in line mode this is what
    // keeps every core busy reading files.
    let below: Vec<Walked> = children
        .par_iter()
        .map(|child| {
            let name = Path::new(OsStr::from_bytes(child.name.to_bytes()));
            let child_relative = relative.join(name);
            let child_full = full.join(name);
            let handle = dir.open_child(&child.name);
            // One `fstat` on a descriptor we already hold, and only per
            // directory: no path to resolve, so none of the contention that
            // made a `stat` per entry worth avoiding in the first place.
            let opened = handle.as_ref().and_then(Dir::status);
            if let Some(status) = &opened
                && options.skips_device(status.device)
            {
                return Walked::default();
            }
            let mut child_walked = match handle {
                Some(handle) => {
                    read(options, &handle, &child_full, &child_relative, child.depth)
                        // This directory will not answer; the portable walk can.
                        .unwrap_or_else(|| {
                            super::walk_directory(
                                options,
                                &child_full,
                                &child_relative,
                                child.depth,
                            )
                        })
                }
                None => Walked::unreadable(),
            };
            if !options.apparent {
                child_walked.measure.bytes += opened.map_or(child.bytes, |status| status.bytes);
            }
            if child.visible {
                let measure = child_walked.measure;
                child_walked.rows.push(Row {
                    name: child_relative.to_string_lossy().to_string(),
                    kind: "directory",
                    executable: false,
                    measure,
                });
            }
            child_walked
        })
        .collect();
    for child_walked in below {
        walked.absorb(child_walked);
    }

    Some(walked)
}

/// `counted` is `None` outside line mode, and `Some(None)` for a file that
/// would not open — the same thing the portable walk calls unreadable.
fn measure_entry(entry: &Entry, options: &Options, counted: Option<Option<u64>>) -> Measure {
    let mut measure = Measure {
        bytes: if options.apparent {
            entry.bytes
        } else {
            entry.allocated
        },
        ..Measure::default()
    };
    match counted {
        Some(Some(lines)) => measure.lines = lines,
        Some(None) => measure.unreadable += 1,
        None => {}
    }
    measure
}

fn kind_of(objtype: u32) -> &'static str {
    match objtype {
        VDIR => "directory",
        VLNK => "link",
        VREG => "file",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// One row, reduced to the fields both walks must agree on.
    type Shape = (String, &'static str, bool, u64, u64);

    /// One link: which file it is, where it sits, and what it measures.
    type Linked = ((u64, u64), String, u64, u64);

    /// A whole walk, reduced the same way.
    type Answer = (u64, u64, usize, Vec<Shape>, Vec<Linked>);

    /// Everything the two walks have to agree about, in a comparable shape —
    /// including the links, since the bulk walk reads device, inode and link
    /// count out of a packed reply rather than out of a `stat`.
    fn shape(walked: Walked) -> Answer {
        let mut rows: Vec<Shape> = walked
            .rows
            .into_iter()
            .map(|row| {
                (
                    row.name,
                    row.kind,
                    row.executable,
                    row.measure.bytes,
                    row.measure.lines,
                )
            })
            .collect();
        rows.sort();
        let mut links: Vec<Linked> = walked
            .links
            .into_iter()
            .map(|link| (link.file, link.path, link.bytes, link.lines))
            .collect();
        links.sort();
        (
            walked.measure.bytes,
            walked.measure.lines,
            walked.measure.unreadable,
            rows,
            links,
        )
    }

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let at = |path: &str| root.path().join(path);
        fs::create_dir_all(at("visible/deeper")).unwrap();
        fs::create_dir_all(at(".hidden/deeper")).unwrap();
        fs::write(at("notes.txt"), "one\ntwo\nthree\n").unwrap();
        fs::write(at("no-newline.txt"), "trailing").unwrap();
        fs::write(at("binary.bin"), b"\0\0\n\n\n").unwrap();
        fs::write(at(".secret"), "hidden\n").unwrap();
        fs::write(at("visible/inner.txt"), "a\nb\n").unwrap();
        fs::write(at("visible/deeper/deep.txt"), "deep\n").unwrap();
        fs::write(at(".hidden/deeper/buried.txt"), "buried\n").unwrap();
        fs::write(at("spaced name.txt"), "space\n").unwrap();
        fs::write(at("üni—code.txt"), "unicode\n").unwrap();
        let runnable = at("run.sh");
        fs::write(&runnable, "#!/bin/sh\n").unwrap();
        let mut mode = fs::metadata(&runnable).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        fs::set_permissions(&runnable, mode).unwrap();
        fs::hard_link(at("notes.txt"), at("visible/same-file.txt")).unwrap();
        std::os::unix::fs::symlink("notes.txt", at("link")).unwrap();
        std::os::unix::fs::symlink("/nowhere", at("broken")).unwrap();
        std::os::unix::fs::symlink("visible", at("dirlink")).unwrap();
        root
    }

    /// The bulk walk is an optimisation, so it has to answer exactly what the
    /// portable walk answers — down to the unreadable count.
    #[test]
    fn the_bulk_walk_agrees_with_the_portable_one() {
        let root = fixture();
        for lines in [false, true] {
            for all in [false, true] {
                for apparent in [false, true] {
                    for display_depth in [1, 2, usize::MAX] {
                        let options = Options {
                            lines,
                            all,
                            apparent,
                            display_depth,
                            ..Options::default()
                        };
                        let bulk = shape(walk(&options, root.path()).expect("bulk walk runs"));
                        let portable = shape(super::super::walk_directory(
                            &options,
                            root.path(),
                            Path::new(""),
                            0,
                        ));
                        assert!(!bulk.4.is_empty(), "the fixture has a hardlink");
                        assert_eq!(
                            bulk, portable,
                            "lines={lines} all={all} apparent={apparent} depth={display_depth}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ignored_entries_never_reach_the_totals() {
        let root = fixture();
        let plain = Options {
            display_depth: 1,
            ..Options::default()
        };
        let whole = walk(&plain, root.path()).expect("bulk walk runs");
        let options = Options {
            display_depth: 1,
            ignore: super::super::Ignore::new(&["visible".to_string()]),
            ..Options::default()
        };
        let trimmed = walk(&options, root.path()).expect("bulk walk runs");
        assert!(trimmed.measure.bytes < whole.measure.bytes);
        assert!(!trimmed.rows.iter().any(|row| row.name == "visible"));
        let portable = super::super::walk_directory(&options, root.path(), Path::new(""), 0);
        assert_eq!(trimmed.measure.bytes, portable.measure.bytes);
    }

    /// The device comes out of the packed reply rather than a `stat`, so `-x`
    /// has to agree with the portable walk about which entries it drops.
    #[test]
    fn one_file_system_agrees_with_the_portable_walk() {
        let root = fixture();
        let here = super::super::device(&fs::symlink_metadata(root.path()).unwrap());
        for device in [Some(here), Some(here.wrapping_add(1)), None] {
            let options = Options {
                display_depth: usize::MAX,
                device,
                ..Options::default()
            };
            let bulk = shape(walk(&options, root.path()).expect("bulk walk runs"));
            let portable = shape(super::super::walk_directory(
                &options,
                root.path(),
                Path::new(""),
                0,
            ));
            assert_eq!(bulk, portable, "device={device:?}");
        }
        // The fixture is all on one device, so only a device it is not on can
        // show that anything is being dropped at all.
        let elsewhere = Options {
            display_depth: usize::MAX,
            device: Some(here.wrapping_add(1)),
            ..Options::default()
        };
        let walked = walk(&elsewhere, root.path()).expect("bulk walk runs");
        assert_eq!(walked.measure.bytes, 0);
        assert!(walked.rows.is_empty());
    }

    #[test]
    fn directory_symlinks_are_not_followed() {
        let root = fixture();
        let options = Options {
            lines: false,
            all: false,
            display_depth: usize::MAX,
            ..Options::default()
        };
        let walked = walk(&options, root.path()).expect("bulk walk runs");
        let rows = &walked.rows;
        let dirlink = rows.iter().find(|row| row.name == "dirlink").unwrap();
        assert_eq!(dirlink.kind, "link");
        assert!(!rows.iter().any(|row| row.name.starts_with("dirlink/")));
    }
}
