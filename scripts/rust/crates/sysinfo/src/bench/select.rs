use super::{
    record::{ANY, Run},
    store::Store,
};
#[derive(Clone, Debug, Default)]
pub struct Selector {
    pub host: String,
    pub os_id: String,
    pub epoch: String,
    pub run_id: String,
}
impl Selector {
    pub fn parse(text: &str) -> Self {
        let (rest, run_id) = text.trim().split_once(':').unwrap_or((text.trim(), ""));
        let (rest, epoch) = rest.split_once('@').unwrap_or((rest, ""));
        let (host, os_id) = rest.split_once('/').unwrap_or((rest, ""));
        Self {
            host: host.trim().into(),
            os_id: os_id.trim().into(),
            epoch: epoch.trim().into(),
            run_id: run_id.trim().into(),
        }
    }
    pub fn matches(&self, run: &Run) -> bool {
        (self.host.is_empty() || self.host == run.host)
            && (self.os_id.is_empty() || self.os_id == run.os_id())
            && (self.epoch.is_empty() || self.epoch == run.epoch())
            && (self.run_id.is_empty() || self.run_id == run.run_id)
    }
    pub fn candidates(&self, store: &Store, grades: &[&str]) -> Result<Vec<Run>, String> {
        Ok(store
            .list_runs(
                (!self.host.is_empty()).then_some(self.host.as_str()),
                grades,
            )?
            .into_iter()
            .filter(|run| self.matches(run))
            .collect())
    }
    pub fn resolve(&self, store: &Store) -> Result<Option<Run>, String> {
        let mut matches = self.candidates(store, ANY)?;
        let preferred = matches
            .iter()
            .position(|run| run.grade == "clean")
            .unwrap_or(0);
        Ok((!matches.is_empty()).then(|| matches.swap_remove(preferred)))
    }
}
