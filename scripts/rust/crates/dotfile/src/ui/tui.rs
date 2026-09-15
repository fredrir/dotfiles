use std::collections::{HashSet, VecDeque};
use std::io::{self, IsTerminal, Stderr};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use tachyonfx::{CellFilter, Effect, EffectRenderer, Interpolation, fx};
use ui_diff_view::{Action as DiffAction, DiffDocument, DiffView, ViewState};
use ui_progress::{PhaseState, Progress, ProgressBar};
use ui_terminal::{Inline, SignalGuard, SignalOptions, Teardown, ui_style};
use ui_theme::{Palette, Role, ThemeHandle};
use workstation::text::plural;

use crate::decision::{Choice, Prompt, Request, Server, Subject};
use crate::event::{Event, Phase, Summary};

use super::UiPolicy;

const ITEM_CAPACITY: usize = 6;
const EFFECT_FRAME: Duration = Duration::from_millis(33);
const SPINNER_FRAME: Duration = Duration::from_millis(80);
const INPUT_FRAME: Duration = Duration::from_millis(100);
const DECISION_POLL: Duration = Duration::from_millis(25);
const MERGE_CHOICES: [Choice; 5] = [
    Choice::Repo,
    Choice::Live,
    Choice::Ignore,
    Choice::Skip,
    Choice::Abort,
];
const REMOTE_CHOICES: [Choice; 2] = [Choice::Discard, Choice::Cancel];
const OVERWRITE_CHOICES: [Choice; 4] = [
    Choice::Overwrite,
    Choice::Keep,
    Choice::OverwriteAll,
    Choice::KeepAll,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiUpdate {
    pub redraw: bool,
    pub phase_changed: bool,
    pub output: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct UiModel {
    verbose: bool,
    profile: String,
    dry_run: bool,
    peer: Option<String>,
    phase: Option<Phase>,
    completed: usize,
    total: Option<usize>,
    label: String,
    seen_phases: Vec<Phase>,
    completed_phases: Vec<Phase>,
    items: VecDeque<String>,
    seen_items: HashSet<String>,
    warning: Option<(String, Option<String>)>,
    seen_warnings: HashSet<(String, Option<String>)>,
    failure: Option<(String, Option<String>)>,
    finished: Option<Summary>,
    cancelling: bool,
    decision: Option<DecisionState>,
}

#[derive(Clone, Debug)]
struct DecisionState {
    request: Request,
    choices: Vec<Choice>,
    selected: usize,
    diff: Option<DiffDocument>,
    diff_view: ViewState,
}

impl DecisionState {
    fn new(request: Request) -> Self {
        let choices = decision_choices(&request.prompt);
        let selected = match &request.prompt {
            Prompt::MergeTarget {
                targets, default, ..
            } if !targets.is_empty() => (*default).min(targets.len() - 1),
            prompt => choices
                .iter()
                .position(|choice| *choice == prompt.preselected())
                .unwrap_or(0),
        };
        let diff = match &request.prompt {
            Prompt::Merge { repo, live, .. } => {
                Some(DiffDocument::new(&diff_value(repo), &diff_value(live)))
            }
            Prompt::Overwrite {
                repo: Some(repo),
                live: Some(live),
                ..
            } => Some(DiffDocument::new(&diff_value(repo), &diff_value(live))),
            _ => None,
        };
        Self {
            request,
            choices,
            selected,
            diff,
            diff_view: ViewState::default(),
        }
    }

    fn selected(&self) -> Choice {
        self.choices[self.selected]
    }
}

fn diff_value(value: &str) -> std::borrow::Cow<'_, str> {
    if value.len() <= 256 * 1024
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value)
    {
        match parsed {
            serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                if let Ok(formatted) = serde_json::to_string_pretty(&parsed) {
                    return formatted.into();
                }
            }
            serde_json::Value::String(text) if text.contains('\n') => return text.into(),
            _ => {}
        }
    }
    value.into()
}

