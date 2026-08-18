//! What a flatten would do, worked out in full before anything moves.
//!
//! There are two shapes of answer. A collapse walks the chain of wrapper
//! directories that hold exactly one thing each and stops at the first
//! directory holding something worth keeping where it is; it can never land
//! two entries on one name, which is what lets it run without asking. A deep
//! flatten surveys the whole tree, so it can land two entries on one name,
//! and every one of those is settled before the first rename.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

/// Where one entry sits: which directory of the survey, and which of its
/// entries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub dir: usize,
    pub leaf: usize,
}

/// An entry that is not a directory — a file, a symlink, a socket. What a
/// deep flatten moves, and what a collision is between.
pub struct Leaf {
    pub name: OsString,
    /// Settled by planning, and for a collision by whoever is asked.
    pub moves: bool,
}

/// One directory of the survey. Children always sit at a higher index than
/// their parent, so anything that reads bottom-up is one reverse pass.
pub struct Node {
    pub name: OsString,
    /// The directory's path relative to the target, ending in a separator:
    /// `""` at the target itself, then `"sub/"`, `"sub/x/"`.
    pub prefix: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub leaves: Vec<Leaf>,
}

/// Lifting the contents of the innermost wrapper straight into the target.
pub struct Collapse {
    /// The wrapper directories, outermost first. Never empty.
    pub chain: Vec<OsString>,
    /// What the innermost one holds, and what the target is about to hold.
    pub entries: Vec<OsString>,
}

impl Collapse {
    /// Where a lifted entry is now, relative to the target.
    pub fn source(&self, entry: &OsStr) -> String {
        let mut path = String::new();
        for step in &self.chain {
            path.push_str(&show(step));
            path.push('/');
        }
        path.push_str(&show(entry));
        path
    }

    /// Whether the innermost wrapper holds an entry named after the outermost
    /// one — the one way a collapse can land on a name that is still taken,
    /// by the very wrapper it is emptying.
    pub fn shadowed(&self) -> bool {
        self.entries.iter().any(|entry| *entry == self.chain[0])
    }
}

/// Moving every buried entry up into the target and removing what held them.
pub struct Deep {
    pub dirs: Vec<Node>,
    /// Every entry below the target, shallowest first and then by path: the
    /// order names are claimed in, so the shallowest entry keeps its name and
    /// anything deeper has to ask.
    pub order: Vec<Spot>,
    /// The entries in `order` that found their name already claimed.
    pub collisions: Vec<Spot>,
    /// Directories the survey could not read, and so cannot empty.
    pub unreadable: usize,
    holders: HashMap<OsString, Spot>,
}

/// A move that had to be given up on, and why.
pub struct Refusal {
    pub source: String,
    pub reason: String,
}

impl Deep {
    fn new(dirs: Vec<Node>, unreadable: usize) -> Deep {
        let mut order: Vec<Spot> = Vec::new();
        for (dir, node) in dirs.iter().enumerate() {
            for leaf in 0..node.leaves.len() {
                order.push(Spot { dir, leaf });
            }
        }
        // Shallowest first, so the entry nearest the top keeps its name;
        // then by path, so the same tree always picks the same winner.
        order.sort_by(|left, right| {
            let (here, there) = (&dirs[left.dir], &dirs[right.dir]);
            here.depth
                .cmp(&there.depth)
                .then_with(|| here.prefix.cmp(&there.prefix))
                .then_with(|| {
                    here.leaves[left.leaf]
                        .name
                        .cmp(&there.leaves[right.leaf].name)
                })
        });

        let mut plan = Deep {
            dirs,
            order,
            collisions: Vec::new(),
            unreadable,
            holders: HashMap::new(),
        };
        for index in 0..plan.order.len() {
            let spot = plan.order[index];
            let name = plan.name(spot).to_os_string();
            if plan.holders.contains_key(&name) {
                plan.collisions.push(spot);
                continue;
            }
            // An entry already in the target holds its name without going
            // anywhere; the first claimant from below has to move to it.
            let moves = plan.dirs[spot.dir].depth > 0;
            plan.holders.insert(name, spot);
            plan.leaf_mut(spot).moves = moves;
        }
        plan
    }

    pub fn name(&self, spot: Spot) -> &OsStr {
        &self.dirs[spot.dir].leaves[spot.leaf].name
    }

