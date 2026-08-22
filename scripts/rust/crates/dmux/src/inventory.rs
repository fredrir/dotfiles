//! Concurrent backend scans and read-only reconciliation (plan §8.1, §18 P4).
//!
//! Reconciliation joins the durable registry to both live inventories and
//! produces listing rows. It is semantically read-only (plan §2.7): it never
//! allocates identity, changes lifecycle, or mutates a native resource;
//! external resources surface as explicit unmanaged rows.
//!
//! Root-owned (plan §19, W3).

use std::collections::HashMap;

use crate::backend::{InventoryOutcome, NativeSpaceRow};
use crate::model::{Backend, Observation, ServerEpoch};
use crate::registry::{BindingRow, BindingState, SpaceRow};

/// Both providers' typed outcomes for one owner, gathered concurrently
/// (plan §8.2 step 3). Provider-level deadlines bound each scan.
#[derive(Debug)]
pub struct BackendScans {
    pub wez: InventoryOutcome,
    pub tmux: InventoryOutcome,
}

impl BackendScans {
    pub fn get(&self, backend: Backend) -> &InventoryOutcome {
        match backend {
            Backend::Wez => &self.wez,
            Backend::Tmux => &self.tmux,
        }
    }

    pub fn both_determinate(&self) -> bool {
        self.wez.is_determinate() && self.tmux.is_determinate()
    }
}

/// Run both scans concurrently with independent closures (each closure owns
/// its provider call; deadlines are the providers' own).
pub fn scan_both<W, T>(wez: W, tmux: T) -> BackendScans
where
    W: FnOnce() -> InventoryOutcome + Send,
    T: FnOnce() -> InventoryOutcome + Send,
{
    std::thread::scope(|s| {
        let wez_handle = s.spawn(wez);
        let tmux_outcome = tmux();
        let wez_outcome = wez_handle
            .join()
            .unwrap_or_else(|_| InventoryOutcome::Malformed {
                detail: "wez scan thread panicked".into(),
            });
        BackendScans {
            wez: wez_outcome,
            tmux: tmux_outcome,
        }
    })
}

/// The stable detail prefix both adapters emit on `Malformed` when the
/// observed server epoch contradicts the expected one. Recognized here until
/// a first-class variant is added as a coordinated contract change.
pub const EPOCH_CHANGED_PREFIX: &str = "backend_epoch_changed:";

pub fn epoch_changed_detail(outcome: &InventoryOutcome) -> Option<&str> {
    match outcome {
        InventoryOutcome::Malformed { detail } => detail.strip_prefix(EPOCH_CHANGED_PREFIX),
        _ => None,
    }
}

/// How an indeterminate outcome maps onto the durable record's observation
/// column (plan §5.2). Determinate outcomes resolve per-row instead.
pub fn indeterminate_observation(outcome: &InventoryOutcome) -> Option<Observation> {
    Some(match outcome {
        InventoryOutcome::Complete(_) | InventoryOutcome::ServerStopped { .. } => return None,
        InventoryOutcome::VersionMismatch { .. } | InventoryOutcome::ProtocolMismatch { .. } => {
            Observation::Incompatible
        }
        _ => Observation::Unreachable,
    })
}

