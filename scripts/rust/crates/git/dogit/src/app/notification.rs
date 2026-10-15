use super::*;

pub(super) struct Notification {
    pub title: String,
    pub detail: String,
    pub failed: bool,
    pub started: Instant,
}

impl Notification {
    pub fn view(&self, reduced_motion: bool) -> view::ToastView<'_> {
        view::ToastView {
            title: &self.title,
            detail: &self.detail,
            failed: self.failed,
            elapsed: self.started.elapsed(),
            reduced_motion,
        }
    }
}

pub(super) fn success_message(label: &str) -> String {
    let message = match label.to_ascii_lowercase().as_str() {
        "commit" => "Committed!",
        "amend" => "Amended!",
        "push" => "Pushed!",
        "pull" => "Pulled!",
        "fetch" => "Fetched!",
        "merge" => "Merged!",
        "rebase" => "Rebased!",
        "cherry-pick" | "cherry-pick merge" => "Cherry-picked!",
        "revert" | "revert merge" => "Reverted!",
        "switch branch" => "Branch switched!",
        "create branch" => "Branch created!",
        "save stash" => "Stashed!",
        "apply stash" => "Stash applied!",
        "pop stash" => "Stash popped!",
        "drop stash" => "Stash dropped!",
        "discard selected changes" => "Changes discarded!",
        "continue operation" => "Continued!",
        "abort operation" => "Aborted!",
        "toggle all staging" => "Staging updated!",
        "stage files" | "stage selection" => "Staged!",
        "unstage files" | "unstage selection" => "Unstaged!",
        _ => return format!("{} finished!", capitalize(label)),
    };
    message.into()
}

pub(super) fn capitalize(label: &str) -> String {
    let label = label.trim();
    let mut letters = label.chars();
    letters.next().map_or_else(
        || "Operation".into(),
        |first| first.to_uppercase().collect::<String>() + letters.as_str(),
    )
}
