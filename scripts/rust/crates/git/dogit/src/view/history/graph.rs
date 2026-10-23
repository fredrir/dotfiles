use super::*;

const NODE_WIDTH: usize = 1;
const STEP: usize = 2;
const UP: u8 = 1;
const RIGHT: u8 = 2;
const DOWN: u8 = 4;
const LEFT: u8 = 8;

#[derive(Default)]
pub(super) struct Graph {
    rows: Vec<Row>,
    lanes: usize,
}

struct Lane<'a> {
    oid: &'a str,
    color: usize,
}

struct Row {
    lane: usize,
    color: usize,
    through: Vec<(usize, usize)>,
    parents: Vec<(usize, usize)>,
    joining: Option<(usize, usize)>,
}

impl Graph {
    pub(super) fn new(items: &[NavItem]) -> Self {
        let mut graph = Self::default();
        let mut lanes: Vec<Option<Lane<'_>>> = Vec::new();
        let mut next_color = 0;
        for item in items {
            let Some(commit) = &item.commit else { continue };
            let lane = lanes
                .iter()
                .position(|lane| lane.as_ref().is_some_and(|lane| lane.oid == commit.oid))
                .unwrap_or_else(|| vacant(&mut lanes));
            let color = lanes[lane].take().map_or_else(
                || {
                    let color = next_color;
                    next_color += 1;
                    color
                },
                |lane| lane.color,
            );
            let mut through: Vec<_> = lanes
                .iter()
                .enumerate()
                .filter_map(|(index, lane)| lane.as_ref().map(|lane| (index, lane.color)))
                .collect();
            let mut parents = Vec::new();
            let mut joining = None;
            for (index, parent) in commit.parents.iter().enumerate() {
                let mut target = lanes
                    .iter()
                    .position(|lane| lane.as_ref().is_some_and(|lane| lane.oid == parent))
                    .unwrap_or_else(|| {
                        let target = if index == 0 { lane } else { vacant(&mut lanes) };
                        let color = if index == 0 {
                            color
                        } else {
                            let color = next_color;
                            next_color += 1;
                            color
                        };
                        lanes[target] = Some(Lane { oid: parent, color });
                        target
                    });
                if index == 0 && target > lane {
                    joining = lanes[target].take().map(|parent| (target, parent.color));
                    lanes[lane] = Some(Lane { oid: parent, color });
                    through.retain(|(index, _)| *index != target);
                    target = lane;
                }
                if !parents.iter().any(|(lane, _)| *lane == target) {
                    parents.push((
                        target,
                        lanes[target].as_ref().map_or(color, |lane| lane.color),
                    ));
                }
            }
            graph.lanes = graph.lanes.max(lanes.len());
            graph.rows.push(Row {
                lane,
                color,
                through,
                parents,
                joining,
            });
            while lanes.last().is_some_and(Option::is_none) {
                lanes.pop();
            }
        }
        graph
    }

    pub(super) fn width(&self, available: u16) -> u16 {
        if usize::from(available) < NODE_WIDTH || self.lanes == 0 {
            return 0;
        }
        let lanes = self
            .lanes
            .min((usize::from(available) - NODE_WIDTH) / STEP + 1);
        ((lanes - 1) * STEP + NODE_WIDTH) as u16
    }

