use std::path::PathBuf;

use hostkit::Route;
use workstation::Style;
use workstation::screen::{self, Key, Screen};

use crate::cli::Direction;
use crate::place;
use crate::remote::{Entry, Listing, Peer, Target};
use crate::report;

const MARKER: &str = "▸ ";
const BLANK: &str = "  ";
const MIN_BOX: usize = 38;
const MAX_BOX: usize = 72;

pub enum Chosen {
    Picked(String),
    Cancelled,
    Unavailable,
}

pub struct Browser<'a> {
    pub direction: Direction,
    pub peer: &'a Peer,
    pub style: &'a Style,
    pub this: &'a str,
    pub route: Option<Route>,
    // What lands on a push, and the entry to open on; absent when a pull was
    // asked for from a home with nothing named in it.
    pub name: Option<String>,
    pub local_display: String,
    pub start: String,
    pub mirror: Option<String>,
    pub remote_home: String,
    pub home: PathBuf,
    pub here: PathBuf,
}

struct State {
    dir: String,
    home: String,
    entries: Vec<Entry>,
    missing: bool,
    cursor: usize,
    offset: usize,
    prompt: Option<String>,
}

impl State {
    fn matching(&self) -> Vec<usize> {
        let Some(text) = self.filter() else {
            return (0..self.entries.len()).collect();
        };
        let needle = text.to_lowercase();
        (0..self.entries.len())
            .filter(|index| self.entries[*index].name.to_lowercase().contains(&needle))
            .collect()
    }

    // A prompt that starts at a root is a place to go, not a name to look for.
    fn filter(&self) -> Option<&str> {
        let text = self.prompt.as_deref()?;
        match text.starts_with('/') || text.starts_with('~') || text.is_empty() {
            true => None,
            false => Some(text),
        }
    }

    fn selected(&self, rows: &[usize]) -> Option<&Entry> {
        rows.get(self.cursor).map(|index| &self.entries[*index])
    }
}

