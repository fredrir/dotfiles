use std::thread;

use hostkit::{Host, Route};

use super::Options;
use super::detect;
use super::model::{Context, RouteState, Snapshot, TargetInfo};
use super::ssh_info;

pub fn snapshot(options: &Options) -> Result<Snapshot, String> {
    let this = Host::this()?;
    let peer = this.peer();
    let detected = detect::session(this);
    let explicit = !options.targets.is_empty();
    let context = if explicit {
        Context::Query
    } else {
        match detected.session.as_ref() {
            Some(session) if session.tls => Context::Tls,
            Some(_) => Context::Ssh,
            None => Context::Local,
        }
    };
    let need_routes = !explicit && (detected.session.is_none() || options.diagnostics);
    let target_names = if explicit {
        options.targets.clone()
    } else if options.diagnostics {
        vec![peer.name().to_string()]
    } else {
        Vec::new()
    };

    let (routes, targets) = thread::scope(|scope| {
        let route_worker = need_routes.then(|| {
            scope.spawn(|| {
                Ok::<_, String>(
                    hostkit::snapshot::probe(this, peer, 22)
                        .routes
                        .into_iter()
                        .map(|probe| RouteState {
                            route: probe.route,
                            local: probe.local_address,
                            peer: probe.peer_address,
                            available: probe.up,
                            elapsed: probe.elapsed,
                            error: probe.error,
                        })
                        .collect::<Vec<_>>(),
                )
            })
        });
        let target_worker = (!target_names.is_empty())
            .then(|| scope.spawn(|| resolve_targets(&target_names, options.diagnostics)));
        let routes = route_worker
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "route probe worker panicked".to_string())?
            })
            .transpose()?
            .unwrap_or_default();
        let targets = target_worker
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| "SSH resolution worker panicked".to_string())
            })
            .transpose()?
            .unwrap_or_default();
        Ok::<_, String>((routes, targets))
    })?;

    let preferred = Route::every().into_iter().find(|route| {
        routes
            .iter()
            .any(|state| state.route == *route && state.available)
    });
    Ok(Snapshot {
        context,
        this,
        peer,
        session: (!explicit).then_some(detected.session).flatten(),
        preferred,
        routes,
        targets,
        warnings: detected.warnings,
    })
}

fn resolve_targets(names: &[String], diagnostics: bool) -> Vec<TargetInfo> {
    thread::scope(|scope| {
        let workers: Vec<_> = names
            .iter()
            .map(|name| {
                let name = name.clone();
                scope.spawn(move || ssh_info::resolve(&name, diagnostics))
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker.join().unwrap_or_else(|_| TargetInfo {
                    input: "unknown".into(),
                    hostname: String::new(),
                    route: None,
                    bound: None,
                    proxy: None,
                    user: None,
                    port: None,
                    master: Default::default(),
                    error: Some("SSH resolution worker panicked".into()),
                })
            })
            .collect()
    })
}
