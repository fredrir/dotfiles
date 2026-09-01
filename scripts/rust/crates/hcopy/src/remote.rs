use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::place;

const CONNECT: &str = "ConnectTimeout=8";
const QUIET: &str = "LogLevel=ERROR";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub path: String,
    pub home: String,
    pub entries: Vec<Entry>,
    pub missing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    File,
    Missing,
}

pub enum Target {
    Home(String),
    Absolute(String),
}

impl Target {
    fn expression(&self) -> String {
        match self {
            Target::Home(rest) if rest.is_empty() => "\"$HOME\"".to_string(),
            Target::Home(rest) => format!("\"$HOME\"/{}", place::quote(rest)),
            Target::Absolute(path) => place::quote(path),
        }
    }
}

#[derive(Clone)]
pub struct Peer {
    host: String,
    cache: Arc<Mutex<HashMap<String, Listing>>>,
    inflight: Arc<Mutex<HashSet<String>>>,
}

impl Peer {
    pub fn new(host: &str) -> Peer {
        Peer {
            host: host.to_string(),
            cache: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn list(&self, target: &Target) -> Result<Listing, String> {
        if let Target::Absolute(path) = target
            && let Some(found) = self.cached(path)
        {
            return Ok(found);
        }
        let listing = fetch(&self.host, target)?;
        self.remember(&listing);
        Ok(listing)
    }

    pub fn cached(&self, path: &str) -> Option<Listing> {
        self.cache.lock().ok()?.get(path).cloned()
    }

    // Walking the list is the moment to pay for the directory under the
    // cursor, so that entering it is already answered when it is asked for.
    pub fn prefetch(&self, path: String) {
        if self.cached(&path).is_some() {
            return;
        }
        {
            let Ok(mut inflight) = self.inflight.lock() else {
                return;
            };
            if !inflight.insert(path.clone()) {
                return;
            }
        }
        let peer = self.clone();
        std::thread::spawn(move || {
            if let Ok(listing) = fetch(&peer.host, &Target::Absolute(path.clone())) {
                peer.remember(&listing);
            }
            if let Ok(mut inflight) = peer.inflight.lock() {
                inflight.remove(&path);
            }
        });
    }

    fn remember(&self, listing: &Listing) {
        if listing.missing {
            return;
        }
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(listing.path.clone(), listing.clone());
        }
    }

    // Anything already listed has been paid for once. Asking the other
    // machine to confirm it a second time is a whole round trip, which on a
    // route that fell back to Tailscale is seconds rather than milliseconds.
    pub fn knows_directory(&self, path: &str) -> bool {
        self.cached(path).is_some()
    }

    pub fn knows_entry(&self, path: &str) -> bool {
        let name = place::name_of(path);
        self.cached(place::parent_of(path))
            .is_some_and(|listing| listing.entries.iter().any(|entry| entry.name == name))
    }

    pub fn kind(&self, path: &str) -> Result<Kind, String> {
        let quoted = place::quote(path);
        let script = format!(
            "if [ -d {quoted} ]; then echo directory; \
             elif [ -e {quoted} ] || [ -L {quoted} ]; then echo file; \
             else echo missing; fi"
        );
        Ok(match run(&self.host, &script)?.trim() {
            "directory" => Kind::Directory,
            "file" => Kind::File,
            _ => Kind::Missing,
        })
    }

    pub fn make_directory(&self, path: &str) -> Result<(), String> {
        run(&self.host, &format!("mkdir -p -- {}", place::quote(path))).map(|_| ())
    }
}

fn fetch(host: &str, target: &Target) -> Result<Listing, String> {
    parse(&run(host, &script(target))?)
}

// One round trip answers where home is, where the request landed, and what is
// in it, because over a shared connection the trip is the whole cost.
//
// It ends in `exit 0` so that the status reports the connection and nothing
// else: `ls -L` returns a failure for a home with one dangling symlink in it,
// which says nothing about the listing it just printed.
fn script(target: &Target) -> String {
    format!(
        "printf 'HOME\\t%s\\n' \"$HOME\"; \
         if cd -- {} 2>/dev/null; then printf 'DIR\\t%s\\n' \"$PWD\"; ls -1ApL 2>/dev/null; \
         else printf 'GONE\\t\\n'; fi; exit 0",
        target.expression()
    )
}

fn run(host: &str, script: &str) -> Result<String, String> {
    let output = Command::new("ssh")
        .args(["-T", "-o", CONNECT, "-o", QUIET, host, script])
        .output()
        .map_err(|error| format!("ssh: {error}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("no answer");
        return Err(format!("{host}: {}", reason.trim()));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{host}: replied with non-UTF-8 names"))
}

fn parse(text: &str) -> Result<Listing, String> {
    let mut listing = Listing::default();
    let mut found = false;
    for line in text.lines() {
        match line.split_once('\t') {
            Some(("HOME", value)) => listing.home = value.to_string(),
            Some(("DIR", value)) => {
                listing.path = value.to_string();
                found = true;
            }
            Some(("GONE", _)) => listing.missing = true,
            _ => listing.entries.push(entry(line)),
        }
    }
    if listing.home.is_empty() {
        return Err("the other machine did not say where its home is".into());
    }
    if !found && !listing.missing {
        return Err("the other machine did not say which directory it listed".into());
    }
    listing.entries.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(listing)
}

fn entry(line: &str) -> Entry {
    match line.strip_suffix('/') {
        Some(name) => Entry {
            name: name.to_string(),
            directory: true,
        },
        None => Entry {
            name: line.to_string(),
            directory: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_separates_the_directories_from_the_files() {
        let listing = parse("HOME\t/home/f\nDIR\t/home/f/projects\nmy-app/\nnotes.md\nsrc/\n")
            .expect("a listing");
        assert_eq!(listing.home, "/home/f");
        assert_eq!(listing.path, "/home/f/projects");
        assert!(!listing.missing);
        assert_eq!(
            listing.entries,
            vec![
                Entry {
                    name: "my-app".into(),
                    directory: true
                },
                Entry {
                    name: "src".into(),
                    directory: true
                },
                Entry {
                    name: "notes.md".into(),
                    directory: false
                },
            ]
        );
    }

    #[test]
    fn directories_come_first_and_then_case_folded_order() {
        let listing = parse("HOME\t/home/f\nDIR\t/home/f\nb.txt\nAlpha/\na.txt\nzeta/\n").unwrap();
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "zeta", "a.txt", "b.txt"]);
    }

    #[test]
    fn a_directory_that_is_not_there_yet_still_reports_home() {
        let listing = parse("HOME\t/home/f\nGONE\t\n").unwrap();
        assert!(listing.missing);
        assert_eq!(listing.home, "/home/f");
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn an_empty_directory_is_not_a_missing_one() {
        let listing = parse("HOME\t/home/f\nDIR\t/home/f/empty\n").unwrap();
        assert!(!listing.missing);
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn a_reply_that_says_nothing_useful_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("DIR\t/home/f\n").is_err());
        assert!(parse("HOME\t/home/f\n").is_err());
    }

    #[test]
    fn a_name_with_a_space_survives_the_listing() {
        let listing = parse("HOME\t/home/f\nDIR\t/home/f\nmy notes/\na file.txt\n").unwrap();
        assert_eq!(listing.entries[0].name, "my notes");
        assert!(listing.entries[0].directory);
        assert_eq!(listing.entries[1].name, "a file.txt");
    }

    fn peer_knowing(path: &str, names: &[(&str, bool)]) -> Peer {
        let peer = Peer::new("archie");
        peer.remember(&Listing {
            path: path.to_string(),
            home: "/home/fredrir".into(),
            entries: names
                .iter()
                .map(|(name, directory)| Entry {
                    name: (*name).to_string(),
                    directory: *directory,
                })
                .collect(),
            missing: false,
        });
        peer
    }

    #[test]
    fn a_listed_directory_does_not_have_to_be_asked_about_again() {
        let peer = peer_knowing("/home/fredrir/projects", &[("my-app", true)]);
        assert!(peer.knows_directory("/home/fredrir/projects"));
        assert!(!peer.knows_directory("/home/fredrir/elsewhere"));
    }

    #[test]
    fn an_entry_of_a_listed_directory_is_known_to_be_there() {
        let peer = peer_knowing(
            "/home/fredrir/projects",
            &[("my-app", true), ("a.txt", false)],
        );
        assert!(peer.knows_entry("/home/fredrir/projects/my-app"));
        assert!(peer.knows_entry("/home/fredrir/projects/a.txt"));
        assert!(!peer.knows_entry("/home/fredrir/projects/gone"));
        assert!(!peer.knows_entry("/home/fredrir/elsewhere/my-app"));
    }

    #[test]
    fn a_directory_that_was_not_there_is_never_remembered_as_being_there() {
        let peer = Peer::new("archie");
        peer.remember(&Listing {
            path: "/home/fredrir/new".into(),
            home: "/home/fredrir".into(),
            entries: Vec::new(),
            missing: true,
        });
        assert!(!peer.knows_directory("/home/fredrir/new"));
    }

    #[test]
    fn the_listing_reports_the_connection_rather_than_what_ls_made_of_it() {
        let text = script(&Target::Home(String::new()));
        assert!(
            text.trim_end().ends_with("exit 0"),
            "a dangling symlink would be read as an unreachable machine: {text}"
        );
    }

    #[test]
    fn the_listing_asks_for_one_name_per_line_and_marks_the_directories() {
        let text = script(&Target::Absolute("/home/f".into()));
        assert!(text.contains("ls -1ApL"));
        assert!(text.contains("cd -- '/home/f'"));
    }

    #[test]
    fn a_home_target_is_expanded_by_the_other_shell_rather_than_this_one() {
        assert_eq!(Target::Home(String::new()).expression(), "\"$HOME\"");
        assert_eq!(
            Target::Home("projects".into()).expression(),
            "\"$HOME\"/'projects'"
        );
        assert_eq!(
            Target::Absolute("/etc/ssh".into()).expression(),
            "'/etc/ssh'"
        );
    }
}
