use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::place;

const CONNECT: &str = "ConnectTimeout=8";
const QUIET: &str = "LogLevel=ERROR";
const MAX_PREFETCHES: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    cache: Arc<Mutex<ListingCache>>,
    flights: Arc<(Mutex<Flights>, Condvar)>,
}

#[derive(Default)]
struct ListingCache {
    listings: HashMap<String, Listing>,
    aliases: HashMap<String, String>,
}

impl ListingCache {
    fn get(&self, path: &str) -> Option<Listing> {
        let canonical = self.aliases.get(path).map_or(path, String::as_str);
        self.listings.get(canonical).cloned()
    }

    fn canonical(&self, path: &str) -> String {
        self.aliases
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.to_string())
    }

    fn insert(&mut self, requested: Option<&str>, listing: &Listing) {
        let canonical = listing.path.clone();
        self.listings.insert(canonical.clone(), listing.clone());
        self.aliases.insert(canonical.clone(), canonical.clone());
        if let Some(requested) = requested {
            self.aliases.insert(requested.to_string(), canonical);
        }
    }

    fn remove(&mut self, requested: &str) {
        let canonical = self
            .aliases
            .get(requested)
            .cloned()
            .unwrap_or_else(|| requested.to_string());
        self.listings.remove(&canonical);
        self.aliases.retain(|_, target| target != &canonical);
    }
}

#[derive(Default)]
struct Flights {
    paths: HashSet<String>,
    prefetches: usize,
}