    /// Where an entry is now, relative to the target.
    pub fn source(&self, spot: Spot) -> String {
        format!("{}{}", self.dirs[spot.dir].prefix, show(self.name(spot)))
    }

    /// What holds the name this entry wants, as things stand.
    pub fn holder(&self, spot: Spot) -> String {
        match self.holders.get(self.name(spot)) {
            Some(held) => self.source(*held),
            None => self.source(spot),
        }
    }

    /// Let a collision have the name after all: it moves, and whatever is
    /// there when it lands is replaced.
    pub fn accept(&mut self, spot: Spot) {
        self.holders.insert(self.name(spot).to_os_string(), spot);
        self.leaf_mut(spot).moves = true;
    }

    pub fn moving(&self, spot: Spot) -> bool {
        self.dirs[spot.dir].leaves[spot.leaf].moves
    }

    pub fn moves(&self) -> impl Iterator<Item = Spot> + '_ {
        self.order.iter().copied().filter(|spot| self.moving(*spot))
    }

    /// Nothing below the target at all: no directory to remove, and so
    /// nothing a deep flatten would do that has not already been done.
    pub fn is_empty(&self) -> bool {
        self.dirs.len() == 1
    }

    /// Which directories will be gone afterwards: the ones left with nothing
    /// in them, all the way down. The target itself is never one of them.
    pub fn removable(&self) -> Vec<bool> {
        let mut removable = vec![true; self.dirs.len()];
        for (index, node) in self.dirs.iter().enumerate().rev() {
            removable[index] = node.leaves.iter().all(|leaf| leaf.moves)
                && node.children.iter().all(|child| removable[*child]);
        }
        removable[0] = false;
        removable
    }

    /// Names more than one entry is moving to. They have to land in the order
    /// they were settled in, so the last one settled is the one left there.
    pub fn contested(&self) -> HashSet<OsString> {
        let mut seen: HashSet<&OsStr> = HashSet::new();
        let mut twice: HashSet<OsString> = HashSet::new();
        for spot in self.moves() {
            if !seen.insert(self.name(spot)) {
                twice.insert(self.name(spot).to_os_string());
            }
        }
        twice
    }

    /// Settle the one conflict nobody is asked about: an entry whose name is
    /// a directory in the target that is not going away, because something
    /// inside it is staying. A name cannot be both, and the directory was
    /// there first — so the move is given up, which can in turn keep another
    /// directory alive, so this runs until nothing changes.
    pub fn refuse_shadowed(&mut self) -> Vec<Refusal> {
        let mut refusals = Vec::new();
        loop {
            let removable = self.removable();
            let mut settled = true;
            for child in self.dirs[0].children.clone() {
                if removable[child] {
                    continue;
                }
                let Some(spot) = self.holders.get(&self.dirs[child].name).copied() else {
                    continue;
                };
                if !self.moving(spot) {
                    continue;
                }
                refusals.push(Refusal {
                    source: self.source(spot),
                    reason: format!(
                        "{} is a directory that is staying",
                        show(&self.dirs[child].name)
                    ),
                });
                self.leaf_mut(spot).moves = false;
                settled = false;
            }
            if settled {
                return refusals;
            }
        }
    }

    /// Directories in the target whose name a moving entry wants, and which
    /// are going away — so the name is theirs only until they do.
    pub fn shadowed(&self) -> Vec<usize> {
        let removable = self.removable();
        self.dirs[0]
            .children
            .iter()
            .copied()
            .filter(|child| removable[*child])
            .filter(|child| match self.holders.get(&self.dirs[*child].name) {
                Some(spot) => self.moving(*spot),
                None => false,
            })
            .collect()
    }

    /// The directories between the target and this one, outermost first.
    pub fn ancestry(&self, dir: usize) -> Vec<usize> {
        let mut line = Vec::new();
        let mut at = dir;
        while let Some(parent) = self.dirs[at].parent {
            line.push(at);
            at = parent;
        }
        line.reverse();
        line
    }

    fn leaf_mut(&mut self, spot: Spot) -> &mut Leaf {
        &mut self.dirs[spot.dir].leaves[spot.leaf]
    }
}

/// What a run is going to do, or that it is going to do nothing.
pub enum Plan {
    Nothing,
    Collapse(Collapse),
    Deep(Deep),
}

