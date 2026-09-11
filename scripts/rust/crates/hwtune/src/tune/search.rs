use serde::Serialize;

use super::{controls::Profile, transaction::Guard};
use crate::bench::compare;
use crate::bench::record::Run;

pub trait Measurements {
    fn benchmark(&mut self, phase: &str) -> Result<Run, String>;
    fn stress(&mut self, phase: &str) -> Result<(), String>;
    fn record(&mut self, evidence: serde_json::Value) -> Result<(), String>;
    fn interrupted(&self) -> bool {
        crate::bench::runner::cancelled()
    }
}

pub fn optimize(
    measurements: &mut dyn Measurements,
    guard: &mut Guard,
    candidates: &[Profile],
    metric: &str,
    minimum: f64,
) -> Result<(Profile, Run), String> {
    let baseline = measurements.benchmark("baseline")?;
    complete(&baseline, metric)?;
    let mut winner: Option<(Profile, f64)> = None;
    for profile in candidates {
        if measurements.interrupted() {
            return Err("tuning was interrupted".into());
        }
        let trial = (|| {
            guard.apply(profile)?;
            measurements.stress(&format!("{}-stability", profile.name))?;
            measurements.benchmark(&profile.name)
        })();
        guard.reset()?;
        match trial {
            Ok(run) => {
                let verdict = evaluate(&baseline, &run, metric, minimum);
                if verdict.accepted
                    && winner
                        .as_ref()
                        .is_none_or(|(_, score)| verdict.improvement_pct > *score)
                {
                    winner = Some((profile.clone(), verdict.improvement_pct));
                }
                measurements.record(
                    serde_json::json!({"profile":profile,"run_id":run.run_id,"verdict":verdict}),
                )?;
            }
            Err(error) => {
                measurements.record(
                    serde_json::json!({"profile":profile,"status":"rejected","reason":error}),
                )?;
                if measurements.interrupted() {
                    return Err(error);
                }
            }
        }
    }
    let (winner, _) = winner
        .ok_or("no candidate produced a validated improvement; original settings were restored")?;
    let repeated_baseline = measurements.benchmark("validation-baseline")?;
    baseline_stable(&baseline, &repeated_baseline, metric)?;
    if measurements.interrupted() {
        return Err("tuning was interrupted".into());
    }
    guard.apply(&winner)?;
    measurements.stress("winner-validation-stability")?;
    let repeated = measurements.benchmark("winner-validation")?;
    let verdict = evaluate(&repeated_baseline, &repeated, metric, minimum);
    measurements.record(serde_json::json!({"phase":"independent-validation","verdict":verdict}))?;
    if !verdict.accepted {
        return Err(format!(
            "winner did not pass independent validation: {}",
            verdict.reason
        ));
    }
    Ok((winner, repeated))
}

#[derive(Clone, Debug, Serialize)]
pub struct Verdict {
    pub accepted: bool,
    pub improvement_pct: f64,
    pub reason: String,
}

pub fn complete(run: &Run, metric: &str) -> Result<(), String> {
    if run.grade != "clean" || !run.gate_reasons.is_empty() {
        return Err(format!(
            "run {} is {}: {}",
            run.run_id,
            run.grade,
            run.gate_reasons.join("; ")
        ));
    }
    let objective = run
        .metric(metric)
        .ok_or_else(|| format!("objective metric {metric} was not measured"))?;
    if objective.samples.len() < 3
        || objective
            .samples
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(format!(
            "{metric} needs at least three positive finite samples"
        ));
    }
    Ok(())
}

pub fn evaluate(before: &Run, after: &Run, metric: &str, minimum: f64) -> Verdict {
    let mut verdict = Verdict {
        accepted: false,
        improvement_pct: 0.0,
        reason: String::new(),
    };
    let evaluated = (|| {
        complete(before, metric)?;
        complete(after, metric)?;
        if before.host != after.host
            || before.epoch() != after.epoch()
            || before.tier != after.tier
            || before.os_id() != after.os_id()
        {
            return Err("host, hardware, operating system, or measurement tier changed".into());
        }
        let comparison = compare::compare_runs(before, after);
        if !comparison.only_left.is_empty() || !comparison.only_right.is_empty() {
            return Err("candidate did not measure the same set of metrics as the baseline".into());
        }
        if let Some(delta) = comparison
            .deltas
            .iter()
            .find(|delta| delta.verdict == "blocked")
        {
            return Err(format!(
                "{} cannot be compared: {}",
                delta.key, delta.reason
            ));
        }
        if let Some(delta) = comparison
            .deltas
            .iter()
            .find(|delta| delta.verdict == "worse")
        {
            return Err(format!(
                "{} regressed by {:.2}% beyond measurement noise",
                delta.key,
                delta.change_pct.abs()
            ));
        }
        let objective = comparison
            .deltas
            .iter()
            .find(|delta| delta.key == metric)
            .ok_or("objective has no comparable result")?;
        verdict.improvement_pct = if objective.proportion == "LIB" {
            -objective.change_pct
        } else {
            objective.change_pct
        };
        if objective.verdict != "better" || verdict.improvement_pct < minimum {
            return Err(format!(
                "{metric} improvement {:.2}% does not exceed {:.2}% noise and {minimum:.2}% minimum",
                verdict.improvement_pct, objective.band_pct
            ));
        }
        Ok(())
    })();
    match evaluated {
        Ok(()) => {
            verdict.accepted = true;
            verdict.reason = "repeatable improvement without a measured regression".into();
        }
        Err(error) => verdict.reason = error,
    }
    verdict
}

pub fn baseline_stable(before: &Run, after: &Run, metric: &str) -> Result<(), String> {
    complete(before, metric)?;
    complete(after, metric)?;
    if before.host != after.host
        || before.epoch() != after.epoch()
        || before.os_id() != after.os_id()
        || before.tier != after.tier
    {
        return Err("baseline identity changed during tuning".into());
    }
    let comparison = compare::compare_runs(before, after);
    if !comparison.only_left.is_empty()
        || !comparison.only_right.is_empty()
        || comparison
            .deltas
            .iter()
            .any(|delta| delta.verdict != "noise")
    {
        return Err("baseline drifted beyond measurement noise; no winner was retained".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/tune/search_tests.rs"]
mod tests;