// ---------------------------------------------------------------------------
// Reconciled listing rows

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRow {
    pub space: SpaceRow,
    pub backend: Backend,
    pub observation: Observation,
    pub groups: u32,
    pub splits: u32,
    pub server_epoch: Option<ServerEpoch>,
    pub native_token: Option<String>,
    pub multi_window: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmanagedRow {
    pub backend: Backend,
    pub native_token: String,
    pub native_name: String,
    pub groups: u32,
    pub splits: u32,
    pub server_epoch: Option<ServerEpoch>,
    pub multi_window: bool,
    /// tmux server with no `@dmux_server_epoch`: listable, children
    /// unaddressable, and `ls` never writes the option (plan §11.2).
    pub unepoched: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconRow {
    Managed(ManagedRow),
    Unmanaged(UnmanagedRow),
}

fn counts(row: &NativeSpaceRow) -> (u32, u32) {
    let groups = row.groups.len() as u32;
    let splits = row.groups.iter().map(|g| g.splits.len() as u32).sum();
    (groups, splits)
}

/// Join durable records to both scans. `records` pairs each Space with its
/// current binding (if any); `backend_of` maps a record to its backend via
/// the registry's backend_instances (v1: one instance per backend).
///
/// Rules:
/// - deleted/aborted records never appear;
/// - a bound record found in a Complete scan is `live` with live counts;
/// - bound but missing from a Complete scan is `absent`;
/// - `ServerStopped` scan → `stopped`; indeterminate scans map via
///   [`indeterminate_observation`] and yield zero live counts;
/// - Complete-scan rows bound to no current binding become unmanaged rows —
///   a failed or partial inventory is NOT an empty inventory, so
///   indeterminate scans contribute no unmanaged rows (plan §2.10).
pub fn reconcile(
    records: &[(SpaceRow, Option<BindingRow>)],
    backend_of: impl Fn(&SpaceRow) -> Option<Backend>,
    scans: &BackendScans,
) -> Vec<ReconRow> {
    let mut rows = Vec::new();
    // Native tokens claimed by a current binding, per backend.
    let mut claimed: HashMap<(Backend, &str), ()> = HashMap::new();
    for (space, binding) in records {
        if space.lifecycle.is_terminal() {
            continue;
        }
        if let (Some(backend), Some(b)) = (backend_of(space), binding.as_ref())
            && b.binding_state == BindingState::Current
        {
            claimed.insert((backend, b.native_token.as_str()), ());
        }
    }

    for (space, binding) in records {
        if space.lifecycle.is_terminal() {
            continue;
        }
        let Some(backend) = backend_of(space) else {
            continue;
        };
        let current = binding
            .as_ref()
            .filter(|b| b.binding_state == BindingState::Current);
        let scan = scans.get(backend);
        let (observation, groups, splits, epoch, multi_window) = match (scan, current) {
            (InventoryOutcome::Complete(inv), Some(b)) => {
                match inv.rows.iter().find(|r| r.native_token == b.native_token) {
                    Some(row) => {
                        let (g, s) = counts(row);
                        (Observation::Live, g, s, inv.server_epoch, row.multi_window)
                    }
                    None => (Observation::Absent, 0, 0, inv.server_epoch, false),
                }
            }
            (InventoryOutcome::Complete(inv), None) => {
                (Observation::Absent, 0, 0, inv.server_epoch, false)
            }
            (InventoryOutcome::ServerStopped { .. }, _) => {
                (Observation::Stopped, 0, 0, None, false)
            }
            (other, _) => (
                indeterminate_observation(other).unwrap_or(Observation::Unreachable),
                0,
                0,
                None,
                false,
            ),
        };
        rows.push(ReconRow::Managed(ManagedRow {
            space: space.clone(),
            backend,
            observation,
            groups,
            splits,
            server_epoch: epoch,
            native_token: current.map(|b| b.native_token.clone()),
            multi_window,
        }));
    }

    for backend in [Backend::Wez, Backend::Tmux] {
        if let InventoryOutcome::Complete(inv) = scans.get(backend) {
            for row in &inv.rows {
                if claimed.contains_key(&(backend, row.native_token.as_str())) {
                    continue;
                }
                let (groups, splits) = counts(row);
                rows.push(ReconRow::Unmanaged(UnmanagedRow {
                    backend,
                    native_token: row.native_token.clone(),
                    native_name: row.native_name.clone(),
                    groups,
                    splits,
                    server_epoch: inv.server_epoch,
                    multi_window: row.multi_window,
                    unepoched: backend == Backend::Tmux && inv.server_epoch.is_none(),
                }));
            }
        }
    }

    // Managed first by permanent SpaceNo (never a transient row index —
    // plan §16.1), then unmanaged by backend/name.
    rows.sort_by(|a, b| match (a, b) {
        (ReconRow::Managed(x), ReconRow::Managed(y)) => x.space.space_no.cmp(&y.space.space_no),
        (ReconRow::Managed(_), ReconRow::Unmanaged(_)) => std::cmp::Ordering::Less,
        (ReconRow::Unmanaged(_), ReconRow::Managed(_)) => std::cmp::Ordering::Greater,
        (ReconRow::Unmanaged(x), ReconRow::Unmanaged(y)) => {
            (x.backend.as_str(), &x.native_name).cmp(&(y.backend.as_str(), &y.native_name))
        }
    });
    rows
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use uuid::Uuid;

    use super::*;
    use crate::backend::{NativeGroupRow, NativeInventory, NativeSplitRow};
    use crate::model::{
        BackendInstanceUid, Health, HostUid, Lifecycle, ProviderHandle, SpaceNo, SpaceUid,
    };
    use crate::registry::NativeKind;

    fn space(no: u64, name: &str, lifecycle: Lifecycle) -> SpaceRow {
        SpaceRow {
            space_uid: SpaceUid(Uuid::from_u128(no as u128)),
            owner: HostUid(Uuid::nil()),
            space_no: SpaceNo(NonZeroU64::new(no).unwrap()),
            backend_instance: BackendInstanceUid(Uuid::nil()),
            logical_name: name.into(),
            lifecycle,
            health: Health::Healthy,
            created_at: String::new(),
            updated_at: String::new(),
            deleted_at: None,
        }
    }

    fn binding(no: u64, token: &str) -> BindingRow {
        BindingRow {
            binding_id: no as i64,
            space_uid: SpaceUid(Uuid::from_u128(no as u128)),
            native_token: token.into(),
            native_kind: NativeKind::TmuxSessionId,
            binding_state: BindingState::Current,
            observation: Observation::Live,
            server_epoch: None,
        }
    }

    fn native(token: &str, name: &str, groups: usize, splits_per: usize) -> NativeSpaceRow {
        NativeSpaceRow {
            native_token: token.into(),
            native_name: name.into(),
            multi_window: false,
            groups: (0..groups)
                .map(|g| NativeGroupRow {
                    handle: ProviderHandle::Tx(g as u64),
                    title: None,
                    splits: (0..splits_per)
                        .map(|p| NativeSplitRow {
                            handle: ProviderHandle::Tx((g * 10 + p) as u64),
                            title: None,
                            cwd: None,
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn complete(rows: Vec<NativeSpaceRow>, epoch: Option<ServerEpoch>) -> InventoryOutcome {
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: epoch,
            rows,
        })
    }

    #[test]
    fn live_absent_stopped_and_unmanaged_rows() {
        let records = vec![
            (space(1, "alpha", Lifecycle::Active), Some(binding(1, "$0"))),
            (space(2, "gone", Lifecycle::Active), Some(binding(2, "$9"))),
            (space(3, "dead", Lifecycle::Deleted), None),
        ];
        let scans = BackendScans {
            wez: InventoryOutcome::ServerStopped {
                detail: "no socket".into(),
            },
            tmux: complete(
                vec![native("$0", "alpha", 2, 2), native("$5", "stray", 1, 1)],
                None,
            ),
        };
        let rows = reconcile(&records, |_| Some(Backend::Tmux), &scans);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let ReconRow::Managed(m1) = &rows[0] else {
            panic!()
        };
        assert_eq!(
            (m1.observation, m1.groups, m1.splits),
            (Observation::Live, 2, 4)
        );
        let ReconRow::Managed(m2) = &rows[1] else {
            panic!()
        };
        assert_eq!(m2.observation, Observation::Absent);
        let ReconRow::Unmanaged(u) = &rows[2] else {
            panic!()
        };
        assert_eq!(u.native_name, "stray");
        assert!(
            u.unepoched,
            "tmux without epoch lists as unmanaged:unepoched"
        );
    }

    #[test]
    fn stopped_backend_marks_records_stopped_and_lists_nothing_unmanaged() {
        let records = vec![(
            space(1, "w", Lifecycle::Active),
            Some(binding(1, "dmux:h:s")),
        )];
        let scans = BackendScans {
            wez: InventoryOutcome::ServerStopped {
                detail: String::new(),
            },
            tmux: InventoryOutcome::Unreachable {
                detail: String::new(),
            },
        };
        let rows = reconcile(&records, |_| Some(Backend::Wez), &scans);
        assert_eq!(rows.len(), 1);
        let ReconRow::Managed(m) = &rows[0] else {
            panic!()
        };
        assert_eq!(m.observation, Observation::Stopped);
    }

    #[test]
    fn indeterminate_scan_contributes_no_unmanaged_rows_and_maps_observation() {
        let records = vec![
            (space(1, "a", Lifecycle::Active), Some(binding(1, "$0"))),
            (
                space(2, "b", Lifecycle::Active),
                Some(binding(2, "dmux:h:s")),
            ),
        ];
        let scans = BackendScans {
            wez: InventoryOutcome::VersionMismatch {
                detail: String::new(),
            },
            tmux: InventoryOutcome::Timeout {
                detail: String::new(),
            },
        };
        let by_no = |s: &SpaceRow| {
            Some(if s.space_no.get() == 1 {
                Backend::Tmux
            } else {
                Backend::Wez
            })
        };
        let rows = reconcile(&records, by_no, &scans);
        assert_eq!(rows.len(), 2);
        let ReconRow::Managed(m1) = &rows[0] else {
            panic!()
        };
        assert_eq!(m1.observation, Observation::Unreachable);
        let ReconRow::Managed(m2) = &rows[1] else {
            panic!()
        };
        assert_eq!(m2.observation, Observation::Incompatible);
    }

    #[test]
    fn epoch_changed_prefix_is_recognized() {
        let o = InventoryOutcome::Malformed {
            detail: "backend_epoch_changed: expected x observed y".into(),
        };
        assert_eq!(epoch_changed_detail(&o), Some(" expected x observed y"));
        assert_eq!(
            epoch_changed_detail(&InventoryOutcome::Malformed {
                detail: "other".into()
            }),
            None
        );
    }

    #[test]
    fn scan_both_runs_and_survives_panic() {
        let scans = scan_both(
            || panic!("wez exploded"),
            || InventoryOutcome::ServerStopped { detail: "s".into() },
        );
        assert!(matches!(scans.wez, InventoryOutcome::Malformed { .. }));
        assert!(matches!(scans.tmux, InventoryOutcome::ServerStopped { .. }));
        assert!(!scans.both_determinate());
    }
}