/// Work out the collapse: walk down while a directory holds exactly one
/// entry and that entry is a directory in its own right.
pub fn collapse(target: &Path) -> io::Result<Plan> {
    let mut path = PathBuf::from(target);
    let mut here = listing(&path)?;
    let mut chain = Vec::new();
    while let Some(name) = lone_directory(&here) {
        path.push(&name);
        chain.push(name);
        here = listing(&path)?;
    }
    if chain.is_empty() {
        return Ok(Plan::Nothing);
    }
    Ok(Plan::Collapse(Collapse {
        chain,
        entries: here.into_iter().map(|(name, _)| name).collect(),
    }))
}

/// Survey the whole tree under the target and settle every name only one
/// entry can have.
pub fn deep(target: &Path) -> io::Result<Plan> {
    // The target itself failing is the caller's problem to report; a
    // directory below it failing is counted, and the rest still runs.
    fs::read_dir(target)?;
    let (dirs, unreadable) = arena(read(target.to_path_buf(), OsString::new()));
    let plan = Deep::new(dirs, unreadable);
    if plan.is_empty() {
        return Ok(Plan::Nothing);
    }
    Ok(Plan::Deep(plan))
}

/// A name as it should be read by a person.
pub fn show(name: &OsStr) -> String {
    Path::new(name).display().to_string()
}

fn lone_directory(entries: &[(OsString, bool)]) -> Option<OsString> {
    match entries {
        [(name, true)] => Some(name.clone()),
        _ => None,
    }
}

/// A directory's entries, each with whether it is a directory in its own
/// right. `file_type` reads the directory entry rather than what it points
/// at, so a symlink is never mistaken for the directory on the other end —
/// which is also what keeps a link loop from becoming a hang.
fn listing(path: &Path) -> io::Result<Vec<(OsString, bool)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        entries.push((entry.file_name(), kind.is_dir()));
    }
    Ok(entries)
}

/// The tree as read, before it is laid out flat.
struct Raw {
    name: OsString,
    dirs: Vec<Raw>,
    leaves: Vec<OsString>,
    unreadable: usize,
}

/// Each directory's subdirectories are read in parallel: the walk is waiting
/// on directory reads, and a thread per core hides most of that on a large
/// tree.
fn read(path: PathBuf, name: OsString) -> Raw {
    let Ok(listing) = fs::read_dir(&path) else {
        return Raw {
            name,
            dirs: Vec::new(),
            leaves: Vec::new(),
            unreadable: 1,
        };
    };
    let mut unreadable = 0;
    let mut below = Vec::new();
    let mut leaves = Vec::new();
    for entry in listing {
        // An entry that fails mid-listing is one this walk cannot see, which
        // is the same thing an unreadable directory is: say so, rather than
        // report a tree emptier than it is.
        let Ok(entry) = entry else {
            unreadable += 1;
            continue;
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => below.push((entry.file_name(), entry.path())),
            Ok(_) => leaves.push(entry.file_name()),
            Err(_) => unreadable += 1,
        }
    }
    let dirs: Vec<Raw> = below
        .into_par_iter()
        .map(|(name, path)| read(path, name))
        .collect();
    Raw {
        name,
        dirs,
        leaves,
        unreadable,
    }
}

/// Lay the tree out as a vector, parents before children, so a bottom-up pass
/// is one reverse iteration and a directory is a `usize` rather than a borrow.
fn arena(root: Raw) -> (Vec<Node>, usize) {
    let mut dirs = vec![Node {
        name: OsString::new(),
        prefix: String::new(),
        depth: 0,
        parent: None,
        children: Vec::new(),
        leaves: Vec::new(),
    }];
    let mut unreadable = 0;
    // Each pending entry is a directory that has its slot but not yet its
    // children: the tree it came from, and where the slot is.
    let mut pending = vec![(root, 0usize)];
    while let Some((raw, index)) = pending.pop() {
        unreadable += raw.unreadable;
        dirs[index].leaves = raw
            .leaves
            .into_iter()
            .map(|name| Leaf { name, moves: false })
            .collect();
        let depth = dirs[index].depth;
        let prefix = dirs[index].prefix.clone();
        for child in raw.dirs {
            let slot = dirs.len();
            dirs.push(Node {
                prefix: format!("{prefix}{}/", show(&child.name)),
                name: child.name.clone(),
                depth: depth + 1,
                parent: Some(index),
                children: Vec::new(),
                leaves: Vec::new(),
            });
            dirs[index].children.push(slot);
            pending.push((child, slot));
        }
    }
    (dirs, unreadable)
}
