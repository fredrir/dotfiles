use super::*;

pub(super) struct Notification {
    pub title: String,
    pub started: Instant,
}

impl Notification {
    pub fn view(&self, reduced_motion: bool) -> view::ToastView<'_> {
        view::ToastView {
            title: &self.title,
            loading: false,
            elapsed: self.started.elapsed(),
            reduced_motion,
        }
    }
}

impl App {
    pub(super) fn toast(&self, reduced_motion: bool) -> Option<view::ToastView<'_>> {
        self.operation
            .as_ref()
            .map(|operation| operation.view(reduced_motion))
            .or_else(|| {
                self.notification
                    .as_ref()
                    .map(|notification| notification.view(reduced_motion))
            })
    }
}

impl PendingOperation {
    pub(super) fn view(&self, reduced_motion: bool) -> view::ToastView<'_> {
        view::ToastView {
            title: &self.label,
            loading: true,
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