impl Browser<'_> {
    pub fn choose(&self) -> Result<Chosen, String> {
        let listing = self.peer.list(&Target::Absolute(self.start.clone()))?;
        let Some(mut screen) = Screen::open().map_err(|error| error.to_string())? else {
            return Ok(Chosen::Unavailable);
        };

        let mut state = State {
            dir: listing.path.clone(),
            home: listing.home.clone(),
            entries: listing.entries,
            missing: listing.missing,
            cursor: 0,
            offset: 0,
            prompt: None,
        };
        if let Some(name) = &self.name {
            self.aim(&mut state, name);
        }

        loop {
            let rows = state.matching();
            self.settle(&mut state, &rows);
            screen
                .draw(&self.frame(&state, &rows))
                .map_err(|error| error.to_string())?;
            self.warm(&state, &rows);

            let key = screen.key().map_err(|error| error.to_string())?;
            match self.act(&mut state, &rows, key)? {
                Step::Continue => {}
                Step::Cancel => {
                    screen.clear().map_err(|error| error.to_string())?;
                    return Ok(Chosen::Cancelled);
                }
                Step::Accept(path) => {
                    screen.clear().map_err(|error| error.to_string())?;
                    return Ok(Chosen::Picked(path));
                }
            }
        }
    }

    fn act(&self, state: &mut State, rows: &[usize], key: Key) -> Result<Step, String> {
        if state.prompt.is_some() {
            return self.typed(state, rows, key);
        }
        Ok(match key {
            Key::Up | Key::Char('k') => {
                state.cursor = state.cursor.saturating_sub(1);
                Step::Continue
            }
            Key::Down | Key::Char('j') => {
                state.cursor = (state.cursor + 1).min(rows.len().saturating_sub(1));
                Step::Continue
            }
            Key::PageUp => {
                state.cursor = state.cursor.saturating_sub(10);
                Step::Continue
            }
            Key::PageDown => {
                state.cursor = (state.cursor + 10).min(rows.len().saturating_sub(1));
                Step::Continue
            }
            Key::Home | Key::Char('g') => {
                state.cursor = 0;
                Step::Continue
            }
            Key::End | Key::Char('G') => {
                state.cursor = rows.len().saturating_sub(1);
                Step::Continue
            }
            Key::Right | Key::Tab | Key::Char('l') => {
                if let Some(entry) = state.selected(rows)
                    && entry.directory
                {
                    let into = place::join(&state.dir, &entry.name);
                    self.enter(state, &into)?;
                }
                Step::Continue
            }
            Key::Left | Key::Char('h') => {
                let name = place::name_of(&state.dir).to_string();
                let up = place::parent_of(&state.dir).to_string();
                if up != state.dir {
                    self.enter(state, &up)?;
                    self.aim(state, &name);
                }
                Step::Continue
            }
            Key::Char('/') => {
                state.prompt = Some(String::new());
                Step::Continue
            }
            Key::Enter => self.accept(state, rows),
            Key::Escape | Key::Interrupt | Key::Char('q') => Step::Cancel,
            _ => Step::Continue,
        })
    }

    fn typed(&self, state: &mut State, rows: &[usize], key: Key) -> Result<Step, String> {
        let mut text = state.prompt.take().unwrap_or_default();
        Ok(match key {
            Key::Escape => Step::Continue,
            Key::Interrupt => Step::Cancel,
            Key::Backspace => {
                text.pop();
                state.prompt = Some(text);
                Step::Continue
            }
            Key::Kill => {
                state.prompt = Some(String::new());
                Step::Continue
            }
            Key::WordBack => {
                let kept = text.trim_end_matches(|c: char| c != '/');
                let kept = kept.strip_suffix('/').unwrap_or(kept);
                state.prompt = Some(kept.to_string());
                Step::Continue
            }
            Key::Char(character) => {
                text.push(character);
                state.prompt = Some(text);
                state.cursor = 0;
                Step::Continue
            }
            Key::Enter if text.starts_with('/') || text.starts_with('~') => {
                let target = place::expand_remote(&text, &state.home);
                self.enter(state, &target)?;
                Step::Continue
            }
            Key::Enter => self.accept(state, rows),
            _ => {
                state.prompt = Some(text);
                Step::Continue
            }
        })
    }

    fn accept(&self, state: &State, rows: &[usize]) -> Step {
        Step::Accept(self.resolved(state, rows))
    }

    fn enter(&self, state: &mut State, path: &str) -> Result<(), String> {
        let listing = self.peer.list(&Target::Absolute(path.to_string()))?;
        self.settle_into(state, listing);
        Ok(())
    }

    fn settle_into(&self, state: &mut State, listing: Listing) {
        state.dir = listing.path;
        state.entries = listing.entries;
        state.missing = listing.missing;
        state.cursor = 0;
        state.offset = 0;
        state.prompt = None;
    }

    // Landing on the mirror is the common case, so it is put in the middle of
    // the box rather than wherever the smallest scroll would leave it.
    fn aim(&self, state: &mut State, name: &str) {
        if let Some(found) = state.entries.iter().position(|entry| entry.name == name) {
            state.cursor = found;
            state.offset = found.saturating_sub(self.height() / 2);
        }
    }

    fn settle(&self, state: &mut State, rows: &[usize]) {
        state.cursor = state.cursor.min(rows.len().saturating_sub(1));
        let height = self.height();
        if state.cursor < state.offset {
            state.offset = state.cursor;
        }
        if state.cursor >= state.offset + height {
            state.offset = state.cursor + 1 - height;
        }
        state.offset = state
            .offset
            .min(rows.len().saturating_sub(height.min(rows.len())));
    }

    // The directory under the cursor is the one about to be asked for, so it
    // is fetched while the answer is still a keystroke away.
    fn warm(&self, state: &State, rows: &[usize]) {
        if let Some(entry) = state.selected(rows)
            && entry.directory
        {
            self.peer.prefetch(place::join(&state.dir, &entry.name));
        }
    }

    fn height(&self) -> usize {
        workstation::terminal_height()
            .unwrap_or(24)
            .saturating_sub(13)
            .clamp(3, 14)
    }

    fn box_width(&self, state: &State, rows: &[usize]) -> usize {
        let longest = rows
            .iter()
            .map(|index| state.entries[*index].name.chars().count() + 12)
            .max()
            .unwrap_or(0);
        let outer = workstation::terminal_width()
            .unwrap_or(80)
            .saturating_sub(6);
        longest.clamp(MIN_BOX, MAX_BOX.min(outer.max(MIN_BOX)))
    }

    fn frame(&self, state: &State, rows: &[usize]) -> Vec<String> {
        let style = self.style;
        let peer = self.peer.host();
        let mut lines = vec![
            report::header(style, self.direction, self.this, peer, self.route),
            String::new(),
        ];
        lines.extend(self.sides(state, rows));
        lines.push(String::new());
        lines.extend(self.listing(state, rows));
        lines.push(String::new());
        lines.push(self.footer(state));
        lines
    }

    fn sides(&self, state: &State, rows: &[usize]) -> Vec<String> {
        let style = self.style;
        let chosen = self.resolved(state, rows);
        let local = format!("{}:{}", self.this, self.landing(&chosen));
        let remote = format!(
            "{}:{}",
            self.peer.host(),
            place::shorten(&chosen, &state.home)
        );
        let (from, to) = match self.direction {
            Direction::Push => (local, remote),
            Direction::Pull => (remote, local),
        };
        vec![
            format!("  {}  {}", style.dim("from"), style.teal(&from)),
            format!("  {}  {}", style.dim("to  "), style.bold(&to)),
        ]
    }

    // What the copy would land on if it were accepted right now, which is
    // what the two lines above the box have to keep saying.
    fn resolved(&self, state: &State, rows: &[usize]) -> String {
        match self.direction {
            Direction::Push => match &self.name {
                Some(name) => place::join(&state.dir, name),
                None => state.dir.clone(),
            },
            Direction::Pull => match state.selected(rows) {
                Some(entry) => place::join(&state.dir, &entry.name),
                None => state.dir.clone(),
            },
        }
    }

    // A pull lands beside whatever the cursor is on, so the line saying where
    // is recomputed rather than fixed when the browser opened.
    fn landing(&self, chosen: &str) -> String {
        match self.direction {
            Direction::Push => self.local_display.clone(),
            Direction::Pull => place::landing(chosen, &self.remote_home, &self.home, &self.here)
                .map(|(_, shown)| shown)
                .unwrap_or_else(|_| "(nowhere under this home)".to_string()),
        }
    }

    fn listing(&self, state: &State, rows: &[usize]) -> Vec<String> {
        let style = self.style;
        let inner = self.box_width(state, rows);
        let title = place::shorten(&state.dir, &state.home);
        let title = format!("{}:{}", self.peer.host(), title);
        let title = screen::fit(&title, inner.saturating_sub(4));
        let rule = "─".repeat(inner.saturating_sub(title.chars().count()));
        let mut lines = vec![format!("  ┌ {} {}┐", style.bold(&title), style.dim(&rule))];

        let height = self.height();
        if rows.is_empty() {
            let note = match state.missing {
                true => "(not there yet, it will be created)",
                false => "(empty)",
            };
            lines.push(self.row(note, inner));
        }
        let shown = rows.iter().enumerate().skip(state.offset).take(height);
        for (slot, index) in shown {
            let entry = &state.entries[*index];
            lines.push(self.entry(entry, slot == state.cursor, state, inner));
        }
        if let Some(more) = elsewhere(state.offset, height, rows.len()) {
            lines.push(self.row(&more, inner));
        }
        lines.push(format!("  └{}┘", "─".repeat(inner + 2)));
        lines
    }

    fn entry(&self, entry: &Entry, active: bool, state: &State, inner: usize) -> String {
        let style = self.style;
        let tag = self.tag(entry, state);
        let label = match entry.directory {
            true => format!("{}/", entry.name),
            false => entry.name.clone(),
        };
        let room = inner.saturating_sub(2 + tag.chars().count() + 2);
        let label = screen::fit(&label, room);
        let used = 2 + label.chars().count() + tag.chars().count();
        let gap = " ".repeat(inner.saturating_sub(used));

        let head = match active {
            true => format!("{}{}", style.teal(MARKER), style.bold(&label)),
            false => format!("{BLANK}{label}"),
        };
        format!("  │ {head}{gap}{} │", style.dim(&tag))
    }

    fn tag(&self, entry: &Entry, state: &State) -> String {
        if Some(&entry.name) != self.name.as_ref() {
            return String::new();
        }
        let Some(mirror) = &self.mirror else {
            return String::new();
        };
        let mirrored = state.dir == place::parent_of(mirror);
        match (mirrored, self.direction) {
            (true, _) => "mirror".to_string(),
            (false, Direction::Push) => "replaces".to_string(),
            (false, Direction::Pull) => String::new(),
        }
    }

    fn row(&self, text: &str, inner: usize) -> String {
        let text = screen::fit(text, inner.saturating_sub(2));
        let gap = " ".repeat(inner.saturating_sub(2 + text.chars().count()));
        format!("  │ {BLANK}{}{gap} │", self.style.dim(&text))
    }

    fn footer(&self, state: &State) -> String {
        let style = self.style;
        if let Some(text) = &state.prompt {
            let label = match text.starts_with('/') || text.starts_with('~') {
                true => "path",
                false => "find",
            };
            return format!(
                "  {} {}{}   {}",
                style.dim(label),
                style.bold(text),
                style.teal("▏"),
                style.dim("⏎ go   esc back"),
            );
        }
        let accept = match self.direction {
            Direction::Push => "⏎ push here",
            Direction::Pull => "⏎ pull this",
        };
        style.dim(&format!(
            "  ↑↓ move   → open   ← up   {accept}   / find   esc cancel"
        ))
    }
}