impl UiModel {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            profile: String::new(),
            dry_run: false,
            peer: None,
            phase: None,
            completed: 0,
            total: None,
            label: String::new(),
            seen_phases: Vec::new(),
            completed_phases: Vec::new(),
            items: VecDeque::with_capacity(ITEM_CAPACITY),
            seen_items: HashSet::new(),
            warning: None,
            seen_warnings: HashSet::new(),
            failure: None,
            finished: None,
            cancelling: false,
            decision: None,
        }
    }

    pub fn apply(&mut self, event: &Event) -> UiUpdate {
        let mut update = UiUpdate {
            redraw: true,
            phase_changed: false,
            output: Vec::new(),
        };
        match event {
            Event::Started {
                profile,
                dry_run,
                peer,
            } => {
                self.profile = super::sanitize_text(profile);
                self.dry_run = *dry_run;
                self.peer = peer.as_deref().map(super::sanitize_text);
                update.phase_changed = true;
                if self.verbose {
                    let operation = if peer.is_some() {
                        "push"
                    } else if *dry_run {
                        "plan"
                    } else {
                        "sync"
                    };
                    update.output.push(match peer {
                        Some(peer) => format!(
                            "{operation} {} → {}",
                            super::sanitize_text(profile),
                            super::sanitize_text(peer)
                        ),
                        None => format!("{operation} {}", super::sanitize_text(profile)),
                    });
                }
            }
            Event::PhaseStarted { phase, total } => {
                self.start_phase(*phase, *total);
                update.phase_changed = true;
                if self.verbose {
                    update.output.push(super::phase_name(*phase).to_string());
                }
            }
            Event::Progress {
                phase,
                completed,
                total,
                label,
            } => {
                if self.phase != Some(*phase) {
                    self.start_phase(*phase, *total);
                    update.phase_changed = true;
                }
                self.completed = *completed;
                self.total = *total;
                self.label = super::sanitize_text(label);
            }
            Event::Item {
                action,
                path,
                detail,
                changed,
            } => {
                if self.verbose && (*action != crate::event::Action::Check || *changed) {
                    let line = super::item_line(*action, path, detail);
                    if self.seen_items.insert(line.clone()) {
                        if self.items.len() == ITEM_CAPACITY {
                            self.items.pop_front();
                        }
                        self.items.push_back(line.clone());
                        update.output.push(line);
                    } else {
                        update.redraw = false;
                    }
                } else {
                    update.redraw = false;
                }
            }
            Event::Warning { message, hint } => {
                let message = super::sanitize_text(message);
                let hint = hint.as_deref().map(super::sanitize_text);
                let key = (message.clone(), hint.clone());
                if self.seen_warnings.insert(key) {
                    self.warning = Some((message.clone(), hint.clone()));
                    update.output.push(format!("warning: {message}"));
                    if let Some(hint) = hint {
                        update.output.push(format!("  hint: {hint}"));
                    }
                } else {
                    update.redraw = false;
                }
            }
            Event::Failed { message, hint, .. } => {
                self.failure = Some((
                    super::sanitize_text(message),
                    hint.as_deref().map(super::sanitize_text),
                ));
            }
            Event::Finished(summary) => {
                if let Some(phase) = self.phase
                    && !self.completed_phases.contains(&phase)
                {
                    self.completed_phases.push(phase);
                }
                self.profile = super::sanitize_text(&summary.profile);
                self.dry_run = summary.dry_run;
                let mut summary = summary.clone();
                summary.profile = super::sanitize_text(&summary.profile);
                summary.peer = summary.peer.as_deref().map(super::sanitize_text);
                self.finished = Some(summary);
            }
        }
        update
    }

    pub fn active(&self) -> bool {
        self.finished.is_none() && self.failure.is_none()
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn failure(&self) -> Option<(String, Option<String>)> {
        self.failure.clone()
    }

    pub fn show_decision(&mut self, request: Request) {
        self.decision = Some(DecisionState::new(request));
    }

    pub fn decision_active(&self) -> bool {
        self.decision.is_some()
    }

    pub fn selected_choice(&self) -> Option<Choice> {
        self.decision.as_ref().map(DecisionState::selected)
    }

    pub fn navigate_diff(&mut self, action: DiffAction, height: u16) {
        if let Some(decision) = &mut self.decision
            && let Some(document) = &decision.diff
        {
            decision
                .diff_view
                .apply(action, document, height.saturating_sub(6));
        }
    }

    pub fn navigate_diff_in_area(&mut self, action: DiffAction, width: u16, height: u16) {
        self.fit_diff_view(width, height);
        self.navigate_diff(action, height);
    }

    pub fn fit_diff_view(&mut self, width: u16, height: u16) {
        if let Some(decision) = &mut self.decision
            && let Some(document) = &decision.diff
        {
            decision.diff_view.fit_width(
                document,
                width.saturating_sub(2),
                height.saturating_sub(6),
            );
        }
    }

    pub fn select_next(&mut self) {
        if let Some(decision) = &mut self.decision {
            decision.selected = (decision.selected + 1) % decision.choices.len();
        }
    }

    pub fn select_previous(&mut self) {
        if let Some(decision) = &mut self.decision {
            let count = decision.choices.len();
            decision.selected = (decision.selected + count - 1) % count;
        }
    }

    pub fn select_choice(&mut self, choice: Choice) {
        if let Some(decision) = &mut self.decision
            && let Some(selected) = decision
                .choices
                .iter()
                .position(|candidate| *candidate == choice)
        {
            decision.selected = selected;
        }
    }

    pub fn choice_for_key(&self, key: char) -> Option<Choice> {
        let decision = self.decision.as_ref()?;
        let candidates: &[Choice] = match key {
            'r' => &[Choice::Repo],
            'l' => &[Choice::Live],
            'i' => &[Choice::Ignore],
            's' => &[Choice::KeepAll, Choice::Skip],
            'a' => &[Choice::OverwriteAll, Choice::Abort],
            'd' => &[Choice::Discard],
            'c' => &[Choice::Cancel],
            'y' => &[Choice::Overwrite],
            'n' => &[Choice::Keep],
            _ => &[],
        };
        candidates
            .iter()
            .copied()
            .find(|candidate| decision.choices.contains(candidate))
    }

    pub fn answers_on_key(&self) -> bool {
        matches!(
            self.decision.as_ref().map(|decision| &decision.request.prompt),
            Some(Prompt::Overwrite { .. })
        )
    }

    pub fn select_index(&mut self, selected: usize) {
        if let Some(decision) = &mut self.decision
            && selected < decision.choices.len()
        {
            decision.selected = selected;
        }
    }

    pub fn decision_response(&self) -> Option<(Request, Choice)> {
        self.decision
            .as_ref()
            .map(|decision| (decision.request.clone(), decision.selected()))
    }

    pub fn cancel_response(&self) -> Option<(Request, Choice)> {
        self.decision.as_ref().map(|decision| {
            let choice = decision.request.prompt.cancellation();
            (decision.request.clone(), choice)
        })
    }

    pub fn dismiss_decision(&mut self) {
        self.decision = None;
    }

    fn request_cancel(&mut self) {
        self.cancelling = true;
    }

    fn start_phase(&mut self, phase: Phase, total: Option<usize>) {
        if let Some(previous) = self.phase
            && previous != phase
            && !self.completed_phases.contains(&previous)
        {
            self.completed_phases.push(previous);
        }
        if !self.seen_phases.contains(&phase) {
            self.seen_phases.push(phase);
        }
        self.phase = Some(phase);
        self.completed = 0;
        self.total = total;
        self.label.clear();
    }

    fn desired_height(&self) -> u16 {
        if self
            .decision
            .as_ref()
            .is_some_and(|decision| decision.diff.is_some())
        {
            return ui_terminal::terminal_height()
                .unwrap_or(24)
                .saturating_sub(2)
                .clamp(4, 22) as u16;
        }
        match (self.verbose, self.peer.is_some()) {
            (false, false) => 4,
            (false, true) => 5,
            (true, false) => 11,
            (true, true) => 12,
        }
    }
}

