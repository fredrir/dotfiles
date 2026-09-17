use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use hostkit::{Host, Route};
use workstation::Style;
use workstation::path::home_relative_in;

use crate::browse::{Browser, Chosen};
use crate::cli::{Direction, Request};
use crate::place::{self, Local};
use crate::remote::{Kind, Listing, Peer, Target};
use crate::report;
use crate::transfer::{self, Plan};

struct Session {
    this: Host,
    peer: Peer,
    home: PathBuf,
    style: Style,
    route: Option<Route>,
    remote_home: String,
}

pub fn main(request: Request) -> Result<(), String> {
    let this = Host::this()?;
    let target = request.target.as_deref().unwrap_or(this.peer().name());
    if target == this.name() {
        return Err("cannot target the local machine".to_string());
    }
    let peer = Peer::new(target);
    let home = place::home()?;
    let anchor = anchor(&request, &home)?;

    let asked = peer.host().to_string();
    let is_peer = Host::from_name(&asked).is_ok();
    let route = if is_peer {
        Some(std::thread::spawn(move || hostkit::ssh::resolved(&asked)))
    } else {
        None
    };

    let below = anchor.as_ref().map(Local::parent).unwrap_or_default();
    let listing = peer.list(&Target::Home(below))?;

    let session = Session {
        this,
        peer,
        home,
        style: Style::for_stdout(),
        route: route.and_then(|handle| handle.join().ok().flatten()),
        remote_home: listing.home.clone(),
    };

    let start = session.start(request.direction, anchor.as_ref(), &listing);
    let Some(remote) = session.decide(&request, anchor.as_ref(), &start)? else {
        return Ok(());
    };

    let plan = session.plan(&request, anchor.as_ref(), &remote)?;
    session.verify(&request, &plan)?;
    if !session.confirmed(&request, &plan)? {
        return Ok(());
    }
    session.carry(&request, &plan)
}

fn anchor(request: &Request, home: &Path) -> Result<Option<Local>, String> {
    let named = request.path.as_deref();
    if request.direction == Direction::Pull {
        if named.is_none() && request.remote.is_some() {
            return Ok(None);
        }
        if place::absolute(named.unwrap_or("."), home)? == home {
            return Ok(None);
        }
    }
    let local = place::resolve(named.unwrap_or("."), home)?;
    if request.direction == Direction::Push && std::fs::symlink_metadata(&local.absolute).is_err() {
        return Err(format!(
            "local source does not exist: {}",
            local.absolute.display()
        ));
    }
    Ok(Some(local))
}

struct Start {
    directory: String,
    name: Option<String>,
    mirror: Option<String>,
}

fn name(anchor: Option<&Local>) -> Result<&str, String> {
    anchor
        .map(|local| local.name.as_str())
        .ok_or_else(|| "nothing named to copy".to_string())
}

impl Session {
    fn start(&self, direction: Direction, anchor: Option<&Local>, listing: &Listing) -> Start {
        let Some(local) = anchor else {
            return Start {
                directory: self.remote_home.clone(),
                name: None,
                mirror: None,
            };
        };
        let parent = local.parent();
        let mirror = place::join(&self.remote_home, &local.relative);
        let remote_file = listing
            .entries
            .iter()
            .find(|entry| entry.name == local.name)
            .is_some_and(|entry| !entry.directory);
        let open_mirror = direction == Direction::Push && local.absolute.is_dir() && !remote_file;
        Start {
            directory: match open_mirror {
                true => mirror.clone(),
                false => match parent.is_empty() {
                    true => self.remote_home.clone(),
                    false => place::join(&self.remote_home, &parent),
                },
            },
            name: Some(local.name.clone()),
            mirror: Some(mirror),
        }
    }

