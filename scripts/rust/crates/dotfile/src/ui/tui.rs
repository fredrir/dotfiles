use std::collections::{HashSet, VecDeque};
use std::io::{self, IsTerminal, Stderr};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tachyonfx::{CellFilter, Effect, EffectRenderer, Interpolation, fx};

use crate::decision::{Choice, Prompt, Request, Server};
use crate::event::{Event, Phase, Summary};

use super::UiPolicy;

const ITEM_CAPACITY: usize = 6;
const EFFECT_FRAME: Duration = Duration::from_millis(33);
const SPINNER_FRAME: Duration = Duration::from_millis(80);
const INPUT_FRAME: Duration = Duration::from_millis(100);
const DECISION_POLL: Duration = Duration::from_millis(25);
#[cfg(unix)]
static TERMINATION_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
const MERGE_CHOICES: [Choice; 5] = [
    Choice::Repo,
    Choice::Live,
    Choice::Ignore,
    Choice::Skip,
    Choice::Abort,
];
const REMOTE_CHOICES: [Choice; 2] = [Choice::Discard, Choice::Cancel];

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
                .position(|choice| *choice == prompt.safe_default())
                .unwrap_or(0),
        };
        Self {
            request,
            choices,
            selected,
        }
    }

    fn selected(&self) -> Choice {
        self.choices[self.selected]
    }
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
            let choice = super::cancellation_choice(&decision.request.prompt);
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
    let signals = match SignalGuard::new() {
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
    let started = Instant::now();
    let mut last_draw = Instant::now();
    let mut dirty = true;
    let mut effect =
        ((phase_changed || model.decision_active()) && policy.motion).then(|| phase_effect(policy));

    let ui_result = (|| -> Result<(), String> {
        loop {
            if crate::cancel::requested() && !model.cancelling {
                if let Some((request, choice)) = model.cancel_response() {
                    let _ = decisions.respond(&request, choice);
                    model.dismiss_decision();
                }
                model.request_cancel();
                dirty = true;
            }
            if !pending_output.is_empty() {
                terminal.write_scrollback(&pending_output, policy.color)?;
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
                terminal.draw(&model, frame_index, policy, effect.as_mut(), tick)?;
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
                InputAction::Select(choice) if model.decision_active() => {
                    model.select_choice(choice);
                    dirty = true;
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
                            effect = Some(phase_effect(policy));
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
                effect = Some(phase_effect(policy));
            }
            for event in receiver.try_iter().take(256) {
                let update = model.apply(&event);
                dirty |= update.redraw;
                pending_output.extend(update.output);
                if update.phase_changed && policy.motion && model.active() {
                    effect = Some(phase_effect(policy));
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
    if area.is_empty() {
        return;
    }
    Clear.render(area, buffer);
    if let Some(decision) = &model.decision {
        render_decision(decision, area, buffer, color);
        return;
    }
    let mut row = 0;
    render_header(model, line_area(area, row), buffer, color);
    row += 1;
    if model.peer.is_some() && row < area.height {
        render_push_track(model, line_area(area, row), buffer, color);
        row += 1;
    }
    if row < area.height {
        render_status(model, line_area(area, row), buffer, frame_index, color);
        row += 1;
    }
    if row < area.height {
        render_progress(model, line_area(area, row), buffer, frame_index, color);
        row += 1;
    }
    if model.verbose && row < area.height {
        let panel = Rect::new(area.x, area.y + row, area.width, area.height - row);
        render_items(model, panel, buffer, color);
    } else if row < area.height {
        render_notice(model, line_area(area, row), buffer, color);
    }
}

fn render_decision(decision: &DecisionState, area: Rect, buffer: &mut Buffer, color: bool) {
    if area.height >= 7 {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(ui_style(color, Color::Rgb(124, 58, 237), Modifier::BOLD))
            .title(Span::styled(
                " decision ",
                ui_style(color, Color::Rgb(196, 181, 253), Modifier::BOLD),
            ));
        let inner = block.inner(area);
        block.render(area, buffer);
        render_decision_body(decision, inner, buffer, color, true);
    } else {
        render_decision_body(decision, area, buffer, color, false);
    }
}

fn render_decision_body(
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
            let display_path = compact_text(&super::compact_path(path), width.max(8));
            let display_key = compact_text(key, width.max(8));
            if spacious {
                render_decision_line(
                    area,
                    0,
                    Line::from(Span::styled(
                        "◆ MERGE CONFLICT",
                        ui_style(color, Color::Rgb(244, 114, 182), Modifier::BOLD),
                    )),
                    buffer,
                );
                render_labeled_value(area, 1, "path", &display_path, buffer, color);
                render_labeled_value(area, 2, "key", &display_key, buffer, color);
                render_labeled_value(
                    area,
                    3,
                    "repo",
                    &compact_text(repo, width.max(8)),
                    buffer,
                    color,
                );
                render_labeled_value(
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
                            "◆ MERGE  ",
                            ui_style(color, Color::Rgb(244, 114, 182), Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{compact_key}  ·  {compact_path}"),
                            ui_style(color, Color::Rgb(203, 213, 225), Modifier::empty()),
                        ),
                    ]),
                    buffer,
                );
                render_labeled_value(
                    area,
                    1,
                    "repo",
                    &compact_text(repo, width.max(8)),
                    buffer,
                    color,
                );
                render_labeled_value(
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
                        "◆ MERGE DESTINATION",
                        ui_style(color, Color::Rgb(129, 140, 248), Modifier::BOLD),
                    )),
                    buffer,
                );
                render_labeled_value(area, 1, "path", &display_path, buffer, color);
                render_labeled_value(area, 2, "key", &display_key, buffer, color);
                render_labeled_value(area, 3, "target", &selected, buffer, color);
                render_labeled_value(
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
                            "◆ TARGET  ",
                            ui_style(color, Color::Rgb(129, 140, 248), Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(
                                "{}  ·  {}",
                                compact_text(key, pair_width),
                                compact_text(&super::compact_path(path), pair_width)
                            ),
                            ui_style(color, Color::Rgb(203, 213, 225), Modifier::empty()),
                        ),
                    ]),
                    buffer,
                );
                render_labeled_value(area, 1, "target", &selected, buffer, color);
                render_labeled_value(
                    area,
                    2,
                    "options",
                    &format!("{} destinations", targets.len()),
                    buffer,
                    color,
                );
            }
        }
        Prompt::RemoteChanges { host, changes } => {
            let host = compact_text(host, width.max(8));
            let count = changes.len();
            let count_label = if count == 1 { "change" } else { "changes" };
            render_decision_line(
                area,
                0,
                Line::from(vec![
                    Span::styled(
                        "◆ REMOTE CHANGES  ",
                        ui_style(color, Color::Rgb(251, 146, 60), Modifier::BOLD),
                    ),
                    Span::styled(
                        host,
                        ui_style(color, Color::Rgb(203, 213, 225), Modifier::empty()),
                    ),
                ]),
                buffer,
            );
            render_labeled_value(
                area,
                1,
                "peer",
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
        Line::from(choice_spans(decision, color)),
        buffer,
    );
    if spacious {
        render_decision_line(
            area,
            area.height.saturating_sub(1),
            Line::from(Span::styled(
                "  ←/→ navigate  ·  enter confirm  ·  q cancel",
                ui_style(color, Color::Rgb(100, 116, 139), Modifier::empty()),
            )),
            buffer,
        );
    }
}