impl Peer {
    pub fn new(host: &str) -> Peer {
        Peer {
            host: host.to_string(),
            cache: Arc::new(Mutex::new(ListingCache::default())),
            flights: Arc::new((Mutex::new(Flights::default()), Condvar::new())),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn list(&self, target: &Target) -> Result<Listing, String> {
        self.load_with(target, true, fetch)
    }

    pub fn refresh(&self, target: &Target) -> Result<Listing, String> {
        self.load_with(target, false, fetch)
    }

    pub fn cached(&self, path: &str) -> Option<Listing> {
        lock(&self.cache).get(path)
    }

    pub fn prefetch(&self, path: String) {
        if self.cached(&path).is_some() {
            return;
        }
        let key = self.canonical(&path);
        if !self.begin_prefetch(&key) {
            return;
        }
        let peer = self.clone();
        let flight = key.clone();
        let started = std::thread::Builder::new()
            .name("hcopy-prefetch".into())
            .spawn(move || {
                if let Ok(listing) = fetch(&peer.host, &Target::Absolute(path.clone())) {
                    peer.remember(Some(&path), &listing);
                }
                peer.finish(&flight, true);
            });
        if started.is_err() {
            self.finish(&key, true);
        }
    }

    fn remember(&self, requested: Option<&str>, listing: &Listing) {
        if listing.missing {
            if let Some(requested) = requested {
                lock(&self.cache).remove(requested);
            }
            return;
        }
        lock(&self.cache).insert(requested, listing);
    }

    fn canonical(&self, path: &str) -> String {
        lock(&self.cache).canonical(path)
    }

    fn begin_prefetch(&self, path: &str) -> bool {
        let mut flights = lock(&self.flights.0);
        if flights.prefetches >= MAX_PREFETCHES || flights.paths.contains(path) {
            return false;
        }
        flights.paths.insert(path.to_string());
        flights.prefetches += 1;
        true
    }

    fn finish(&self, path: &str, prefetch: bool) {
        let mut flights = lock(&self.flights.0);
        flights.paths.remove(path);
        if prefetch {
            flights.prefetches = flights.prefetches.saturating_sub(1);
        }
        self.flights.1.notify_all();
    }

    fn load_with<F>(&self, target: &Target, use_cache: bool, fetcher: F) -> Result<Listing, String>
    where
        F: FnOnce(&str, &Target) -> Result<Listing, String>,
    {
        let requested = match target {
            Target::Absolute(path) => Some(path.as_str()),
            Target::Home(_) => None,
        };
        if use_cache
            && let Some(path) = requested
            && let Some(listing) = self.cached(path)
        {
            return Ok(listing);
        }
        let Some(requested) = requested else {
            let listing = fetcher(&self.host, target)?;
            self.remember(None, &listing);
            return Ok(listing);
        };
        let flight = self.canonical(requested);
        let mut flights = lock(&self.flights.0);
        loop {
            if !flights.paths.contains(&flight) {
                if use_cache && let Some(listing) = self.cached(requested) {
                    return Ok(listing);
                }
                flights.paths.insert(flight.clone());
                break;
            }
            flights = wait(&self.flights.1, flights);
        }
        drop(flights);
        let result = fetcher(&self.host, target);
        if let Ok(listing) = &result {
            self.remember(Some(requested), listing);
        }
        self.finish(&flight, false);
        result
    }

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
        let reply = run(&self.host, &script)?;
        let reply = std::str::from_utf8(&reply)
            .map_err(|_| format!("{}: replied with non-UTF-8 output", self.host))?;
        Ok(match reply.trim() {
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
    parse_reply(host, &run(host, &script(target))?)
}

fn script(target: &Target) -> String {
    format!(
        "target={}; printf 'HOME\\000%s\\000ASK\\000%s\\000' \"$HOME\" \"$target\"; \
         if cd -- \"$target\" 2>/dev/null; then printf 'DIR\\000%s\\000' \"$PWD\"; \
         if find . ! -name . -prune -exec sh -c \
         'for entry do name=${{entry#./}}; if [ -d \"$entry\" ]; then kind=D; else kind=F; fi; \
         printf \"ENTRY\\000%s\\000%s\\000\" \"$kind\" \"$name\"; done' sh {{}} + 2>/dev/null; \
         then printf 'DONE\\000'; else printf 'FAIL\\000directory could not be read\\000'; fi; \
         elif [ -e \"$target\" ] || [ -L \"$target\" ]; \
         then printf 'FAIL\\000directory could not be opened\\000'; else probe=$target; \
         while [ \"$probe\" != / ]; do parent=${{probe%/*}}; \
         if [ \"$parent\" = \"$probe\" ]; then probe=.; else probe=$parent; \
         [ -n \"$probe\" ] || probe=/; fi; \
         if [ -e \"$probe\" ] || [ -L \"$probe\" ]; then \
         if [ -d \"$probe\" ] && [ -x \"$probe\" ]; then printf 'GONE\\000'; \
         else printf 'FAIL\\000directory could not be opened\\000'; fi; break; fi; done; fi; exit 0",
        target.expression()
    )
}

fn run(host: &str, script: &str) -> Result<Vec<u8>, String> {
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
    Ok(output.stdout)
}

fn parse_reply(host: &str, bytes: &[u8]) -> Result<Listing, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| format!("{host}: replied with non-UTF-8 names"))?;
    parse(text)
}

fn parse(text: &str) -> Result<Listing, String> {
    if !text.ends_with('\0') {
        return Err("the other machine sent an unterminated listing record".into());
    }
    let mut listing = Listing::default();
    let mut requested = None;
    let mut directory = false;
    let mut finished = false;
    let mut fields = text.split('\0').peekable();
    while let Some(tag) = fields.next() {
        if tag.is_empty() && fields.peek().is_none() {
            break;
        }
        if finished {
            return Err("the other machine sent records after the listing ended".into());
        }
        match tag {
            "HOME" if listing.home.is_empty() => {
                listing.home = field(&mut fields, "home")?.to_string()
            }
            "ASK" if requested.is_none() => {
                requested = Some(field(&mut fields, "requested directory")?.to_string())
            }
            "DIR" => {
                if directory {
                    return Err("the other machine reported the directory twice".into());
                }
                listing.path = field(&mut fields, "listed directory")?.to_string();
                directory = true;
            }
            "ENTRY" => {
                if !directory {
                    return Err("the other machine reported an entry before its directory".into());
                }
                let directory = match field(&mut fields, "entry kind")? {
                    "D" => true,
                    "F" => false,
                    _ => return Err("the other machine reported an unknown entry kind".into()),
                };
                listing.entries.push(Entry {
                    name: field(&mut fields, "entry name")?.to_string(),
                    directory,
                });
            }
            "DONE" if directory => finished = true,
            "GONE" => {
                if directory || !listing.entries.is_empty() {
                    return Err("the other machine contradicted its directory status".into());
                }
                listing.missing = true;
                finished = true;
            }
            "FAIL" => {
                let reason = field(&mut fields, "listing failure")?;
                return Err(format!(
                    "the other machine could not list the directory: {reason}"
                ));
            }
            _ => return Err("the other machine replied with an unknown listing record".into()),
        }
    }
    if listing.home.is_empty() {
        return Err("the other machine did not say where its home is".into());
    }
    if requested.as_deref().is_none_or(str::is_empty) {
        return Err("the other machine did not say which directory was requested".into());
    }
    if !finished {
        return Err("the other machine did not say which directory it listed".into());
    }
    if listing.missing {
        listing.path = requested.unwrap_or_default();
        listing.entries.clear();
    } else if listing.path.is_empty() {
        return Err("the other machine reported an empty directory path".into());
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

fn field<'a>(fields: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<&'a str, String> {
    fields
        .next()
        .ok_or_else(|| format!("the other machine omitted the {name}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn wait<'a, T>(condition: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condition
        .wait(guard)
        .unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    use super::*;

    fn listed(path: &str, names: &[(&str, bool)]) -> Listing {
        Listing {
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
        }
    }

    #[test]
    fn a_listing_separates_the_directories_from_the_files() {
        let listing = parse(
            "HOME\0/home/f\0ASK\0/home/f/projects\0DIR\0/home/f/projects\0\
             ENTRY\0D\0my-app\0ENTRY\0F\0notes.md\0ENTRY\0D\0src\0DONE\0",
        )
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
        let listing = parse(
            "HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0F\0b.txt\0\
             ENTRY\0D\0Alpha\0ENTRY\0F\0a.txt\0ENTRY\0D\0zeta\0DONE\0",
        )
        .unwrap();
        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "zeta", "a.txt", "b.txt"]);
    }

    #[test]
    fn a_directory_that_is_not_there_keeps_the_requested_path() {
        let listing = parse("HOME\0/home/f\0ASK\0/home/f/new/place\0GONE\0").unwrap();
        assert!(listing.missing);
        assert_eq!(listing.home, "/home/f");
        assert_eq!(listing.path, "/home/f/new/place");
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn an_empty_directory_is_not_a_missing_one() {
        let listing =
            parse("HOME\0/home/f\0ASK\0/home/f/empty\0DIR\0/home/f/empty\0DONE\0").unwrap();
        assert!(!listing.missing);
        assert!(listing.entries.is_empty());
    }

    #[test]
    fn a_reply_that_says_nothing_useful_is_an_error() {
        assert!(parse("").is_err());
        assert!(parse("ASK\0/home/f\0DIR\0/home/f\0DONE\0").is_err());
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0").is_err());
        assert!(parse("HOME\0/home/f\0DIR\0/home/f\0DONE\0").is_err());
    }

    #[test]
    fn whitespace_in_names_survives_the_listing() {
        let listing = parse(
            "HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0D\0my\nnotes\0\
             ENTRY\0F\0a\tfile.txt\0DONE\0",
        )
        .unwrap();
        assert_eq!(listing.entries[0].name, "my\nnotes");
        assert!(listing.entries[0].directory);
        assert_eq!(listing.entries[1].name, "a\tfile.txt");
    }

    #[test]
    fn a_failed_listing_is_not_a_missing_directory() {
        let error =
            parse("HOME\0/home/f\0ASK\0/home/f/private\0FAIL\0directory could not be read\0")
                .unwrap_err();
        assert!(error.contains("could not list"));
        assert!(error.contains("could not be read"));
    }

    #[test]
    fn malformed_records_are_rejected() {
        assert!(
            parse("HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0X\0name\0DONE\0").is_err()
        );
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0WHAT\0").is_err());
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0DIR").is_err());
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0").is_err());
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0ENTRY\0F\0name\0GONE\0").is_err());
        assert!(parse("HOME\0/home/f\0ASK\0/home/f\0GONE\0ENTRY\0F\0name\0").is_err());
    }

    #[test]
    fn non_utf8_identities_have_a_clear_error() {
        let error = parse_reply(
            "archie",
            b"HOME\0/home/f\0ASK\0/home/f\0DIR\0/home/f\0ENTRY\0F\0bad\xff\0DONE\0",
        )
        .unwrap_err();
        assert_eq!(error, "archie: replied with non-UTF-8 names");
    }

    fn peer_knowing(path: &str, names: &[(&str, bool)]) -> Peer {
        let peer = Peer::new("archie");
        peer.remember(Some(path), &listed(path, names));
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
        peer.remember(
            Some("/home/fredrir/new"),
            &Listing {
                path: "/home/fredrir/new".into(),
                home: "/home/fredrir".into(),
                entries: Vec::new(),
                missing: true,
            },
        );
        assert!(!peer.knows_directory("/home/fredrir/new"));
    }

    #[test]
    fn canonical_and_requested_paths_share_one_cache_entry() {
        let peer = Peer::new("archie");
        peer.remember(
            Some("/home/fredrir/projects/../notes"),
            &listed("/home/fredrir/notes", &[("old", false)]),
        );
        peer.remember(
            Some("/home/fredrir/notes"),
            &listed("/home/fredrir/notes", &[("new", false)]),
        );

        let alias = peer.cached("/home/fredrir/projects/../notes").unwrap();
        assert_eq!(alias.entries[0].name, "new");
        assert_eq!(alias.path, "/home/fredrir/notes");
    }

    #[test]
    fn refreshing_bypasses_and_updates_the_cache() {
        let peer = peer_knowing("/home/fredrir/projects", &[("old", false)]);
        let calls = AtomicUsize::new(0);
        let refreshed = peer
            .load_with(
                &Target::Absolute("/home/fredrir/projects".into()),
                false,
                |_, _| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(listed("/home/fredrir/projects", &[("new", false)]))
                },
            )
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(refreshed.entries[0].name, "new");
        assert_eq!(
            peer.cached("/home/fredrir/projects").unwrap().entries[0].name,
            "new"
        );
    }

    #[test]
    fn a_missing_refresh_invalidates_every_alias() {
        let peer = Peer::new("archie");
        peer.remember(
            Some("/home/fredrir/projects/../notes"),
            &listed("/home/fredrir/notes", &[]),
        );
        let missing = Listing {
            path: "/home/fredrir/projects/../notes".into(),
            home: "/home/fredrir".into(),
            entries: Vec::new(),
            missing: true,
        };
        peer.load_with(
            &Target::Absolute("/home/fredrir/projects/../notes".into()),
            false,
            |_, _| Ok(missing),
        )
        .unwrap();

        assert!(peer.cached("/home/fredrir/projects/../notes").is_none());
        assert!(peer.cached("/home/fredrir/notes").is_none());
    }

    #[test]
    fn foreground_requests_share_an_inflight_fetch() {
        let peer = Peer::new("archie");
        let calls = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_peer = peer.clone();
        let first_calls = calls.clone();
        let first = std::thread::spawn(move || {
            first_peer.load_with(
                &Target::Absolute("/home/fredrir/projects".into()),
                true,
                |_, _| {
                    first_calls.fetch_add(1, Ordering::SeqCst);
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(listed("/home/fredrir/projects", &[("shared", true)]))
                },
            )
        });
        started_rx.recv().unwrap();
        let second_peer = peer.clone();
        let second_calls = calls.clone();
        let second = std::thread::spawn(move || {
            second_peer.load_with(
                &Target::Absolute("/home/fredrir/projects".into()),
                true,
                |_, _| {
                    second_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(listed("/home/fredrir/projects", &[("duplicate", true)]))
                },
            )
        });
        release_tx.send(()).unwrap();

        let first = first.join().unwrap().unwrap();
        let second = second.join().unwrap().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first, second);
    }

    #[test]
    fn detached_prefetch_work_is_deduplicated_and_bounded() {
        let peer = Peer::new("archie");
        assert!(peer.begin_prefetch("/one"));
        assert!(!peer.begin_prefetch("/one"));
        assert!(peer.begin_prefetch("/two"));
        assert!(!peer.begin_prefetch("/three"));

        peer.finish("/one", true);
        assert!(peer.begin_prefetch("/three"));
        peer.finish("/two", true);
        peer.finish("/three", true);
    }

    #[test]
    fn the_listing_uses_nul_records_and_reports_listing_failures() {
        let text = script(&Target::Absolute("/home/f".into()));
        assert!(text.contains("printf 'HOME\\000%s\\000ASK\\000%s\\000'"));
        assert!(text.contains("printf \"ENTRY\\000%s\\000%s\\000\""));
        assert!(text.contains("find . ! -name . -prune"));
        assert!(text.contains("FAIL\\000directory could not be read\\000"));
        assert!(text.contains("target='/home/f'"));
        assert!(text.trim_end().ends_with("exit 0"));
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

    #[cfg(unix)]
    #[test]
    fn the_shell_protocol_preserves_hidden_and_whitespace_names_and_directory_links() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("folder")).unwrap();
        std::fs::create_dir(root.path().join(".hidden-dir")).unwrap();
        std::fs::write(root.path().join(".hidden-file"), []).unwrap();
        std::fs::write(root.path().join("line\nbreak\tfile"), []).unwrap();
        symlink("folder", root.path().join("linked-folder")).unwrap();
        let target = Target::Absolute(root.path().to_string_lossy().into_owned());
        let output = Command::new("sh")
            .args(["-c", &script(&target)])
            .output()
            .unwrap();
        assert!(output.status.success());
        let listing = parse_reply("local", &output.stdout).unwrap();

        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == ".hidden-dir" && entry.directory)
        );
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == ".hidden-file" && !entry.directory)
        );
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == "line\nbreak\tfile" && !entry.directory)
        );
        assert!(
            listing
                .entries
                .iter()
                .any(|entry| entry.name == "linked-folder" && entry.directory)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_protocol_keeps_the_exact_missing_request_and_rejects_files() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing\nplace");
        let missing = missing.to_string_lossy().into_owned();
        let output = Command::new("sh")
            .args(["-c", &script(&Target::Absolute(missing.clone()))])
            .output()
            .unwrap();
        let listing = parse_reply("local", &output.stdout).unwrap();
        assert!(listing.missing);
        assert_eq!(listing.path, missing);

        let file = root.path().join("a-file");
        std::fs::write(&file, []).unwrap();
        let output = Command::new("sh")
            .args([
                "-c",
                &script(&Target::Absolute(file.to_string_lossy().into_owned())),
            ])
            .output()
            .unwrap();
        let error = parse_reply("local", &output.stdout).unwrap_err();
        assert!(error.contains("could not be opened"));

        let beneath_file = file.join("child");
        let output = Command::new("sh")
            .args([
                "-c",
                &script(&Target::Absolute(
                    beneath_file.to_string_lossy().into_owned(),
                )),
            ])
            .output()
            .unwrap();
        let error = parse_reply("local", &output.stdout).unwrap_err();
        assert!(error.contains("could not be opened"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_shell_protocol_reports_non_utf8_names_without_loss() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(OsString::from_vec(b"bad\xff".to_vec())),
            [],
        )
        .unwrap();
        let target = Target::Absolute(root.path().to_string_lossy().into_owned());
        let output = Command::new("sh")
            .args(["-c", &script(&target)])
            .output()
            .unwrap();
        let error = parse_reply("local", &output.stdout).unwrap_err();
        assert_eq!(error, "local: replied with non-UTF-8 names");
    }
}
