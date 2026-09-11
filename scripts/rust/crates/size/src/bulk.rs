use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rayon::prelude::*;
use rustix::fs::{Mode, OFlags, fstat, open, openat};
use workstation::path;

use super::bulk_decode::{self, Entry, decode};
use super::{Link, Measure, Options, Row, Walked, count_lines_in};

// `enum vtype`, from <sys/vnode.h>. Anything else is an "other" to us.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const BATCH: usize = 256 * 1024;

struct Dir(OwnedFd);

impl Dir {
    fn open(path: &Path) -> Option<Dir> {
        open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()
        .map(Dir)
    }

    fn open_child(&self, name: &CStr) -> Option<Dir> {
        openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()
        .map(Dir)
    }

    fn status(&self) -> Option<Status> {
        let stat = fstat(&self.0).ok()?;
        Some(Status {
            device: stat.st_dev as u64,
            bytes: stat.st_blocks as u64 * 512,
        })
    }

    fn open_file(&self, name: &CStr) -> Option<File> {
        openat(
            &self.0,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()
        .map(File::from)
    }
}

struct Status {
    device: u64,
    bytes: u64,
}

fn attributes() -> libc::attrlist {
    libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: bulk_decode::COMMON,
        volattr: 0,
        dirattr: bulk_decode::DIR_ALLOCSIZE,
        fileattr: bulk_decode::FILE,
        forkattr: 0,
    }
}

#[allow(unsafe_code)]
fn next_batch(dir: &Dir, list: &mut libc::attrlist, buffer: &mut [u8]) -> Option<usize> {
    // SAFETY: Dir owns a readable directory fd; list is fully initialized.
    // The kernel receives exclusive access to exactly buffer.len() writable
    // bytes. Borrowed entries are consumed before this buffer is reused.
    let count = unsafe {
        libc::getattrlistbulk(
            dir.0.as_raw_fd(),
            (list as *mut libc::attrlist).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    };
    usize::try_from(count).ok()
}

pub fn walk(options: &Options, target: &Path) -> Option<Walked> {
    let dir = Dir::open(target)?;
    read(options, &dir, target, Path::new(""), 0)
}

struct Child {
    name: CString,
    visible: bool,
    depth: usize,
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
        let count = next_batch(dir, &mut list, &mut buffer)?;
        if count == 0 {
            break;
        }

        // Decode the batch first. The names borrow the buffer, so everything
        // below must finish with them before the next call overwrites it.
        let mut batch = Vec::with_capacity(count.min(buffer.len() / 24));
        let mut entry = buffer.as_slice();
        for _ in 0..count {
            let (found, length) = decode(entry)?;
            entry = entry.get(length..)?;
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
            let is_hidden = path::hidden(OsStr::from_bytes(found.name.to_bytes()));
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
#[path = "../tests/unit/bulk_tests.rs"]
mod tests;
