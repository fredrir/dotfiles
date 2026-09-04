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
#[path = "../tests/unit/remote_tests.rs"]
mod tests;