// What the box is not showing, in whichever direction it is not showing it.
fn elsewhere(offset: usize, height: usize, total: usize) -> Option<String> {
    let above = offset;
    let below = total.saturating_sub(offset + height);
    match (above, below) {
        (0, 0) => None,
        (0, below) => Some(format!("… {below} below")),
        (above, 0) => Some(format!("… {above} above")),
        (above, below) => Some(format!("… {above} above · {below} below")),
    }
}

enum Step {
    Continue,
    Cancel,
    Accept(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(names: &[(&str, bool)]) -> Vec<Entry> {
        names
            .iter()
            .map(|(name, directory)| Entry {
                name: (*name).to_string(),
                directory: *directory,
            })
            .collect()
    }

    fn state(prompt: Option<&str>) -> State {
        State {
            dir: "/home/fredrir/projects".into(),
            home: "/home/fredrir".into(),
            entries: entries(&[("my-app", true), ("scratch", true), ("notes.md", false)]),
            missing: false,
            cursor: 0,
            offset: 0,
            prompt: prompt.map(str::to_string),
        }
    }

    #[test]
    fn with_no_prompt_every_entry_is_a_row() {
        assert_eq!(state(None).matching(), vec![0, 1, 2]);
    }

    #[test]
    fn a_typed_name_narrows_the_rows_without_regard_to_case() {
        let state = state(Some("MY"));
        assert_eq!(state.matching(), vec![0]);
        assert_eq!(state.filter(), Some("MY"));
    }

    #[test]
    fn a_typed_path_is_somewhere_to_go_rather_than_a_filter() {
        assert_eq!(state(Some("/etc")).filter(), None);
        assert_eq!(state(Some("~/go")).filter(), None);
        assert_eq!(state(Some("/etc")).matching(), vec![0, 1, 2]);
    }

    #[test]
    fn an_empty_prompt_hides_nothing() {
        assert_eq!(state(Some("")).filter(), None);
        assert_eq!(state(Some("")).matching(), vec![0, 1, 2]);
    }

    #[test]
    fn a_prompt_that_matches_nothing_leaves_no_selection() {
        let state = state(Some("zzz"));
        let rows = state.matching();
        assert!(rows.is_empty());
        assert!(state.selected(&rows).is_none());
    }

    fn browser<'a>(direction: Direction, peer: &'a Peer, style: &'a Style) -> Browser<'a> {
        Browser {
            direction,
            peer,
            style,
            this: "macie",
            route: Some(Route::Cable),
            name: Some("my-app".into()),
            local_display: "~/projects/my-app".into(),
            start: "/home/fredrir/projects".into(),
            mirror: Some("/home/fredrir/projects/my-app".into()),
            remote_home: "/home/fredrir".into(),
            home: PathBuf::from("/Users/fredrir"),
            here: PathBuf::from("/Users/fredrir/projects"),
        }
    }

    #[test]
    fn a_push_lands_the_item_in_whatever_directory_is_open() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let mut state = state(None);
        let rows = state.matching();
        assert_eq!(
            browser.resolved(&state, &rows),
            "/home/fredrir/projects/my-app"
        );
        state.dir = "/home/fredrir/scratch".into();
        assert_eq!(
            browser.resolved(&state, &rows),
            "/home/fredrir/scratch/my-app"
        );
    }

