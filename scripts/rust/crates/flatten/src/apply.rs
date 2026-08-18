//! Carrying out a plan.
//!
//! Every move is a rename between two open directories, which is why nothing
//! here can half-move a file: a rename either happened or did not, and the
//! entry is in exactly one place either way. Directories are removed only
//! after every move, and only from the inside out.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

use crate::dir::{Dir, directory_not_empty};
use crate::plan::{Collapse, Deep, Spot, show};

/// What a run actually managed to do.
#[derive(Default)]
pub struct Done {
    pub moved: usize,
    pub removed: usize,
    pub failures: Vec<Failure>,
}

/// One thing that did not happen, named the way the plan named it.
pub struct Failure {
    pub path: String,
    pub error: io::Error,
}

/// Told about each move as it is made, for `--verbose`.
pub type Narrate<'a> = &'a mut dyn FnMut(&str, &str);

/// Lift the innermost wrapper's contents into the target and remove the
/// wrappers.
pub fn collapse(target: &Path, plan: &Collapse, narrate: Narrate) -> io::Result<Done> {
    let root = Dir::open(target)?;
    // One component at a time, so no step of the chain can be a symlink
    // someone put there between the survey and now.
    let mut chain: Vec<Dir> = Vec::with_capacity(plan.chain.len());
    for name in &plan.chain {
        let below = {
            let parent = chain.last().unwrap_or(&root);
            parent.child(name)?
        };
        chain.push(below);
    }

    // The outermost wrapper is still holding a name one of the entries is
    // about to take. Move it aside first — the descriptors already open go
    // on referring to the same directories whatever they are called.
    let mut outer = plan.chain[0].clone();
    if plan.shadowed() {
        let mut taken: HashSet<&OsStr> = plan.entries.iter().map(OsString::as_os_str).collect();
        taken.insert(&plan.chain[0]);
        let free = spare(&taken);
        root.move_entry(&outer, &root, &free)?;
        outer = free;
    }

    let mut done = Done::default();
    let inner = chain.last().expect("a collapse has at least one wrapper");
    for entry in &plan.entries {
        narrate(&plan.source(entry), &show(entry));
        match inner.move_entry(entry, &root, entry) {
            Ok(()) => done.moved += 1,
            Err(error) => done.failures.push(Failure {
                path: plan.source(entry),
                error,
            }),
        }
    }

    for step in (0..chain.len()).rev() {
        let name: &OsStr = if step == 0 { &outer } else { &plan.chain[step] };
        let parent = if step == 0 { &root } else { &chain[step - 1] };
        if let Err(error) = parent.remove_dir(name) {
            done.failures.push(Failure {
                path: wrapper(plan, step),
                error,
            });
        } else {
            done.removed += 1;
        }
    }
    Ok(done)
}

/// Move every entry the plan settled on up into the target, then remove the
/// directories left with nothing in them.
pub fn deep(target: &Path, plan: &Deep, narrate: Narrate) -> io::Result<Done> {
    let root = Dir::open(target)?;
    let mut done = Done::default();

    // What each directory is called on disk for the rest of this run, which
    // is its own name except for the ones moved out of the way below.
    let mut names: Vec<OsString> = plan.dirs.iter().map(|node| node.name.clone()).collect();
    let moved_aside = move_aside(&root, plan, &mut names, &mut done);

    // A name more than one entry is moving to has to be landed on in the
    // order the answers settled, so the last answer is the one that stands.
    // The rest is a walk of the tree, which reuses each directory's
    // descriptor for everything inside it.
    let contested = plan.contested();
    descend(
        &root, 0, plan, &root, &names, &contested, narrate, &mut done,
    );
    for spot in plan.moves() {
        if !contested.contains(plan.name(spot)) {
            continue;
        }
        move_one(&root, plan, &names, spot, narrate, &mut done);
    }

    let removable = plan.removable();
    let mut gone = vec![false; plan.dirs.len()];
    remove(&root, 0, plan, &names, &removable, &mut gone, &mut done);
    // A directory moved aside is one that was supposed to be gone. If it is
    // still here, put its name back rather than leave a stranger behind.
    for child in moved_aside {
        if gone[child] {
            continue;
        }
        if let Err(error) = root.move_entry(&names[child], &root, &plan.dirs[child].name) {
            done.failures.push(Failure {
                path: show(&names[child]),
                error,
            });
        }
    }
    Ok(done)
}