fn render_labeled_value(
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
                ui_style(color, Color::Rgb(100, 116, 139), Modifier::BOLD),
            ),
            Span::styled(
                value.to_string(),
                ui_style(color, Color::Rgb(226, 232, 240), Modifier::empty()),
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

fn choice_spans(decision: &DecisionState, color: bool) -> Vec<Span<'static>> {
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
                    choice_color(choice),
                    Modifier::BOLD | Modifier::REVERSED,
                ),
            ),
            Span::raw("  ›  "),
            Span::styled(
                format!("{position}   enter to confirm"),
                ui_style(color, Color::Rgb(100, 116, 139), Modifier::empty()),
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
            ui_style(color, choice_color(choice), modifier),
        ));
    }
    spans.push(Span::styled(
        "   enter to confirm",
        ui_style(color, Color::Rgb(100, 116, 139), Modifier::empty()),
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
    }
}

fn choice_color(choice: Choice) -> Color {
    match choice {
        Choice::Repo => Color::Rgb(196, 181, 253),
        Choice::Live => Color::Rgb(94, 234, 212),
        Choice::Ignore => Color::Rgb(148, 163, 184),
        Choice::Target(_) => Color::Rgb(129, 140, 248),
        Choice::Skip | Choice::Cancel => Color::Rgb(250, 204, 21),
        Choice::Abort | Choice::Discard => Color::Rgb(248, 113, 113),
    }
}