pub fn run(
    receiver: Receiver<Event>,
    decisions: Server,
    worker: JoinHandle<Result<Summary, String>>,
    verbose: bool,
    policy: UiPolicy,
) -> Result<Summary, String> {
    let signals = match SignalGuard::with_options(SignalOptions {
        cancellation: Some(crate::cancel::flag()),
        reset_to_default: true,
        reraise_on_drop: false,
        restart_syscalls: true,
    }) {
        Ok(signals) => signals,
        Err(_) => return super::plain::run(receiver, decisions, worker, verbose),
    };
    let mut model = UiModel::new(verbose);
    let mut pending_output = Vec::new();
    let mut phase_changed = false;
    let mut deferred_plain = Vec::new();
    let mut decisions_open = true;
    let pending_decision = loop {
        let incoming = if decisions_open {
            crossbeam_channel::select! {
                recv(receiver) -> event => ChannelInput::Event(event),
                recv(decisions.requests()) -> request => ChannelInput::Decision(request),
            }
        } else {
            ChannelInput::Event(receiver.recv())
        };
        match incoming {
            ChannelInput::Event(Ok(event)) => {
                let finished = matches!(&event, Event::Finished(_) | Event::Failed { .. });
                let starts_tui = starts_tui(&event, verbose);
                if verbose || matches!(&event, Event::Warning { .. } | Event::Failed { .. }) {
                    deferred_plain.push(event.clone());
                }
                let update = model.apply(&event);
                pending_output.extend(update.output);
                phase_changed |= update.phase_changed;
                if finished {
                    return super::plain::run_with_initial(
                        receiver,
                        decisions,
                        worker,
                        verbose,
                        deferred_plain,
                    );
                }
                if starts_tui {
                    break None;
                }
            }
            ChannelInput::Event(Err(_)) => {
                return super::plain::run_with_initial(
                    receiver,
                    decisions,
                    worker,
                    verbose,
                    deferred_plain,
                );
            }
            ChannelInput::Decision(Ok(request)) => break Some(request),
            ChannelInput::Decision(Err(_)) => decisions_open = false,
            ChannelInput::Timeout => unreachable!(),
        }
    };
    if let Some(request) = pending_decision {
        model.show_decision(request);
    }
    let mut terminal = match InlineTerminal::new(model.desired_height(), signals) {
        Ok(terminal) => terminal,
        Err(_) => {
            if let Some((request, _)) = model.decision_response()
                && let Err(error) = decisions.respond(&request, request.prompt.safe_default())
            {
                super::settle_worker_after_ui_error(&receiver, &decisions, worker, Some(request));
                return Err(error);
            }
            return super::plain::run_with_initial(
                receiver,
                decisions,
                worker,
                verbose,
                deferred_plain,
            );
        }
    };
    let mut theme = ThemeHandle::discover();
    let palette = theme.palette();
    let started = Instant::now();
    let mut last_draw = Instant::now();
    let mut dirty = true;
    let mut effect = ((phase_changed || model.decision_active()) && policy.motion)
        .then(|| phase_effect(palette, policy));

    let ui_result = (|| -> Result<(), String> {
        loop {
            dirty |= theme.poll();
            let palette = theme.palette();
            if crate::cancel::requested() && !model.cancelling {
                if let Some((request, choice)) = model.cancel_response() {
                    let _ = decisions.respond(&request, choice);
                    model.dismiss_decision();
                }
                model.request_cancel();
                dirty = true;
            }
            if !pending_output.is_empty() {
                terminal.write_scrollback(palette, &pending_output, policy.color)?;
                pending_output.clear();
                dirty = true;
            }

            let now = Instant::now();
            let effect_running = effect.as_ref().is_some_and(Effect::running) && model.active();
            let operation_animating = model.active() && !model.decision_active();
            let animation_due = policy.motion
                && (effect_running || operation_animating)
                && now.duration_since(last_draw)
                    >= if effect_running {
                        EFFECT_FRAME
                    } else {
                        SPINNER_FRAME
                    };
            if dirty || animation_due {
                let tick = now.duration_since(last_draw);
                let frame_index = if policy.motion {
                    started.elapsed().as_millis() as u64 / SPINNER_FRAME.as_millis() as u64
                } else {
                    0
                };
                terminal.draw(
                    palette,
                    &mut model,
                    frame_index,
                    policy,
                    effect.as_mut(),
                    tick,
                )?;
                last_draw = now;
                dirty = false;
                if effect.as_ref().is_some_and(Effect::done) {
                    effect = None;
                }
            }

            match terminal.input()? {
                InputAction::Cancel if !model.cancelling => {
                    if let Some((request, choice)) = model.cancel_response() {
                        decisions.respond(&request, choice)?;
                        model.dismiss_decision();
                    }
                    crate::cancel::request();
                    model.request_cancel();
                    dirty = true;
                }
                InputAction::Previous if model.decision_active() => {
                    model.select_previous();
                    dirty = true;
                }
                InputAction::Next if model.decision_active() => {
                    model.select_next();
                    dirty = true;
                }
                InputAction::Letter(key) if model.decision_active() => {
                    if let Some(choice) = model.choice_for_key(key) {
                        model.select_choice(choice);
                        if model.answers_on_key()
                            && let Some((request, choice)) = model.decision_response()
                        {
                            decisions.respond(&request, choice)?;
                            model.dismiss_decision();
                        }
                        dirty = true;
                    }
                }
                InputAction::SelectIndex(selected) if model.decision_active() => {
                    model.select_index(selected);
                    dirty = true;
                }
                InputAction::Confirm if model.decision_active() => {
                    if let Some((request, choice)) = model.decision_response() {
                        decisions.respond(&request, choice)?;
                        model.dismiss_decision();
                        dirty = true;
                    }
                }
                InputAction::Redraw => dirty = true,
                InputAction::Diff(action) if model.decision_active() => {
                    let area = terminal.inline.terminal().get_frame().area();
                    model.navigate_diff_in_area(action, area.width, area.height);
                    dirty = true;
                }
                _ => {}
            }

            let wait = if model.decision_active() {
                DECISION_POLL
            } else if model.active() && policy.motion {
                if effect.as_ref().is_some_and(Effect::running) {
                    EFFECT_FRAME
                } else {
                    SPINNER_FRAME
                }
            } else {
                INPUT_FRAME
            };

            let incoming = if decisions_open {
                crossbeam_channel::select! {
                    recv(receiver) -> event => ChannelInput::Event(event),
                    recv(decisions.requests()) -> request => ChannelInput::Decision(request),
                    default(wait) => ChannelInput::Timeout,
                }
            } else {
                match receiver.recv_timeout(wait) {
                    Ok(event) => ChannelInput::Event(Ok(event)),
                    Err(RecvTimeoutError::Timeout) => ChannelInput::Timeout,
                    Err(RecvTimeoutError::Disconnected) => {
                        ChannelInput::Event(Err(crossbeam_channel::RecvError))
                    }
                }
            };
            let next = match incoming {
                ChannelInput::Event(Ok(event)) => event,
                ChannelInput::Event(Err(_)) => break Ok(()),
                ChannelInput::Decision(Ok(request)) => {
                    if model.decision_active() {
                        decisions.respond(&request, request.prompt.safe_default())?;
                    } else {
                        model.show_decision(request);
                        dirty = true;
                        if policy.motion {
                            effect = Some(phase_effect(palette, policy));
                        }
                    }
                    continue;
                }
                ChannelInput::Decision(Err(_)) => {
                    decisions_open = false;
                    continue;
                }
                ChannelInput::Timeout => continue,
            };
            let update = model.apply(&next);
            dirty |= update.redraw;
            pending_output.extend(update.output);
            if update.phase_changed && policy.motion && model.active() {
                effect = Some(phase_effect(palette, policy));
            }
            for event in receiver.try_iter().take(256) {
                let update = model.apply(&event);
                dirty |= update.redraw;
                pending_output.extend(update.output);
                if update.phase_changed && policy.motion && model.active() {
                    effect = Some(phase_effect(palette, policy));
                }
            }
        }
    })();

    let failure = model.failure();
    let pending_decision = model.cancel_response().map(|(request, _)| request);
    drop(terminal);
    match ui_result {
        Ok(()) => super::finish_worker(worker, failure),
        Err(error) => {
            super::settle_worker_after_ui_error(&receiver, &decisions, worker, pending_decision);
            Err(error)
        }
    }
}