/// Move the directories whose name an entry is about to take out of the way.
/// They are all on their way out; this only decides what they are called on
/// the way.
fn move_aside(root: &Dir, plan: &Deep, names: &mut [OsString], done: &mut Done) -> Vec<usize> {
    let shadowed = plan.shadowed();
    if shadowed.is_empty() {
        return Vec::new();
    }
    let mut taken: HashSet<&OsStr> = plan.dirs[0]
        .leaves
        .iter()
        .map(|leaf| leaf.name.as_os_str())
        .collect();
    for child in &plan.dirs[0].children {
        taken.insert(&plan.dirs[*child].name);
    }
    for spot in plan.moves() {
        taken.insert(plan.name(spot));
    }
    let mut aside = Vec::new();
    let mut chosen: Vec<OsString> = Vec::new();
    for child in shadowed {
        let mut busy = taken.clone();
        busy.extend(chosen.iter().map(OsString::as_os_str));
        let free = spare(&busy);
        match root.move_entry(&plan.dirs[child].name, root, &free) {
            Ok(()) => {
                names[child] = free.clone();
                chosen.push(free);
                aside.push(child);
            }
            Err(error) => done.failures.push(Failure {
                path: show(&plan.dirs[child].name),
                error,
            }),
        }
    }
    aside
}

/// Walk the tree, moving what this directory holds before opening the ones
/// below it, so a directory's descriptor is opened once and used for
/// everything in it.
#[allow(clippy::too_many_arguments)]
fn descend(
    dir: &Dir,
    index: usize,
    plan: &Deep,
    root: &Dir,
    names: &[OsString],
    contested: &HashSet<OsString>,
    narrate: Narrate,
    done: &mut Done,
) {
    for (leaf, entry) in plan.dirs[index].leaves.iter().enumerate() {
        if !entry.moves || contested.contains(&entry.name) {
            continue;
        }
        let spot = Spot { dir: index, leaf };
        narrate(&plan.source(spot), &show(&entry.name));
        match dir.move_entry(&entry.name, root, &entry.name) {
            Ok(()) => done.moved += 1,
            Err(error) => done.failures.push(Failure {
                path: plan.source(spot),
                error,
            }),
        }
    }
    for child in &plan.dirs[index].children {
        match dir.child(&names[*child]) {
            Ok(below) => descend(&below, *child, plan, root, names, contested, narrate, done),
            Err(error) => done.failures.push(Failure {
                path: directory(plan, *child),
                error,
            }),
        }
    }
}

/// Move one entry with its directory opened from the target down, for the few
/// that cannot be done on the way past.
fn move_one(
    root: &Dir,
    plan: &Deep,
    names: &[OsString],
    spot: Spot,
    narrate: Narrate,
    done: &mut Done,
) {
    let source = plan.source(spot);
    let mut line: Option<Dir> = None;
    for step in plan.ancestry(spot.dir) {
        let below = {
            let parent = line.as_ref().unwrap_or(root);
            parent.child(&names[step])
        };
        match below {
            Ok(below) => line = Some(below),
            Err(error) => {
                done.failures.push(Failure {
                    path: source,
                    error,
                });
                return;
            }
        }
    }
    let dir = line.as_ref().unwrap_or(root);
    let name = plan.name(spot);
    narrate(&source, &show(name));
    match dir.move_entry(name, root, name) {
        Ok(()) => done.moved += 1,
        Err(error) => done.failures.push(Failure {
            path: source,
            error,
        }),
    }
}

/// Remove what the plan said would be empty, from the inside out. A directory
/// keeping something the plan left alone is walked into but not removed.
fn remove(
    dir: &Dir,
    index: usize,
    plan: &Deep,
    names: &[OsString],
    removable: &[bool],
    gone: &mut [bool],
    done: &mut Done,
) {
    for child in &plan.dirs[index].children {
        if let Ok(below) = dir.child(&names[*child]) {
            remove(&below, *child, plan, names, removable, gone, done);
        }
        if !removable[*child] {
            continue;
        }
        match dir.remove_dir(&names[*child]) {
            Ok(()) => {
                gone[*child] = true;
                done.removed += 1;
            }
            // The plan said this would be empty. Something else put something
            // in it, or a move above failed; either way it is not a rename
            // this run gets to pretend it made.
            Err(error) if directory_not_empty(&error) => done.failures.push(Failure {
                path: directory(plan, *child),
                error: io::Error::other("still has something in it"),
            }),
            Err(error) => done.failures.push(Failure {
                path: directory(plan, *child),
                error,
            }),
        }
    }
}

fn directory(plan: &Deep, index: usize) -> String {
    plan.dirs[index].prefix.trim_end_matches('/').to_string()
}

fn wrapper(plan: &Collapse, step: usize) -> String {
    plan.chain[..=step]
        .iter()
        .map(|name| show(name))
        .collect::<Vec<String>>()
        .join("/")
}

/// A name nothing in the target has or is about to have. Hidden, so a run
/// that dies between the two renames leaves nothing in the way of the next
/// one — which will collapse it back out anyway.
fn spare(taken: &HashSet<&OsStr>) -> OsString {
    let mut nth = 0;
    loop {
        let candidate = OsString::from(format!(".flatten-{nth}"));
        if !taken.contains(candidate.as_os_str()) {
            return candidate;
        }
        nth += 1;
    }
}
