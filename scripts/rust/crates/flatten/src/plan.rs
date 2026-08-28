
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub dir: usize,
    pub leaf: usize,
}

pub struct Leaf {
    pub name: OsString,
    pub moves: bool,
}

pub struct Node {
    pub name: OsString,
    pub prefix: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub leaves: Vec<Leaf>,
}

pub struct Collapse {
    pub chain: Vec<OsString>,
    pub entries: Vec<OsString>,
}

impl Collapse {
    pub fn source(&self, entry: &OsStr) -> String {
        let mut path = String::new();
        for step in &self.chain {
            path.push_str(&show(step));
            path.push('/');
        }
        path.push_str(&show(entry));
        path
    }

    pub fn shadowed(&self) -> bool {
        self.entries.iter().any(|entry| *entry == self.chain[0])
    }
}

pub struct Deep {
    pub dirs: Vec<Node>,
    pub order: Vec<Spot>,
    pub collisions: Vec<Spot>,
    pub unreadable: usize,
    holders: HashMap<OsString, Spot>,
}

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

    pub fn source(&self, spot: Spot) -> String {
        format!("{}{}", self.dirs[spot.dir].prefix, show(self.name(spot)))
    }

    pub fn holder(&self, spot: Spot) -> String {
        match self.holders.get(self.name(spot)) {
            Some(held) => self.source(*held),
            None => self.source(spot),
        }
    }

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

    pub fn is_empty(&self) -> bool {
        self.dirs.len() == 1
    }

    pub fn removable(&self) -> Vec<bool> {
        let mut removable = vec![true; self.dirs.len()];
        for (index, node) in self.dirs.iter().enumerate().rev() {
            removable[index] = node.leaves.iter().all(|leaf| leaf.moves)
                && node.children.iter().all(|child| removable[*child]);
        }
        removable[0] = false;
        removable
    }

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

pub enum Plan {
    Nothing,
    Collapse(Collapse),
    Deep(Deep),
}

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

pub fn show(name: &OsStr) -> String {
    Path::new(name).display().to_string()
}

fn lone_directory(entries: &[(OsString, bool)]) -> Option<OsString> {
    match entries {
        [(name, true)] => Some(name.clone()),
        _ => None,
    }
}

fn listing(path: &Path) -> io::Result<Vec<(OsString, bool)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        entries.push((entry.file_name(), kind.is_dir()));
    }
    Ok(entries)
}

struct Raw {
    name: OsString,
    dirs: Vec<Raw>,
    leaves: Vec<OsString>,
    unreadable: usize,
}

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