    pub(super) fn render(
        &self,
        buffer: &mut Buffer,
        area: Rect,
        index: usize,
        selected_color: Option<Color>,
    ) {
        let Some(row) = self
            .rows
            .get(index)
            .filter(|_| !area.is_empty() && usize::from(area.width) >= NODE_WIDTH)
        else {
            return;
        };
        let width = usize::from(area.width);
        let last_lane = (width - NODE_WIDTH) / STEP;
        let overflow = self.lanes > last_lane + 1;
        let column = |lane: usize| lane.min(last_lane) * STEP + NODE_WIDTH / 2;
        let hidden = |lane: usize| overflow && lane >= last_lane;
        let mut cells = vec![vec![Cell::default(); width]; usize::from(CARD_HEIGHT)];
        for &(lane, color) in &row.through {
            for line in &mut cells {
                line[column(lane)].connect(UP | DOWN, color, hidden(lane));
            }
        }
        let x = column(row.lane);
        if let Some((from, color)) = row.joining {
            let from_x = column(from);
            let clipped = hidden(from) || hidden(row.lane);
            cells[0][from_x].connect(UP | DOWN, color, clipped);
            if from_x == x {
                cells[1][x].connect(UP | DOWN, color, clipped);
            } else {
                cells[1][from_x].connect(UP | LEFT, color, clipped);
                for cell in &mut cells[1][x + 1..from_x] {
                    cell.connect(LEFT | RIGHT, color, clipped);
                }
                cells[1][x].connect(RIGHT | DOWN, color, clipped);
            }
        }
        for &(parent, color) in &row.parents {
            let target = column(parent);
            let clipped = hidden(row.lane) || hidden(parent);
            cells[0][x].connect(DOWN, row.color, clipped);
            if target == x {
                cells[1][x].connect(UP | DOWN, color, clipped);
            } else {
                let (towards, from) = if target > x {
                    (RIGHT, LEFT)
                } else {
                    (LEFT, RIGHT)
                };
                cells[1][x].connect(UP | towards, color, clipped);
                for cell in &mut cells[1][x.min(target) + 1..x.max(target)] {
                    cell.connect(LEFT | RIGHT, color, clipped);
                }
                cells[1][target].connect(from | DOWN, color, clipped);
            }
            for line in cells.iter_mut().skip(2) {
                line[target].connect(UP | DOWN, color, hidden(parent));
            }
        }
        for (y, line) in cells.iter().enumerate().take(usize::from(area.height)) {
            for (x, cell) in line
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.connections != 0)
            {
                put(
                    buffer,
                    area.x + x as u16,
                    area.y + y as u16,
                    1,
                    cell.symbol(),
                    lane_style(cell.color, false),
                );
            }
        }
        let appearance = if let Some(color) = selected_color {
            lane_style(row.color, true).fg(color)
        } else {
            lane_style(row.color, true)
        };
        put(
            buffer,
            area.x + (x - NODE_WIDTH / 2) as u16,
            area.y,
            NODE_WIDTH as u16,
            "●",
            appearance,
        );
    }
}

fn vacant(lanes: &mut Vec<Option<Lane<'_>>>) -> usize {
    lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    })
}

#[derive(Clone, Copy, Default)]
struct Cell {
    connections: u8,
    color: usize,
    clipped: bool,
}

impl Cell {
    fn connect(&mut self, connections: u8, color: usize, clipped: bool) {
        self.connections |= connections;
        self.color = color;
        self.clipped |= clipped;
    }

    fn symbol(self) -> &'static str {
        if self.clipped {
            return if self.connections == LEFT | RIGHT {
                "┄"
            } else {
                "┊"
            };
        }
        match self.connections {
            1 | 4 | 5 => "│",
            2 | 8 | 10 => "─",
            3 => "╰",
            6 => "╭",
            9 => "╯",
            12 => "╮",
            7 => "├",
            11 => "┴",
            13 => "┤",
            14 => "┬",
            15 => "┼",
            _ => " ",
        }
    }
}

fn lane_style(lane: usize, node: bool) -> Style {
    let colors = [
        [130, 215, 233],
        [195, 173, 247],
        [131, 220, 162],
        [242, 183, 157],
        [229, 205, 132],
        [145, 188, 235],
    ];
    let [r, g, b] = colors[lane % colors.len()];
    let color = if color_depth() == ColorDepth::Ansi16 {
        [
            Color::LightCyan,
            Color::LightMagenta,
            Color::LightGreen,
            Color::LightRed,
            Color::LightYellow,
            Color::LightBlue,
        ][lane % colors.len()]
    } else {
        let channel = |value: u8| {
            if node {
                value
            } else {
                (u16::from(value) * 3 / 4) as u8
            }
        };
        ui_theme::Color::Rgb(channel(r), channel(g), channel(b))
            .at_depth(color_depth())
            .ratatui()
    };
    style(Role::Plain).fg(color).add_modifier(if node {
        Modifier::BOLD
    } else {
        Modifier::empty()
    })
}
