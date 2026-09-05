//! Goal checkpoints use public RPC state, never the source credential or SQLite stores.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::rpc::Codex;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GoalSnapshot {
    pub objective: String,
    /// Desired status after takeover. A source goal paused by us retains `active` here.
    pub status: String,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: i64,
}

impl GoalSnapshot {
    fn same_goal(&self, other: &Self) -> bool {
        self.objective == other.objective && self.created_at == other.created_at
    }
    pub fn remaining_budget(&self) -> Option<i64> {
        self.token_budget
            .map(|budget| budget.saturating_sub(self.tokens_used).max(0))
    }

    fn validate(&self) -> Result<(), String> {
        if self.objective.trim().is_empty() || self.objective.chars().count() > 4000 {
            return Err("Codex goal objective is empty or exceeds 4000 characters".into());
        }
        if !matches!(
            self.status.as_str(),
            "active" | "paused" | "blocked" | "usageLimited" | "budgetLimited" | "complete"
        ) {
            return Err("Codex goal has an unsupported status".into());
        }
        if self.tokens_used < 0
            || self.time_used_seconds < 0
            || self.token_budget.is_some_and(|budget| budget < 0)
        {
            return Err("Codex goal has invalid usage or budget".into());
        }
        Ok(())
    }
}

type Request<'a> = dyn FnMut(&str, Value) -> Result<Value, String> + 'a;

/// Persist this result in the source receipt BEFORE calling pause().
pub(super) fn capture(server: &mut Codex, root: &str) -> Result<Option<GoalSnapshot>, String> {
    capture_using(&mut |method, params| server.request(method, params), root)
}