    #[test]
    fn a_pull_takes_whatever_the_cursor_is_on() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Pull, &peer, &style);
        let mut state = state(None);
        let rows = state.matching();
        assert_eq!(
            browser.resolved(&state, &rows),
            "/home/fredrir/projects/my-app"
        );
        state.cursor = 2;
        assert_eq!(
            browser.resolved(&state, &rows),
            "/home/fredrir/projects/notes.md"
        );
    }

    #[test]
    fn a_pull_from_an_empty_directory_takes_the_directory() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Pull, &peer, &style);
        let mut state = state(None);
        state.entries.clear();
        let rows = state.matching();
        assert_eq!(browser.resolved(&state, &rows), "/home/fredrir/projects");
    }

    #[test]
    fn the_mirror_is_marked_only_where_it_actually_is() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let mut state = state(None);
        let mine = Entry {
            name: "my-app".into(),
            directory: true,
        };
        let other = Entry {
            name: "scratch".into(),
            directory: true,
        };
        assert_eq!(browser.tag(&mine, &state), "mirror");
        assert_eq!(browser.tag(&other, &state), "");
        state.dir = "/home/fredrir/elsewhere".into();
        assert_eq!(browser.tag(&mine, &state), "replaces");
    }

    #[test]
    fn a_pull_does_not_claim_it_replaces_anything() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Pull, &peer, &style);
        let mut state = state(None);
        state.dir = "/home/fredrir/elsewhere".into();
        let mine = Entry {
            name: "my-app".into(),
            directory: true,
        };
        assert_eq!(browser.tag(&mine, &state), "");
    }

    #[test]
    fn every_row_of_the_box_is_the_same_width() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let state = state(None);
        let rows = state.matching();
        let lines = browser.listing(&state, &rows);
        let widths: Vec<usize> = lines.iter().map(|line| screen::width(line)).collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "the box is ragged: {widths:?}"
        );
    }

    #[test]
    fn a_long_name_is_cut_rather_than_allowed_to_widen_the_box() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let mut state = state(None);
        state.entries = entries(&[("a".repeat(400).as_str(), false)]);
        let rows = state.matching();
        let lines = browser.listing(&state, &rows);
        for line in &lines {
            assert!(
                screen::width(line) <= MAX_BOX + 6,
                "{}",
                screen::width(line)
            );
        }
        let widths: Vec<usize> = lines.iter().map(|line| screen::width(line)).collect();
        assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn an_empty_directory_still_draws_a_box_that_says_why() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        let mut state = state(None);
        state.entries.clear();
        state.missing = true;
        let rows = state.matching();
        let lines = browser.listing(&state, &rows);
        assert!(lines.iter().any(|line| line.contains("will be created")));
        let widths: Vec<usize> = lines.iter().map(|line| screen::width(line)).collect();
        assert!(widths.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn a_box_showing_everything_says_nothing_about_elsewhere() {
        assert_eq!(elsewhere(0, 14, 10), None);
        assert_eq!(elsewhere(0, 14, 14), None);
    }

    #[test]
    fn a_box_reports_what_is_off_each_end() {
        assert_eq!(elsewhere(0, 14, 75), Some("… 61 below".to_string()));
        assert_eq!(elsewhere(61, 14, 75), Some("… 61 above".to_string()));
        assert_eq!(
            elsewhere(10, 14, 75),
            Some("… 10 above · 51 below".to_string())
        );
    }

    #[test]
    fn the_footer_says_what_enter_would_do() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        assert!(
            browser(Direction::Push, &peer, &style)
                .footer(&state(None))
                .contains("push here")
        );
        assert!(
            browser(Direction::Pull, &peer, &style)
                .footer(&state(None))
                .contains("pull this")
        );
    }

    #[test]
    fn the_footer_changes_while_something_is_being_typed() {
        let peer = Peer::new("archie");
        let style = Style::plain();
        let browser = browser(Direction::Push, &peer, &style);
        assert!(browser.footer(&state(Some("my"))).contains("find"));
        assert!(browser.footer(&state(Some("/etc"))).contains("path"));
    }
}