fn starts_tui(event: &Event, verbose: bool) -> bool {
    verbose
        || matches!(
            event,
            Event::Started {
                dry_run: false,
                peer: Some(_),
                ..
            }
        )
        || matches!(event, Event::Item { changed: true, .. })
        || matches!(
            event,
            Event::PhaseStarted {
                phase: Phase::Tooling,
                ..
            }
        )
        || matches!(
            event,
            Event::PhaseStarted {
                phase: Phase::Links,
                total: Some(total),
            } if *total > 0
        )
}

pub fn render_buffer(
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    frame_index: u64,
    color: bool,
) {
    render_buffer_with_palette(&Palette::current(), model, area, buffer, frame_index, color);
}

fn theme_color(palette: &Palette, role: Role) -> Color {
    palette.foreground(role).ratatui()
}

pub fn render_buffer_with_palette(
    palette: &Palette,
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    frame_index: u64,
    color: bool,
) {
    if area.is_empty() {
        return;
    }
    Clear.render(area, buffer);
    buffer.set_style(
        area,
        palette.ratatui(
            if color {
                ui_theme::ColorMode::Always
            } else {
                ui_theme::ColorMode::Never
            },
            true,
            Role::Background,
        ),
    );
    if let Some(decision) = &model.decision {
        render_decision(palette, decision, area, buffer, color);
        return;
    }
    let mut row = 0;
    render_header(palette, model, line_area(area, row), buffer, color);
    row += 1;
    if model.peer.is_some() && row < area.height {
        render_push_track(palette, model, line_area(area, row), buffer, color);
        row += 1;
    }
    if row < area.height {
        render_status(
            palette,
            model,
            line_area(area, row),
            buffer,
            frame_index,
            color,
        );
        row += 1;
    }
    if row < area.height {
        render_progress(
            palette,
            model,
            line_area(area, row),
            buffer,
            frame_index,
            color,
        );
        row += 1;
    }
    if model.verbose && row < area.height {
        let panel = Rect::new(area.x, area.y + row, area.width, area.height - row);
        render_items(palette, model, panel, buffer, color);
    } else if row < area.height {
        render_notice(palette, model, line_area(area, row), buffer, color);
    }
}

fn render_decision(
    palette: &Palette,
    decision: &DecisionState,
    area: Rect,
    buffer: &mut Buffer,
    color: bool,
) {
    if area.height >= 7 {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(ui_style(
                color,
                theme_color(palette, Role::Accent),
                Modifier::BOLD,
            ))
            .title(Span::styled(
                decision_title(&decision.request.prompt),
                ui_style(color, theme_color(palette, Role::Ours), Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buffer);
        render_decision_body(palette, decision, inner, buffer, color, true);
    } else {
        render_decision_body(palette, decision, area, buffer, color, false);
    }
}

fn overwrite_spans(
    palette: &Palette,
    decision: &DecisionState,
    subject: Subject,
    color: bool,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("  {}", subject.question()),
        ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
    )];
    spans.extend(choice_spans(palette, decision, color));
    spans
}

fn decision_title(prompt: &Prompt) -> String {
    match prompt {
        Prompt::Merge { .. } => " MERGE CONFLICT ".to_string(),
        Prompt::Overwrite { subject, .. } => format!(" {} ", subject.title()),
        _ => " decision ".to_string(),
    }
}