pub(super) fn pause(server: &mut Codex, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    pause_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

/// Re-read after the current turn drains; never resurrect a goal it completed or cleared.
pub(super) fn refresh(
    server: &mut Codex,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<Option<GoalSnapshot>, String> {
    refresh_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

pub(super) fn stage(server: &mut Codex, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    stage_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

/// Only call after the source execution owner has stopped and activation is durable.
pub(super) fn activate(server: &mut Codex, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    activate_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

/// Source rollback changes status only: its existing budget and counters stay intact.
pub(super) fn rollback(server: &mut Codex, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    rollback_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

/// Explicit recovery on the committed destination, whose native goal has a new lifecycle.
pub(super) fn recover_destination(
    server: &mut Codex,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<(), String> {
    recover_destination_using(
        &mut |method, params| server.request(method, params),
        root,
        goal,
    )
}

fn read(request: &mut Request<'_>, root: &str) -> Result<Option<GoalSnapshot>, String> {
    let result = request("thread/goal/get", json!({"threadId":root}))?;
    let value = result.get("goal").ok_or("Codex omitted goal state")?;
    if value.is_null() {
        return Ok(None);
    }
    if value.get("threadId").and_then(Value::as_str) != Some(root) {
        return Err("Codex goal belongs to another thread".into());
    }
    let goal: GoalSnapshot = serde_json::from_value(value.clone()).map_err(super::error)?;
    goal.validate()?;
    Ok(Some(goal))
}

fn ensure_descendants(request: &mut Request<'_>, root: &str) -> Result<(), String> {
    let loaded = request("thread/loaded/list", json!({}))?;
    let ids = loaded
        .get("data")
        .and_then(Value::as_array)
        .ok_or("Codex omitted loaded threads")?;
    if !ids.iter().any(|id| id.as_str() == Some(root)) {
        return Err("managed goal thread is not loaded".into());
    }
    for id in ids {
        let id = id
            .as_str()
            .ok_or("Codex returned an invalid loaded thread")?;
        if id != root && read(request, id)?.is_some_and(|goal| goal.status == "active") {
            return Err("active descendant goals cannot be handed off yet; pause or complete them before moving the root session".into());
        }
    }
    Ok(())
}

fn capture_using(request: &mut Request<'_>, root: &str) -> Result<Option<GoalSnapshot>, String> {
    ensure_descendants(request, root)?;
    read(request, root)
}

fn status(request: &mut Request<'_>, root: &str, expected: &str) -> Result<(), String> {
    let response = request(
        "thread/goal/set",
        json!({"threadId":root,"status":expected}),
    )?;
    // Activation can immediately start or finish work. Its acknowledgement, not
    // a later status sample, proves the requested transition was accepted.
    if response.pointer("/goal/threadId").and_then(Value::as_str) != Some(root)
        || response.pointer("/goal/status").and_then(Value::as_str) != Some(expected)
    {
        return Err(format!("Codex did not confirm goal status {expected}"));
    }
    Ok(())
}

fn pause_using(request: &mut Request<'_>, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    goal.validate()?;
    ensure_descendants(request, root)?;
    if goal.status == "active" {
        // The goal may have completed between capture and this call. Never overwrite it.
        if let Some(current) = read(request, root)?
            && current.status == "active"
        {
            if !current.same_goal(goal) {
                return Err("root goal changed before pause; retry the handoff".into());
            }
            status(request, root, "paused")?;
        }
    }
    Ok(())
}

fn refresh_using(
    request: &mut Request<'_>,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<Option<GoalSnapshot>, String> {
    goal.validate()?;
    ensure_descendants(request, root)?;
    let Some(mut current) = read(request, root)? else {
        return Ok(None);
    };
    if current.status == "active" {
        return Err(
            "Codex goal became active while the handoff was draining; pause and retry".into(),
        );
    }
    if current.status == "paused" && goal.status == "active" && current.same_goal(goal) {
        current.status = "active".into();
    }
    Ok(Some(current))
}

fn stage_using(request: &mut Request<'_>, root: &str, goal: &GoalSnapshot) -> Result<(), String> {
    goal.validate()?;
    if goal.remaining_budget() == Some(0) {
        return Err("goal has no remaining token budget; Codex cannot import a zero budget, so this goal stays on the source".into());
    }
    if read(request, root)?.is_some_and(|current| current.status == "active") {
        return Err("destination goal is already active before takeover".into());
    }
    request(
        "thread/goal/set",
        json!({"threadId":root,"objective":goal.objective,"status":"paused","tokenBudget":goal.remaining_budget()}),
    )?;
    let staged = read(request, root)?.ok_or("destination did not retain staged goal")?;
    if staged.status != "paused"
        || staged.objective != goal.objective
        || staged.token_budget != goal.remaining_budget()
        || staged.tokens_used != 0
    {
        return Err(
            "destination goal does not match the paused checkpoint and remaining budget".into(),
        );
    }
    Ok(())
}

fn activate_using(
    request: &mut Request<'_>,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<(), String> {
    goal.validate()?;
    let current = read(request, root)?.ok_or("destination lost its staged goal")?;
    if current.status != "paused"
        || current.objective != goal.objective
        || current.token_budget != goal.remaining_budget()
        || current.tokens_used != 0
    {
        return Err("destination goal changed before activation".into());
    }
    let desired = if goal.status == "active" && goal.remaining_budget() == Some(0) {
        "budgetLimited"
    } else {
        &goal.status
    };
    status(request, root, desired)
}

fn rollback_using(
    request: &mut Request<'_>,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<(), String> {
    goal.validate()?;
    if goal.status == "active"
        && let Some(current) = read(request, root)?
        && current.status == "paused"
        && current.same_goal(goal)
    {
        let desired = if current.remaining_budget() == Some(0) {
            "budgetLimited"
        } else {
            "active"
        };
        status(request, root, desired)?;
    }
    Ok(())
}

fn recover_destination_using(
    request: &mut Request<'_>,
    root: &str,
    goal: &GoalSnapshot,
) -> Result<(), String> {
    goal.validate()?;
    let Some(current) = read(request, root)? else {
        return Err("committed destination goal is missing; inspect it before resuming".into());
    };
    if current.objective != goal.objective || current.token_budget != goal.remaining_budget() {
        return Err("committed destination goal was replaced; refusing to overwrite it".into());
    }
    if current.status == "paused" && current.tokens_used == 0 && current.time_used_seconds == 0 {
        activate_using(request, root, goal)?;
    }
    // Running, completed, limited, blocked, or already-used paused goals represent
    // destination progress. Recovery must not replay the original source intent.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ROOT: &str = "root";

    fn goal(status: &str, used: i64, budget: Option<i64>) -> GoalSnapshot {
        GoalSnapshot {
            objective: "Ship the tmux environment".into(),
            status: status.into(),
            token_budget: budget,
            tokens_used: used,
            time_used_seconds: 42,
            created_at: 123,
        }
    }

    #[derive(Default)]
    struct Fake {
        goals: HashMap<String, GoalSnapshot>,
        loaded: Vec<String>,
        writes: Vec<Value>,
    }

    impl Fake {
        fn with(goal: GoalSnapshot) -> Self {
            Self {
                goals: HashMap::from([(ROOT.into(), goal)]),
                loaded: vec![ROOT.into()],
                writes: vec![],
            }
        }
        fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
            if method == "thread/loaded/list" {
                return Ok(json!({"data":self.loaded}));
            }
            let id = params["threadId"].as_str().unwrap();
            if method == "thread/goal/set" {
                self.writes.push(params.clone());
                if let Some(objective) = params.get("objective").and_then(Value::as_str) {
                    let mut next = goal(
                        "paused",
                        0,
                        params.get("tokenBudget").and_then(Value::as_i64),
                    );
                    next.objective = objective.into();
                    next.time_used_seconds = 0;
                    self.goals.insert(id.into(), next);
                }
                if let Some(status) = params.get("status").and_then(Value::as_str) {
                    self.goals.get_mut(id).ok_or("no goal")?.status = status.into();
                }
            }
            let Some(goal) = self.goals.get(id) else {
                return Ok(json!({"goal":null}));
            };
            let mut value = serde_json::to_value(goal).unwrap();
            value["threadId"] = json!(id);
            Ok(json!({"goal":value}))
        }
    }

    #[test]
    fn capture_refuses_active_child_before_any_pause() {
        let mut fake = Fake::with(goal("active", 100, Some(1000)));
        fake.loaded.push("child".into());
        fake.goals.insert("child".into(), goal("active", 1, None));
        assert!(
            capture_using(&mut |m, p| fake.request(m, p), ROOT)
                .unwrap_err()
                .contains("descendant")
        );
        assert!(fake.writes.is_empty());
    }

    #[test]
    fn active_goal_is_paused_and_final_usage_is_captured_without_reset() {
        let mut fake = Fake::with(goal("active", 100, Some(1000)));
        let original = capture_using(&mut |m, p| fake.request(m, p), ROOT)
            .unwrap()
            .unwrap();
        pause_using(&mut |m, p| fake.request(m, p), ROOT, &original).unwrap();
        assert_eq!(
            fake.writes,
            vec![json!({"threadId":ROOT,"status":"paused"})]
        );
        fake.goals.get_mut(ROOT).unwrap().tokens_used = 175;
        let snapshot = refresh_using(&mut |m, p| fake.request(m, p), ROOT, &original)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status, "active");
        assert_eq!(snapshot.remaining_budget(), Some(825));
        assert_eq!(snapshot.time_used_seconds, 42);
    }

    #[test]
    fn completion_or_clear_during_drain_is_not_resurrected() {
        let original = goal("active", 100, None);
        let mut fake = Fake::with(goal("complete", 200, None));
        pause_using(&mut |m, p| fake.request(m, p), ROOT, &original).unwrap();
        let snapshot = refresh_using(&mut |m, p| fake.request(m, p), ROOT, &original)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.status, "complete");
        rollback_using(&mut |m, p| fake.request(m, p), ROOT, &original).unwrap();
        assert!(fake.writes.is_empty());
        fake.goals.clear();
        assert!(
            refresh_using(&mut |m, p| fake.request(m, p), ROOT, &original)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn staging_carries_remaining_allowance_and_never_starts_execution() {
        let snapshot = goal("active", 250, Some(1000));
        let mut fake = Fake::default();
        stage_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).unwrap();
        assert_eq!(fake.goals[ROOT].status, "paused");
        assert_eq!(fake.goals[ROOT].token_budget, Some(750));
        assert_eq!(fake.goals[ROOT].tokens_used, 0);
        activate_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).unwrap();
        assert_eq!(fake.goals[ROOT].status, "active");
        assert_eq!(fake.writes[1], json!({"threadId":ROOT,"status":"active"}));
    }

    #[test]
    fn exhausted_budget_is_never_reactivated() {
        let snapshot = goal("active", 1100, Some(1000));
        assert_eq!(snapshot.remaining_budget(), Some(0));
        let mut fake = Fake::default();
        assert!(
            stage_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot)
                .unwrap_err()
                .contains("zero budget")
        );
        assert!(fake.writes.is_empty());
        let mut source = Fake::with(goal("paused", 1100, Some(1000)));
        rollback_using(&mut |m, p| source.request(m, p), ROOT, &snapshot).unwrap();
        assert_eq!(source.goals[ROOT].status, "budgetLimited");
    }

    #[test]
    fn rollback_preserves_budget_counters_and_originally_paused_state() {
        let original = goal("active", 100, Some(1000));
        let mut source = Fake::with(goal("paused", 250, Some(1000)));
        rollback_using(&mut |m, p| source.request(m, p), ROOT, &original).unwrap();
        assert_eq!(
            source.writes,
            vec![json!({"threadId":ROOT,"status":"active"})]
        );
        assert_eq!(source.goals[ROOT].tokens_used, 250);
        assert_eq!(source.goals[ROOT].token_budget, Some(1000));
        source.writes.clear();
        rollback_using(
            &mut |m, p| source.request(m, p),
            ROOT,
            &goal("paused", 250, Some(1000)),
        )
        .unwrap();
        assert!(source.writes.is_empty());
    }

    #[test]
    fn replacing_a_goal_during_drain_never_inherits_active_intent() {
        let original = goal("active", 100, Some(1000));
        for replace_objective in [false, true] {
            let mut replacement = goal("paused", 0, Some(500));
            if replace_objective {
                replacement.objective = "A deliberately paused different task".into();
            } else {
                replacement.created_at += 1;
            }
            let mut fake = Fake::with(replacement);
            let refreshed = refresh_using(&mut |m, p| fake.request(m, p), ROOT, &original)
                .unwrap()
                .unwrap();
            assert_eq!(refreshed.status, "paused");
            rollback_using(&mut |m, p| fake.request(m, p), ROOT, &original).unwrap();
            assert!(fake.writes.is_empty());
            fake.goals.get_mut(ROOT).unwrap().status = "active".into();
            assert!(pause_using(&mut |m, p| fake.request(m, p), ROOT, &original).is_err());
            assert!(fake.writes.is_empty());
        }
    }

    #[test]
    fn destination_recovery_resumes_staged_goal_but_never_replays_progress() {
        let snapshot = goal("active", 200, Some(1000));
        let mut fake = Fake::default();
        stage_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).unwrap();
        fake.goals.get_mut(ROOT).unwrap().created_at += 1;
        recover_destination_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).unwrap();
        assert_eq!(fake.goals[ROOT].status, "active");
        for status in ["active", "complete", "blocked", "paused"] {
            let current = fake.goals.get_mut(ROOT).unwrap();
            current.status = status.into();
            current.tokens_used = 25;
            fake.writes.clear();
            recover_destination_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).unwrap();
            assert!(fake.writes.is_empty());
        }
        fake.goals.get_mut(ROOT).unwrap().objective = "A different goal".into();
        assert!(
            recover_destination_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).is_err()
        );
        assert!(fake.writes.is_empty());
    }

    #[test]
    fn activation_rejects_changed_destination_and_malformed_source_state() {
        let snapshot = goal("active", 0, None);
        let mut fake = Fake::with(goal("paused", 1, None));
        assert!(activate_using(&mut |m, p| fake.request(m, p), ROOT, &snapshot).is_err());
        let bad = goal("active", -1, Some(100));
        assert!(stage_using(&mut |m, p| fake.request(m, p), ROOT, &bad).is_err());
        assert!(fake.writes.is_empty());
    }
}