    fn decide(
        &self,
        request: &Request,
        anchor: Option<&Local>,
        start: &Start,
    ) -> Result<Option<String>, String> {
        if let Some(given) = &request.remote {
            let expanded = place::expand_remote(given, &self.remote_home);
            return Ok(Some(match request.direction {
                // `--to` names a directory to land in, the way the browser does.
                Direction::Push => place::join(&expanded, name(anchor)?),
                Direction::Pull => expanded,
            }));
        }
        if request.yes {
            let mirror = start
                .mirror
                .clone()
                .ok_or_else(|| "nothing named to copy; name a path or pass --from".to_string())?;
            return Ok(Some(mirror));
        }

        let here =
            std::env::current_dir().map_err(|error| format!("this directory is gone: {error}"))?;
        let browser = Browser {
            direction: request.direction,
            peer: &self.peer,
            style: &self.style,
            this: self.this.name(),
            route: self.route,
            name: start.name.clone(),
            local_display: anchor.map(Local::display).unwrap_or_default(),
            start: start.directory.clone(),
            mirror: start.mirror.clone(),
            remote_home: self.remote_home.clone(),
            home: self.home.clone(),
            here,
        };
        match browser.choose()? {
            Chosen::Picked(path) => Ok(Some(path)),
            Chosen::Cancelled => Ok(None),
            Chosen::Interrupted => Err("interrupted".to_string()),
            Chosen::Unavailable => Err(format!(
                "no terminal to choose in; pass --yes for the mirrored path, \
                 or {} to name one",
                match request.direction {
                    Direction::Push => "--to",
                    Direction::Pull => "--from",
                }
            )),
        }
    }

    fn plan(
        &self,
        request: &Request,
        anchor: Option<&Local>,
        remote: &str,
    ) -> Result<Plan, String> {
        let (local, local_display) = match request.direction {
            Direction::Push => {
                let local = anchor.ok_or_else(|| "nothing named to copy".to_string())?;
                (local.absolute.clone(), local.display())
            }
            Direction::Pull => self.landing(remote)?,
        };
        Ok(Plan {
            direction: request.direction,
            host: self.peer.host().to_string(),
            local,
            local_display,
            remote: remote.to_string(),
            remote_display: home_relative_in(Path::new(remote), Path::new(&self.remote_home)),
            route: self.route,
            dry_run: request.dry_run,
            checksum: request.checksum,
            all: request.all,
        })
    }

    fn landing(&self, remote: &str) -> Result<(PathBuf, String), String> {
        let here =
            std::env::current_dir().map_err(|error| format!("this directory is gone: {error}"))?;
        place::landing(remote, &self.remote_home, &self.home, &here)
    }

    fn verify(&self, request: &Request, plan: &Plan) -> Result<(), String> {
        if request.direction != Direction::Pull || self.peer.knows_entry(&plan.remote) {
            return Ok(());
        }
        match self.peer.kind(&plan.remote)? {
            Kind::Missing => Err(format!(
                "{}:{} does not exist",
                plan.host, plan.remote_display
            )),
            _ => Ok(()),
        }
    }

    fn confirmed(&self, request: &Request, plan: &Plan) -> Result<bool, String> {
        let style = &self.style;
        println!(
            "{}",
            report::header(
                style,
                plan.direction,
                self.this.name(),
                &plan.host,
                plan.route
            )
        );
        println!();
        for line in report::endpoints(style, plan, self.this.name()) {
            println!("{line}");
        }
        println!();

        if request.yes || request.dry_run || request.remote.is_none() {
            return Ok(true);
        }
        match workstation::confirm("  Continue? [Y/n] ") {
            Some(true) => Ok(true),
            Some(false) => {
                println!("  {}", style.dim("cancelled"));
                Ok(false)
            }
            None => Err("nothing to read an answer from; pass --yes to skip the question".into()),
        }
    }

    fn carry(&self, request: &Request, plan: &Plan) -> Result<(), String> {
        let style = &self.style;
        if !plan.dry_run {
            match plan.direction {
                Direction::Push => {
                    let parent = place::parent_of(&plan.remote);
                    if !self.peer.knows_directory(parent) {
                        self.peer.make_directory(parent)?;
                    }
                }
                Direction::Pull => std::fs::create_dir_all(plan.local_parent())
                    .map_err(|error| format!("{}: {error}", plan.local_parent().display()))?,
            }
        }

        let live = io::stdout().is_terminal() && !request.verbose;
        let mut progress = ui_progress::Reporter::new(io::stdout());
        let outcome = transfer::run(plan, |running| {
            if live {
                let _ = progress.update(&report::progress(style, running));
            }
        })?;
        let _ = progress.finish();

        if request.verbose {
            for line in &outcome.lines {
                println!("  {}", style.dim(line));
            }
        }
        println!("{}", report::summary(style, plan, &outcome));
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/run_tests.rs"]
mod tests;