fn render_decision_body(
    palette: &Palette,
    decision: &DecisionState,
    area: Rect,
    buffer: &mut Buffer,
    color: bool,
    spacious: bool,
) {
    let width = area.width.saturating_sub(12) as usize;
    match &decision.request.prompt {
        Prompt::Merge {
            path,
            key,
            repo,
            live,
        } => {
            if area.height >= 9 {
                render_labeled_value(
                    palette,
                    area,
                    0,
                    "path",
                    &super::compact_path(path),
                    buffer,
                    color,
                );
                render_labeled_value(
                    palette,
                    area,
                    1,
                    "key",
                    &compact_text(key, usize::from(area.width.saturating_sub(12))),
                    buffer,
                    color,
                );
                if let Some(document) = &decision.diff {
                    DiffView {
                        document,
                        state: &decision.diff_view,
                        palette,
                        color,
                        left_label: "repo",
                        right_label: "live",
                    }
                    .render(
                        Rect::new(
                            area.x,
                            area.y + 2,
                            area.width,
                            area.height.saturating_sub(4),
                        ),
                        buffer,
                    );
                }
                render_decision_line(
                    area,
                    area.height.saturating_sub(2),
                    Line::from(choice_spans(palette, decision, color)),
                    buffer,
                );
                render_decision_line(
                    area,
                    area.height.saturating_sub(1),
                    Line::from("  ←/→ choose · r repo · l live · enter confirm · q cancel"),
                    buffer,
                );
                return;
            }
            let display_path = compact_text(&super::compact_path(path), width.max(8));
            let display_key = compact_text(key, width.max(8));
            if spacious {
                render_decision_line(
                    area,
                    0,
                    Line::from(Span::styled(
                        "  MERGE CONFLICT",
                        ui_style(color, theme_color(palette, Role::Conflict), Modifier::BOLD),
                    )),
                    buffer,
                );
                render_labeled_value(palette, area, 1, "path", &display_path, buffer, color);
                render_labeled_value(palette, area, 2, "key", &display_key, buffer, color);
                render_labeled_value(
                    palette,
                    area,
                    3,
                    "repo",
                    &compact_text(repo, width.max(8)),
                    buffer,
                    color,
                );
                render_labeled_value(
                    palette,
                    area,
                    4,
                    "live",
                    &compact_text(live, width.max(8)),
                    buffer,
                    color,
                );
            } else {
                let pair_width = (area.width.saturating_sub(16) as usize / 2).max(4);
                let compact_key = compact_text(key, pair_width);
                let compact_path = compact_text(&super::compact_path(path), pair_width);
                render_decision_line(
                    area,
                    0,
                    Line::from(vec![
                        Span::styled(
                            "  MERGE  ",
                            ui_style(color, theme_color(palette, Role::Conflict), Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{compact_key}  |  {compact_path}"),
                            ui_style(color, theme_color(palette, Role::Plain), Modifier::empty()),
                        ),
                    ]),
                    buffer,
                );
                render_labeled_value(
                    palette,
                    area,
                    1,
                    "repo",
                    &compact_text(repo, width.max(8)),
                    buffer,
                    color,
                );
                render_labeled_value(
                    palette,
                    area,
                    2,
                    "live",
                    &compact_text(live, width.max(8)),
                    buffer,
                    color,
                );
            }
        }
        Prompt::MergeTarget {
            path, key, targets, ..
        } => {
            let display_path = compact_text(&super::compact_path(path), width.max(8));
            let display_key = compact_text(key, width.max(8));
            let selected = selected_target_label(decision, targets, width.max(8));
            if spacious {
                render_decision_line(
                    area,
                    0,
                    Line::from(Span::styled(
                        "  MERGE DESTINATION",
                        ui_style(color, theme_color(palette, Role::Theirs), Modifier::BOLD),
                    )),
                    buffer,
                );
                render_labeled_value(palette, area, 1, "path", &display_path, buffer, color);
                render_labeled_value(palette, area, 2, "key", &display_key, buffer, color);
                render_labeled_value(palette, area, 3, "target", &selected, buffer, color);
                render_labeled_value(
                    palette,
                    area,
                    4,
                    "options",
                    &format!("{} destinations", targets.len()),
                    buffer,
                    color,
                );
            } else {
                let pair_width = (area.width.saturating_sub(17) as usize / 2).max(4);
                render_decision_line(
                    area,
                    0,
                    Line::from(vec![
                        Span::styled(
                            "  TARGET  ",
                            ui_style(color, theme_color(palette, Role::Theirs), Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                "{}  |  {}",
                                compact_text(key, pair_width),
                                compact_text(&super::compact_path(path), pair_width)
                            ),
                            ui_style(color, theme_color(palette, Role::Plain), Modifier::empty()),
                        ),
                    ]),
                    buffer,
                );
                render_labeled_value(palette, area, 1, "target", &selected, buffer, color);
                render_labeled_value(
                    palette,
                    area,
                    2,
                    "options",
                    &format!("{} destinations", targets.len()),
                    buffer,
                    color,
                );
            }
        }
        Prompt::Overwrite {
            subject,
            path,
            detail,
            index,
            total,
            ..
        } => {
            let position = if *total > 1 {
                format!("  ({index} of {total})")
            } else {
                String::new()
            };
            if area.height >= 9 && decision.diff.is_some() {
                render_labeled_value(
                    palette,
                    area,
                    0,
                    "path",
                    &format!("{}{position}", super::compact_path(path)),
                    buffer,
                    color,
                );
                render_labeled_value(
                    palette,
                    area,
                    1,
                    "found",
                    &compact_text(detail, usize::from(area.width.saturating_sub(12))),
                    buffer,
                    color,
                );
                if let Some(document) = &decision.diff {
                    DiffView {
                        document,
                        state: &decision.diff_view,
                        palette,
                        color,
                        left_label: "repo",
                        right_label: "live",
                    }
                    .render(
                        Rect::new(
                            area.x,
                            area.y + 2,
                            area.width,
                            area.height.saturating_sub(4),
                        ),
                        buffer,
                    );
                }
                render_decision_line(
                    area,
                    area.height.saturating_sub(2),
                    Line::from(overwrite_spans(palette, decision, *subject, color)),
                    buffer,
                );
                render_decision_line(
                    area,
                    area.height.saturating_sub(1),
                    Line::from("  y yes · n no · a all · s skip · j/k scroll · q quit"),
                    buffer,
                );
                return;
            }
            let display_path = compact_text(&super::compact_path(path), width.max(8));
            if spacious {
                render_decision_line(
                    area,
                    0,
                    Line::from(Span::styled(
                        format!("  {}{position}", subject.title()),
                        ui_style(color, theme_color(palette, Role::Conflict), Modifier::BOLD),
                    )),
                    buffer,
                );
                render_labeled_value(palette, area, 1, "path", &display_path, buffer, color);
                render_labeled_value(
                    palette,
                    area,
                    2,
                    "found",
                    &compact_text(detail, width.max(8)),
                    buffer,
                    color,
                );
            } else {
                render_decision_line(
                    area,
                    0,
                    Line::from(vec![
                        Span::styled(
                            "  REPLACE  ",
                            ui_style(color, theme_color(palette, Role::Conflict), Modifier::BOLD),
                        ),
                        Span::styled(
                            display_path,
                            ui_style(color, theme_color(palette, Role::Plain), Modifier::empty()),
                        ),
                    ]),
                    buffer,
                );
                render_labeled_value(
                    palette,
                    area,
                    1,
                    "found",
                    &compact_text(detail, width.max(8)),
                    buffer,
                    color,
                );
            }
            let choice_row = if spacious {
                area.height.saturating_sub(2)
            } else {
                area.height.saturating_sub(1)
            };
            render_decision_line(
                area,
                choice_row,
                Line::from(overwrite_spans(palette, decision, *subject, color)),
                buffer,
            );
            if spacious {
                render_decision_line(
                    area,
                    area.height.saturating_sub(1),
                    Line::from("  y yes · n no · a all · s skip · q quit"),
                    buffer,
                );
            }
            return;
        }
        Prompt::RemoteChanges { host, changes } => {
            let host = compact_text(host, width.max(8));
            let count = changes.len();
            let count_label = plural(count, "change", "changes");
            render_decision_line(
                area,
                0,
                Line::from(vec![Span::styled(
                    "  REMOTE CHANGES",
                    ui_style(color, theme_color(palette, Role::Warning), Modifier::BOLD),
                )]),
                buffer,
            );
            render_labeled_value(
                palette,
                area,
                1,
                &host,
                &format!("{count} incoming {count_label}"),
                buffer,
                color,
            );
            if let Some(change) = changes.first() {
                let remaining = count.saturating_sub(1);
                let suffix = if remaining == 0 {
                    String::new()
                } else {
                    format!("  +{remaining} more")
                };
                render_labeled_value(
                    palette,
                    area,
                    2,
                    "first",
                    &format!("{}{suffix}", compact_text(change, width.max(8))),
                    buffer,
                    color,
                );
            }
        }
    }
    let choice_row = if spacious {
        area.height.saturating_sub(2)
    } else {
        area.height.saturating_sub(1)
    };
    render_decision_line(
        area,
        choice_row,
        Line::from(choice_spans(palette, decision, color)),
        buffer,
    );
    if spacious {
        render_decision_line(
            area,
            area.height.saturating_sub(1),
            Line::from(Span::styled(
                "  ←/→ navigate  |  enter confirm  |  q cancel",
                ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
            )),
            buffer,
        );
    }
}

fn render_labeled_value(
    palette: &Palette,
    area: Rect,
    row: u16,
    label: &str,
    value: &str,
    buffer: &mut Buffer,
    color: bool,
) {
    render_decision_line(
        area,
        row,
        Line::from(vec![
            Span::styled(
                format!("  {label:<7}"),
                ui_style(color, theme_color(palette, Role::Muted), Modifier::BOLD),
            ),
            Span::styled(
                value.to_string(),
                ui_style(color, theme_color(palette, Role::Strong), Modifier::empty()),
            ),
        ]),
        buffer,
    );
}

fn render_decision_line(area: Rect, row: u16, line: Line<'static>, buffer: &mut Buffer) {
    if row < area.height {
        Paragraph::new(line).render(line_area(area, row), buffer);
    }
}

fn choice_spans(palette: &Palette, decision: &DecisionState, color: bool) -> Vec<Span<'static>> {
    if let Prompt::MergeTarget { targets, .. } = &decision.request.prompt {
        let choice = decision.selected();
        let label = match choice {
            Choice::Target(index) => targets
                .get(index)
                .map(|target| compact_text(target, 32))
                .unwrap_or_else(|| format!("target {}", index + 1)),
            Choice::Cancel => "cancel".to_string(),
            _ => choice_name(choice).to_string(),
        };
        let position = match choice {
            Choice::Target(index) => format!("{}/{}", index + 1, targets.len()),
            _ => "safe cancel".to_string(),
        };
        return vec![
            Span::raw("  ‹  "),
            Span::styled(
                format!(" {label} "),
                ui_style(
                    color,
                    choice_color(palette, choice),
                    Modifier::BOLD | Modifier::REVERSED,
                ),
            ),
            Span::raw("  ›  "),
            Span::styled(
                format!("{position}   ↩ to confirm"),
                ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
            ),
        ];
    }
    let mut spans = vec![Span::raw("  ")];
    for (index, choice) in decision.choices.iter().copied().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        let selected = index == decision.selected;
        let modifier = if selected {
            Modifier::BOLD | Modifier::REVERSED
        } else {
            Modifier::empty()
        };
        spans.push(Span::styled(
            format!(" {} ", choice_name(choice)),
            ui_style(color, choice_color(palette, choice), modifier),
        ));
    }
    spans.push(Span::styled(
        "   ↩ to confirm",
        ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
    ));
    spans
}