fn render_header(model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let mode = if model.peer.is_some() {
        "PUSH"
    } else if model.dry_run {
        "PLAN"
    } else {
        "SYNC"
    };
    let mut spans = vec![
        Span::styled(
            "◆ ",
            ui_style(color, Color::Rgb(45, 212, 191), Modifier::BOLD),
        ),
        Span::styled(
            "DOTFILE",
            ui_style(color, Color::Rgb(226, 232, 240), Modifier::BOLD),
        ),
        Span::styled(
            "  /  ",
            ui_style(color, Color::Rgb(71, 85, 105), Modifier::empty()),
        ),
        Span::styled(
            mode,
            ui_style(color, Color::Rgb(167, 139, 250), Modifier::BOLD),
        ),
    ];
    if !model.profile.is_empty() {
        spans.push(Span::styled(
            format!("  {}", model.profile),
            ui_style(color, Color::Rgb(148, 163, 184), Modifier::empty()),
        ));
    }
    if let Some(peer) = &model.peer {
        spans.push(Span::styled(
            format!("  →  {peer}"),
            ui_style(color, Color::Rgb(94, 234, 212), Modifier::empty()),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn render_push_track(model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let push_seen = model.seen_phases.contains(&Phase::Push);
    let remote_seen = model.seen_phases.contains(&Phase::Remote);
    let finished = model.finished.is_some();
    let nodes = [
        (
            "local",
            push_seen || remote_seen || finished,
            !push_seen && !remote_seen && !finished,
        ),
        (
            "origin",
            remote_seen || finished,
            model.phase == Some(Phase::Push) && !finished,
        ),
        (
            "peer",
            finished,
            model.phase == Some(Phase::Remote) && !finished,
        ),
    ];
    let mut spans = vec![Span::raw("  ")];
    for (index, (name, done, current)) in nodes.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(
                " ━━━━━ ",
                ui_style(color, Color::Rgb(51, 65, 85), Modifier::empty()),
            ));
        }
        let (symbol, foreground, modifier) = if done {
            ("●", Color::Rgb(52, 211, 153), Modifier::BOLD)
        } else if current {
            ("◉", Color::Rgb(34, 211, 238), Modifier::BOLD)
        } else {
            ("○", Color::Rgb(71, 85, 105), Modifier::empty())
        };
        spans.push(Span::styled(
            format!("{symbol} {name}"),
            ui_style(color, foreground, modifier),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn render_status(model: &UiModel, area: Rect, buffer: &mut Buffer, frame_index: u64, color: bool) {
    let line = if let Some(summary) = &model.finished {
        let (symbol, state, foreground) = if summary.dry_run {
            ("◇", "PLAN READY", Color::Rgb(250, 204, 21))
        } else if summary.peer.is_some() {
            ("✓", "PUSHED", Color::Rgb(52, 211, 153))
        } else if summary.changed == 0 {
            ("✓", "CURRENT", Color::Rgb(52, 211, 153))
        } else {
            ("✓", "SYNCED", Color::Rgb(52, 211, 153))
        };
        let detail = if summary.dry_run {
            format!("{} changes pending", summary.changed)
        } else if summary.peer.is_some() {
            match summary.remote_changed {
                Some(remote_changed) => {
                    format!("local {} · peer {remote_changed}", summary.changed)
                }
                None => format!("local {}", summary.changed),
            }
        } else if summary.changed == 0 {
            format!("{} checked", summary.checked)
        } else {
            format!("{} changed · {} checked", summary.changed, summary.checked)
        };
        Line::from(vec![
            Span::styled(
                format!("  {symbol} {state}"),
                ui_style(color, foreground, Modifier::BOLD),
            ),
            Span::styled(
                format!("  {detail} · {} ms", summary.elapsed.as_millis()),
                ui_style(color, Color::Rgb(148, 163, 184), Modifier::empty()),
            ),
        ])
    } else if let Some((message, _)) = &model.failure {
        Line::from(vec![
            Span::styled(
                "  × FAILED  ",
                ui_style(color, Color::Rgb(248, 113, 113), Modifier::BOLD),
            ),
            Span::styled(
                message.clone(),
                ui_style(color, Color::Rgb(254, 202, 202), Modifier::empty()),
            ),
        ])
    } else if model.cancelling {
        Line::from(vec![
            Span::styled(
                "  ◌ CANCELLING",
                ui_style(color, Color::Rgb(250, 204, 21), Modifier::BOLD),
            ),
            Span::styled(
                "  waiting for the current operation",
                ui_style(color, Color::Rgb(148, 163, 184), Modifier::empty()),
            ),
        ])
    } else {
        let spinner = ["◐", "◓", "◑", "◒"][(frame_index as usize) % 4];
        let phase = model.phase.map(super::phase_name).unwrap_or("preparing");
        let counter = match model.total {
            Some(total) => format!("  {}/{}", model.completed, total),
            None if model.completed > 0 => format!("  {}", model.completed),
            None => String::new(),
        };
        let label = if model.verbose && !model.label.is_empty() {
            format!("  ·  {}", model.label)
        } else {
            String::new()
        };
        Line::from(vec![
            Span::styled(
                format!("  {spinner} {phase}"),
                ui_style(color, Color::Rgb(34, 211, 238), Modifier::BOLD),
            ),
            Span::styled(
                format!("{counter}{label}"),
                ui_style(color, Color::Rgb(148, 163, 184), Modifier::empty()),
            ),
        ])
    };
    Paragraph::new(line).render(area, buffer);
}

fn render_progress(
    model: &UiModel,
    area: Rect,
    buffer: &mut Buffer,
    frame_index: u64,
    color: bool,
) {
    if model.finished.is_some() || model.failure.is_some() {
        render_summary_breakdown(model, area, buffer, color);
        return;
    }
    if let Some(total) = model.total {
        let ratio = if total == 0 {
            f64::from(model.completed > 0)
        } else {
            (model.completed as f64 / total as f64).clamp(0.0, 1.0)
        };
        let percent = (ratio * 100.0).round() as usize;
        let gauge = Gauge::default()
            .gauge_style(
                ui_style(color, Color::Rgb(45, 212, 191), Modifier::BOLD).bg(if color {
                    Color::Rgb(30, 41, 59)
                } else {
                    Color::Reset
                }),
            )
            .ratio(ratio)
            .label(format!("{} / {}  ·  {percent}%", model.completed, total));
        gauge.render(inset(area, 2), buffer);
    } else {
        let width = area.width.saturating_sub(4) as usize;
        let width = width.max(1);
        let position = frame_index as usize % width;
        let mut spans = Vec::with_capacity(width + 2);
        spans.push(Span::raw("  "));
        for index in 0..width {
            let (symbol, foreground) = if index == position {
                ("━", Color::Rgb(94, 234, 212))
            } else if index.abs_diff(position) == 1 {
                ("━", Color::Rgb(14, 116, 144))
            } else {
                ("─", Color::Rgb(51, 65, 85))
            };
            spans.push(Span::styled(
                symbol,
                ui_style(color, foreground, Modifier::empty()),
            ));
        }
        Paragraph::new(Line::from(spans)).render(area, buffer);
    }
}

fn render_summary_breakdown(model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
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
        Paragraph::new(format!("    {}", parts.join("  ·  ")))
            .style(ui_style(
                color,
                Color::Rgb(100, 116, 139),
                Modifier::empty(),
            ))
            .render(area, buffer);
    }
}

fn summary_part(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn render_items(model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(ui_style(color, Color::Rgb(51, 65, 85), Modifier::empty()))
        .title(Span::styled(
            " activity ",
            ui_style(color, Color::Rgb(148, 163, 184), Modifier::BOLD),
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
                    ui_style(color, Color::Rgb(45, 212, 191), Modifier::BOLD),
                ),
                Span::styled(
                    line.clone(),
                    ui_style(color, Color::Rgb(203, 213, 225), Modifier::empty()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(inner, buffer);
}

fn render_notice(model: &UiModel, area: Rect, buffer: &mut Buffer, color: bool) {
    let line = match &model.warning {
        Some((message, _)) => Line::from(vec![
            Span::styled(
                "  ! ",
                ui_style(color, Color::Rgb(250, 204, 21), Modifier::BOLD),
            ),
            Span::styled(
                message.clone(),
                ui_style(color, Color::Rgb(253, 224, 71), Modifier::empty()),
            ),
        ]),
        None => Line::from(Span::styled(
            "  ctrl-c to cancel",
            ui_style(color, Color::Rgb(71, 85, 105), Modifier::empty()),
        )),
    };
    Paragraph::new(line).render(area, buffer);
}

fn phase_effect(policy: UiPolicy) -> Effect {
    let color = if policy.color {
        Color::Rgb(51, 65, 85)
    } else {
        Color::DarkGray
    };
    fx::fade_from_fg(color, (180, Interpolation::CubicOut)).with_filter(CellFilter::Text)
}

fn ui_style(color: bool, foreground: Color, modifier: Modifier) -> Style {
    let style = if color {
        Style::default().fg(foreground)
    } else {
        Style::default()
    };
    style.add_modifier(modifier)
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
    terminal: Terminal<CrosstermBackend<Stderr>>,
    raw_mode: bool,
    _signals: SignalGuard,
}

impl InlineTerminal {
    fn new(height: u16, signals: SignalGuard) -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(io::Error::other(
                "interactive terminal input is unavailable",
            ));
        }
        enable_raw_mode()?;
        let backend = CrosstermBackend::new(io::stderr());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        );
        let mut terminal = match terminal {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                return Err(error);
            }
        };
        if let Err(error) = terminal.hide_cursor() {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self {
            terminal,
            raw_mode: true,
            _signals: signals,
        })
    }

    fn draw(
        &mut self,
        model: &UiModel,
        frame_index: u64,
        policy: UiPolicy,
        effect: Option<&mut Effect>,
        tick: Duration,
    ) -> Result<(), String> {
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                render_buffer(model, area, frame.buffer_mut(), frame_index, policy.color);
                if let Some(effect) = effect {
                    frame.render_effect(effect, area, tachyonfx::Duration::from(tick));
                }
            })
            .map(|_| ())
            .map_err(|error| format!("unable to render sync status: {error}"))
    }

    fn write_scrollback(&mut self, lines: &[String], color: bool) -> Result<(), String> {
        for chunk in lines.chunks(64) {
            let height = chunk.len() as u16;
            self.terminal
                .insert_before(height, |buffer| {
                    let lines = chunk
                        .iter()
                        .map(|line| scrollback_line(line, color))
                        .collect::<Vec<_>>();
                    Paragraph::new(lines).render(buffer.area, buffer);
                })
                .map_err(|error| format!("unable to write sync activity: {error}"))?;
        }
        Ok(())
    }

    fn input(&self) -> Result<InputAction, String> {
        if !self.raw_mode {
            return Ok(InputAction::None);
        }
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
                        KeyCode::Char('r') => InputAction::Select(Choice::Repo),
                        KeyCode::Char('l') => InputAction::Select(Choice::Live),
                        KeyCode::Char('i') => InputAction::Select(Choice::Ignore),
                        KeyCode::Char('s') => InputAction::Select(Choice::Skip),
                        KeyCode::Char('a') => InputAction::Select(Choice::Abort),
                        KeyCode::Char('d') => InputAction::Select(Choice::Discard),
                        KeyCode::Char('c') => InputAction::Select(Choice::Cancel),
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

#[cfg(unix)]
struct SignalGuard {
    previous: Vec<(libc::c_int, libc::sighandler_t)>,
}

#[cfg(unix)]
impl SignalGuard {
    fn new() -> io::Result<Self> {
        TERMINATION_SIGNAL.store(0, std::sync::atomic::Ordering::Release);
        let mut previous = Vec::with_capacity(3);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let handler = terminal_signal as *const () as libc::sighandler_t;
            let prior = unsafe { libc::signal(signal, handler) };
            if prior == libc::SIG_ERR {
                for (installed, handler) in previous.into_iter().rev() {
                    unsafe { libc::signal(installed, handler) };
                }
                return Err(io::Error::last_os_error());
            }
            previous.push((signal, prior));
        }
        Ok(Self { previous })
    }
}

#[cfg(unix)]
impl Drop for SignalGuard {
    fn drop(&mut self) {
        for (signal, handler) in self.previous.drain(..).rev() {
            unsafe { libc::signal(signal, handler) };
        }
    }
}

#[cfg(unix)]
extern "C" fn terminal_signal(signal: libc::c_int) {
    TERMINATION_SIGNAL.store(signal, std::sync::atomic::Ordering::Release);
    crate::cancel::request();
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
}

#[cfg(unix)]
pub(crate) fn termination_signal() -> libc::c_int {
    TERMINATION_SIGNAL.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(not(unix))]
struct SignalGuard;

#[cfg(not(unix))]
impl SignalGuard {
    fn new() -> io::Result<Self> {
        Ok(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputAction {
    None,
    Redraw,
    Cancel,
    Previous,
    Next,
    Select(Choice),
    SelectIndex(usize),
    Confirm,
}

enum ChannelInput {
    Event(Result<Event, crossbeam_channel::RecvError>),
    Decision(Result<Request, crossbeam_channel::RecvError>),
    Timeout,
}

impl Drop for InlineTerminal {
    fn drop(&mut self) {
        let origin = self.terminal.get_frame().area().as_position();
        let _ = self.terminal.clear();
        let _ = self.terminal.set_cursor_position(origin);
        let _ = self.terminal.show_cursor();
        let _ = self.terminal.backend_mut().flush();
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
    }
}

fn scrollback_line(line: &str, color: bool) -> Line<'static> {
    let foreground = if line.starts_with("warning:") {
        Color::Rgb(250, 204, 21)
    } else if line.starts_with("  hint:") {
        Color::Rgb(148, 163, 184)
    } else {
        Color::Rgb(203, 213, 225)
    };
    Line::from(Span::styled(
        line.to_string(),
        ui_style(color, foreground, Modifier::empty()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_tui_starts_for_planned_link_work_before_apply() {
        assert!(starts_tui(
            &Event::Started {
                profile: "macos".to_string(),
                dry_run: false,
                peer: Some("archie".to_string()),
            },
            false,
        ));
        assert!(!starts_tui(
            &Event::Started {
                profile: "macos".to_string(),
                dry_run: true,
                peer: Some("archie".to_string()),
            },
            false,
        ));
        assert!(starts_tui(
            &Event::PhaseStarted {
                phase: Phase::Links,
                total: Some(1),
            },
            false,
        ));
        assert!(!starts_tui(
            &Event::PhaseStarted {
                phase: Phase::Links,
                total: Some(0),
            },
            false,
        ));
        assert!(!starts_tui(
            &Event::PhaseStarted {
                phase: Phase::Secrets,
                total: Some(1),
            },
            false,
        ));
        assert!(starts_tui(
            &Event::Item {
                action: crate::event::Action::Merge,
                path: std::path::PathBuf::from("settings.json"),
                detail: String::new(),
                changed: true,
            },
            false,
        ));
    }
}