fn selected_target_label(decision: &DecisionState, targets: &[String], limit: usize) -> String {
    match decision.selected() {
        Choice::Target(index) => targets
            .get(index)
            .map(|target| compact_text(target, limit))
            .unwrap_or_else(|| format!("target {}", index + 1)),
        Choice::Cancel => "cancel".to_string(),
        choice => choice_name(choice).to_string(),
    }
}

fn compact_text(value: &str, limit: usize) -> String {
    let mut result = String::with_capacity(value.len().min(limit + 1));
    let mut truncated = false;
    for (index, character) in value.chars().enumerate() {
        if index == limit {
            truncated = true;
            break;
        }
        match character {
            '\n' | '\r' => result.push('↵'),
            '\t' => result.push(' '),
            value if value.is_control() => result.push('�'),
            value => result.push(value),
        }
    }
    if truncated {
        result.pop();
        result.push('…');
    }
    result
}

fn decision_choices(prompt: &Prompt) -> Vec<Choice> {
    match prompt {
        Prompt::Merge { .. } => MERGE_CHOICES.to_vec(),
        Prompt::MergeTarget { targets, .. } => (0..targets.len())
            .map(Choice::Target)
            .chain(std::iter::once(Choice::Cancel))
            .collect(),
        Prompt::RemoteChanges { .. } => REMOTE_CHOICES.to_vec(),
        Prompt::Overwrite { .. } => OVERWRITE_CHOICES.to_vec(),
    }
}

fn choice_name(choice: Choice) -> &'static str {
    match choice {
        Choice::Repo => "repo",
        Choice::Live => "live",
        Choice::Ignore => "ignore",
        Choice::Target(_) => "target",
        Choice::Skip => "skip",
        Choice::Abort => "abort",
        Choice::Discard => "discard",
        Choice::Cancel => "cancel",
        Choice::Overwrite => "yes",
        Choice::Keep => "no",
        Choice::OverwriteAll => "all",
        Choice::KeepAll => "skip",
    }
}

fn choice_color(palette: &Palette, choice: Choice) -> Color {
    match choice {
        Choice::Repo => theme_color(palette, Role::Ours),
        Choice::Live => theme_color(palette, Role::Theirs),
        Choice::Ignore => theme_color(palette, Role::Muted),
        Choice::Target(_) => theme_color(palette, Role::Theirs),
        Choice::Skip | Choice::Cancel => theme_color(palette, Role::Warning),
        Choice::Abort | Choice::Discard => theme_color(palette, Role::Danger),
        Choice::Overwrite => theme_color(palette, Role::Ours),
        Choice::OverwriteAll => theme_color(palette, Role::Danger),
        Choice::Keep => theme_color(palette, Role::Muted),
        Choice::KeepAll => theme_color(palette, Role::Warning),
    }
}

fn render_header(palette: &Palette, model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let mode = if model.peer.is_some() {
        "PUSH"
    } else if model.dry_run {
        "PLAN"
    } else {
        "SYNC"
    };
    let mut spans = vec![
        Span::styled(
            "  ",
            ui_style(color, theme_color(palette, Role::Accent), Modifier::BOLD),
        ),
        Span::styled(
            "DOTFILE",
            ui_style(color, theme_color(palette, Role::Strong), Modifier::BOLD),
        ),
        Span::styled(
            "  /  ",
            ui_style(color, theme_color(palette, Role::Border), Modifier::empty()),
        ),
        Span::styled(
            mode,
            ui_style(color, theme_color(palette, Role::Accent), Modifier::BOLD),
        ),
    ];
    if !model.profile.is_empty() {
        spans.push(Span::styled(
            format!("  {}", model.profile),
            ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
        ));
    }
    if let Some(peer) = &model.peer {
        spans.push(Span::styled(
            format!("  →  {peer}"),
            ui_style(color, theme_color(palette, Role::Accent), Modifier::empty()),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn render_push_track(
    palette: &Palette,
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    color: bool,
) {
    let push_seen = model.seen_phases.contains(&Phase::Push);
    let remote_seen = model.seen_phases.contains(&Phase::Remote);
    let finished = model.finished.is_some();
    let state = |done, active| {
        if done {
            PhaseState::Complete
        } else if active {
            PhaseState::Active
        } else {
            PhaseState::Pending
        }
    };
    let phases = [
        (
            "local",
            state(
                push_seen || remote_seen || finished,
                !push_seen && !remote_seen && !finished,
            ),
        ),
        (
            "origin",
            state(
                remote_seen || finished,
                model.phase == Some(Phase::Push) && !finished,
            ),
        ),
        (
            "peer",
            state(finished, model.phase == Some(Phase::Remote) && !finished),
        ),
    ];
    let mut line = ui_progress::phase_track(&phases, palette, color);
    line.spans.insert(0, Span::raw("  "));
    Paragraph::new(line).render(area, buffer);
}

fn render_status(
    palette: &Palette,
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    frame_index: u64,
    color: bool,
) {
    let line = if let Some(summary) = &model.finished {
        let (symbol, state, foreground) = if summary.dry_run {
            ("◇", "PLAN READY", theme_color(palette, Role::Warning))
        } else if summary.peer.is_some() {
            ("✓", "PUSHED", theme_color(palette, Role::Success))
        } else if summary.changed == 0 {
            ("✓", "CURRENT", theme_color(palette, Role::Success))
        } else {
            ("✓", "SYNCED", theme_color(palette, Role::Success))
        };
        let detail = if summary.dry_run {
            format!("{} changes pending", summary.changed)
        } else if summary.peer.is_some() {
            match summary.remote_changed {
                Some(remote_changed) => {
                    format!("local {} | peer {remote_changed}", summary.changed)
                }
                None => format!("local {}", summary.changed),
            }
        } else if summary.changed == 0 {
            format!("{} checked", summary.checked)
        } else {
            format!("{} changed | {} checked", summary.changed, summary.checked)
        };
        Line::from(vec![
            Span::styled(
                format!("  {symbol} {state}"),
                ui_style(color, foreground, Modifier::BOLD),
            ),
            Span::styled(
                format!("  {detail} | {} ms", summary.elapsed.as_millis()),
                ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
            ),
        ])
    } else if let Some((message, _)) = &model.failure {
        Line::from(vec![
            Span::styled(
                "  × FAILED  ",
                ui_style(color, theme_color(palette, Role::Danger), Modifier::BOLD),
            ),
            Span::styled(
                message.clone(),
                ui_style(color, theme_color(palette, Role::Danger), Modifier::empty()),
            ),
        ])
    } else if model.cancelling {
        Line::from(vec![
            Span::styled(
                "  ◌ CANCELLING",
                ui_style(color, theme_color(palette, Role::Warning), Modifier::BOLD),
            ),
            Span::styled(
                "  waiting for the current operation",
                ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
            ),
        ])
    } else {
        let spinner = ui_progress::Spinner::Quarter.frame(frame_index, true);
        let phase = model.phase.map(super::phase_name).unwrap_or("preparing");
        let counter = match model.total {
            Some(total) => format!("  {}/{}", model.completed, total),
            None if model.completed > 0 => format!("  {}", model.completed),
            None => String::new(),
        };
        let label = if model.verbose && !model.label.is_empty() {
            format!("  |  {}", model.label)
        } else {
            String::new()
        };
        Line::from(vec![
            Span::styled(
                format!("  {spinner} {phase}"),
                ui_style(color, theme_color(palette, Role::Info), Modifier::BOLD),
            ),
            Span::styled(
                format!("{counter}{label}"),
                ui_style(color, theme_color(palette, Role::Muted), Modifier::empty()),
            ),
        ])
    };
    Paragraph::new(line).render(area, buffer);
}

fn render_progress(
    palette: &Palette,
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    frame_index: u64,
    color: bool,
) {
    if model.finished.is_some() || model.failure.is_some() {
        render_summary_breakdown(palette, model, area, buffer, color);
        return;
    }
    ProgressBar {
        progress: Progress {
            completed: model.completed as u64,
            total: model.total.map(|total| total as u64),
        },
        frame: frame_index,
        palette,
        color,
    }
    .render(inset(area, 2), buffer);
}

fn render_summary_breakdown(
    palette: &Palette,
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    color: bool,
) {
    let Some(summary) = &model.finished else {
        return;
    };
    let mut parts = Vec::new();
    if summary.links > 0 {
        parts.push(summary_part(summary.links, "link", "links"));
    }
    if summary.merges > 0 {
        parts.push(summary_part(summary.merges, "merge", "merges"));
    }
    if summary.secrets > 0 {
        parts.push(summary_part(summary.secrets, "secret", "secrets"));
    }
    if summary.generated > 0 {
        parts.push(summary_part(summary.generated, "generated", "generated"));
    }
    if !parts.is_empty() {
        Paragraph::new(format!("    {}", parts.join("  |  ")))
            .style(ui_style(
                color,
                theme_color(palette, Role::Muted),
                Modifier::empty(),
            ))
            .render(area, buffer);
    }
}

fn summary_part(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn render_items(palette: &Palette, model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ui_style(
            color,
            theme_color(palette, Role::Border),
            Modifier::empty(),
        ))
        .title(Span::styled(
            " activity ",
            ui_style(color, theme_color(palette, Role::Muted), Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buffer);
    let available = inner.height as usize;
    let skip = model.items.len().saturating_sub(available);
    let lines = model
        .items
        .iter()
        .skip(skip)
        .map(|line| {
            Line::from(vec![
                Span::styled(
                    "  › ",
                    ui_style(color, theme_color(palette, Role::Accent), Modifier::BOLD),
                ),
                Span::styled(
                    line.clone(),
                    ui_style(color, theme_color(palette, Role::Plain), Modifier::empty()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(inner, buffer);
}

fn render_notice(palette: &Palette, model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let line = match &model.warning {
        Some((message, _)) => Line::from(vec![
            Span::styled(
                "  ! ",
                ui_style(color, theme_color(palette, Role::Warning), Modifier::BOLD),
            ),
            Span::styled(
                message.clone(),
                ui_style(
                    color,
                    theme_color(palette, Role::Warning),
                    Modifier::empty(),
                ),
            ),
        ]),
        None => Line::from(Span::styled(
            "  ctrl-c to cancel",
            ui_style(color, theme_color(palette, Role::Border), Modifier::empty()),
        )),
    };
    Paragraph::new(line).render(area, buffer);
}

fn phase_effect(palette: &Palette, policy: UiPolicy) -> Effect {
    let color = if policy.color {
        theme_color(palette, Role::Border)
    } else {
        Color::DarkGray
    };
    fx::fade_from_fg(color, (180, Interpolation::CubicOut)).with_filter(CellFilter::Text)
}

fn line_area(area: Rect, row: u16) -> Rect {
    Rect::new(area.x, area.y + row, area.width, 1)
}

fn inset(area: Rect, horizontal: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(horizontal),
        area.y,
        area.width.saturating_sub(horizontal.saturating_mul(2)),
        area.height,
    )
}

struct InlineTerminal {
    inline: Inline<Stderr>,
    height: u16,
}

impl InlineTerminal {
    fn new(height: u16, signals: SignalGuard) -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(io::Error::other(
                "interactive terminal input is unavailable",
            ));
        }
        let inline = Inline::with_signals(io::stderr(), height, Teardown::ClearViewport, signals)?;
        Ok(Self { inline, height })
    }

    fn draw(
        &mut self,
        palette: &Palette,
        model: &mut UiModel,
        frame_index: u64,
        policy: UiPolicy,
        effect: Option<&mut Effect>,
        tick: Duration,
    ) -> Result<(), String> {
        let height = model.desired_height();
        if self.height != height {
            self.inline
                .resize_viewport(io::stderr(), height)
                .map_err(|error| format!("unable to resize sync status: {error}"))?;
            self.height = height;
        }
        self.inline
            .terminal()
            .draw(|frame| {
                let area = frame.area();
                model.fit_diff_view(area.width, area.height);
                render_buffer_with_palette(
                    palette,
                    model,
                    area,
                    frame.buffer_mut(),
                    frame_index,
                    policy.color,
                );
                if let Some(effect) = effect {
                    frame.render_effect(effect, area, tachyonfx::Duration::from(tick));
                }
            })
            .map(|_| ())
            .map_err(|error| format!("unable to render sync status: {error}"))
    }

    fn write_scrollback(
        &mut self,
        palette: &Palette,
        lines: &[String],
        color: bool,
    ) -> Result<(), String> {
        for chunk in lines.chunks(64) {
            let height = chunk.len() as u16;
            self.inline
                .terminal()
                .insert_before(height, |buffer| {
                    let lines = chunk
                        .iter()
                        .map(|line| scrollback_line(palette, line, color))
                        .collect::<Vec<_>>();
                    Paragraph::new(lines).render(buffer.area, buffer);
                })
                .map_err(|error| format!("unable to write sync activity: {error}"))?;
        }
        Ok(())
    }

    fn input(&self) -> Result<InputAction, String> {
        while event::poll(Duration::ZERO)
            .map_err(|error| format!("unable to read terminal: {error}"))?
        {
            match event::read().map_err(|error| format!("unable to read terminal: {error}"))? {
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Char('q')
                        || key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        return Ok(InputAction::Cancel);
                    }
                    let action = match key.code {
                        KeyCode::Left | KeyCode::Up | KeyCode::BackTab => InputAction::Previous,
                        KeyCode::Right | KeyCode::Down | KeyCode::Tab => InputAction::Next,
                        KeyCode::Enter | KeyCode::Char(' ') => InputAction::Confirm,
                        KeyCode::Esc => InputAction::Letter('n'),
                        KeyCode::Char('j') => InputAction::Diff(DiffAction::Down),
                        KeyCode::Char('k') => InputAction::Diff(DiffAction::Up),
                        KeyCode::PageUp => InputAction::Diff(DiffAction::PageUp),
                        KeyCode::PageDown => InputAction::Diff(DiffAction::PageDown),
                        KeyCode::Home | KeyCode::Char('g') => InputAction::Diff(DiffAction::Home),
                        KeyCode::End | KeyCode::Char('G') => InputAction::Diff(DiffAction::End),
                        KeyCode::Char('[') => InputAction::Diff(DiffAction::PreviousHunk),
                        KeyCode::Char(']') => InputAction::Diff(DiffAction::NextHunk),
                        KeyCode::Char('v') => InputAction::Diff(DiffAction::ToggleMode),
                        KeyCode::Char('<') => InputAction::Diff(DiffAction::Left),
                        KeyCode::Char('>') => InputAction::Diff(DiffAction::Right),
                        KeyCode::Char(value @ ('r' | 'l' | 'i' | 's' | 'a' | 'd' | 'y' | 'n')) => {
                            InputAction::Letter(value)
                        }
                        KeyCode::Char(value @ ('R' | 'L' | 'I' | 'S' | 'A' | 'D' | 'Y' | 'N')) => {
                            InputAction::Letter(value.to_ascii_lowercase())
                        }
                        KeyCode::Char(value @ '1'..='9') => {
                            InputAction::SelectIndex(value.to_digit(10).unwrap_or(1) as usize - 1)
                        }
                        _ => InputAction::None,
                    };
                    if action != InputAction::None {
                        return Ok(action);
                    }
                }
                TerminalEvent::Resize(_, _) => return Ok(InputAction::Redraw),
                _ => {}
            }
        }
        Ok(InputAction::None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputAction {
    None,
    Redraw,
    Cancel,
    Previous,
    Next,
    Letter(char),
    SelectIndex(usize),
    Confirm,
    Diff(DiffAction),
}

enum ChannelInput {
    Event(Result<Event, crossbeam_channel::RecvError>),
    Decision(Result<Request, crossbeam_channel::RecvError>),
    Timeout,
}

fn scrollback_line(palette: &Palette, line: &str, color: bool) -> Line<'static> {
    let foreground = if line.starts_with("warning:") {
        theme_color(palette, Role::Warning)
    } else if line.starts_with("  hint:") {
        theme_color(palette, Role::Muted)
    } else {
        theme_color(palette, Role::Plain)
    };
    Line::from(Span::styled(
        line.to_string(),
        ui_style(color, foreground, Modifier::empty()),
    ))
}

#[cfg(test)]
#[path = "../../tests/unit/ui/tui_tests.rs"]
mod tests;
