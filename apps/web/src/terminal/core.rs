//! The terminal model: an `alacritty_terminal` grid fed with bytes. Ports the
//! role of `terminal/ghostty/core.ts`: parse output, keep scrollback, track
//! modes, selection and search, and report what changed since the last paint.

use std::cell::RefCell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle, Processor, StdSyncHandler};

use super::backend::GridSize;

/// Lines of scrollback kept by default. The web terminal keeps 10 MB.
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// Collects the events the parser raises while bytes are fed.
#[derive(Clone, Default)]
pub struct EventCollector(Rc<RefCell<Vec<Event>>>);

impl EventListener for EventCollector {
    fn send_event(&self, event: Event) {
        self.0.borrow_mut().push(event);
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// Which viewport rows changed since the last paint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Damage {
    Full,
    Rows(Vec<usize>),
}

/// The grid model behind one terminal surface.
pub struct TerminalCore {
    term: Term<EventCollector>,
    parser: Processor<StdSyncHandler>,
    events: EventCollector,
    size: GridSize,
    scrollback: usize,
    search: Option<RegexSearch>,
    /// Everything that changed since the last `take_damage`, beyond what the grid tracks.
    force_full_damage: bool,
}

impl TerminalCore {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let events = EventCollector::default();
        Self {
            term: Term::new(Self::config(scrollback), &size, events.clone()),
            parser: Processor::new(),
            events,
            size,
            scrollback,
            search: None,
            force_full_damage: true,
        }
    }

    fn config(scrollback: usize) -> Config {
        Config {
            scrolling_history: scrollback,
            kitty_keyboard: true,
            default_cursor_style: CursorStyle { shape: CursorShape::Block, blinking: true },
            ..Config::default()
        }
    }

    /// Parses program output.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Clears everything, scrollback included, then parses `bytes`: an attach snapshot.
    pub fn reset_with(&mut self, bytes: &[u8]) {
        self.term = Term::new(Self::config(self.scrollback), &self.size, self.events.clone());
        self.parser = Processor::new();
        self.force_full_damage = true;
        self.feed(bytes);
    }

    /// When an unterminated synchronized update (mode 2026) must be flushed.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update whose deadline passed, so the last frame does not freeze.
    pub fn flush_expired_sync(&mut self) {
        if self.sync_deadline().is_some_and(|deadline| deadline <= Instant::now()) {
            self.parser.stop_sync(&mut self.term);
        }
    }

    /// Events raised by the bytes fed since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut *self.events.0.borrow_mut())
    }

    pub fn size(&self) -> GridSize {
        self.size
    }

    /// Reflows the grid. Returns false when the size did not change.
    pub fn resize(&mut self, size: GridSize) -> bool {
        if size == self.size {
            return false;
        }
        self.size = size;
        self.term.resize(size);
        self.force_full_damage = true;
        true
    }

    /// Answers a program's text-area size query (CSI 14/18 t).
    pub fn window_size(&self, cell_width: f32, cell_height: f32) -> WindowSize {
        WindowSize {
            num_lines: self.size.rows,
            num_cols: self.size.cols,
            cell_width: cell_width.round() as u16,
            cell_height: cell_height.round() as u16,
        }
    }

    pub fn term(&self) -> &Term<EventCollector> {
        &self.term
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn cursor_style(&self) -> CursorStyle {
        self.term.cursor_style()
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.term.is_focused = focused;
    }

    /// Rows changed since the last call, then resets tracking.
    pub fn take_damage(&mut self) -> Damage {
        let damage = match self.term.damage() {
            TermDamage::Full => Damage::Full,
            TermDamage::Partial(lines) => Damage::Rows(lines.map(|line| line.line).collect()),
        };
        self.term.reset_damage();
        if std::mem::take(&mut self.force_full_damage) { Damage::Full } else { damage }
    }

    /// Forces the next paint to rebuild every row (theme, selection or hover changed).
    pub fn mark_dirty(&mut self) {
        self.force_full_damage = true;
    }

    // Scrolling.

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn is_alternate_screen(&self) -> bool {
        self.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Scrolls the viewport through history; positive moves towards older lines.
    pub fn scroll_lines(&mut self, lines: i32) {
        if lines != 0 {
            self.term.scroll_display(Scroll::Delta(lines));
        }
    }

    pub fn scroll_page(&mut self, up: bool) {
        self.term.scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    pub fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    // Coordinates.

    /// The grid point under a viewport cell.
    pub fn viewport_point(&self, row: usize, col: usize) -> Point {
        let line = row.min(self.size.rows as usize - 1) as i32 - self.display_offset() as i32;
        Point::new(Line(line), Column(col.min(self.size.cols as usize - 1)))
    }

    /// The viewport row a grid line is shown on, if visible.
    pub fn viewport_row(&self, line: Line) -> Option<usize> {
        let row = line.0 + self.display_offset() as i32;
        (row >= 0 && row < self.size.rows as i32).then_some(row as usize)
    }

    // Selection.

    pub fn start_selection(&mut self, point: Point, side: Side, kind: SelectionType) {
        self.term.selection = Some(Selection::new(kind, point, side));
        self.mark_dirty();
    }

    pub fn update_selection(&mut self, point: Point, side: Side) {
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
            self.mark_dirty();
        }
    }

    pub fn clear_selection(&mut self) {
        if self.term.selection.take().is_some() {
            self.mark_dirty();
        }
    }

    pub fn select_all(&mut self) {
        let mut selection =
            Selection::new(SelectionType::Lines, Point::new(self.term.topmost_line(), Column(0)), Side::Left);
        selection.update(Point::new(self.term.bottommost_line(), Column(0)), Side::Right);
        self.term.selection = Some(selection);
        self.mark_dirty();
    }

    pub fn has_selection(&self) -> bool {
        self.term.selection.as_ref().is_some_and(|selection| !selection.is_empty())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|text| !text.is_empty())
    }

    /// The selection's first and last grid line.
    pub fn selection_lines(&self) -> Option<RangeInclusive<Line>> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        Some(range.start.line..=range.end.line)
    }

    // Search.

    /// Sets the find query. An empty query ends the search.
    pub fn set_search(&mut self, query: &str) {
        self.search = if query.is_empty() {
            None
        } else {
            RegexSearch::new(&format!("(?i){}", escape_regex(query))).ok()
        };
        self.mark_dirty();
    }

    /// The next match from `origin` in `direction`, wrapping around the buffer.
    pub fn find(&mut self, origin: Point, forward: bool) -> Option<Match> {
        let regex = self.search.as_mut()?;
        let direction = if forward { Direction::Right } else { Direction::Left };
        let side = if forward { Side::Left } else { Side::Right };
        self.term.search_next(regex, origin, direction, side, None)
    }

    /// Every match on the visible rows, for highlighting.
    pub fn visible_matches(&mut self) -> Vec<Match> {
        let Some(regex) = self.search.as_mut() else {
            return Vec::new();
        };
        let offset = self.term.grid().display_offset() as i32;
        let start = Point::new(Line(-offset), Column(0));
        let end = Point::new(
            Line(self.size.rows as i32 - 1 - offset),
            Column(self.size.cols as usize - 1),
        );
        let start = self.term.line_search_left(start);
        let end = self.term.line_search_right(end);
        RegexIter::new(start, end, Direction::Right, &self.term, regex).collect()
    }

    /// Scrolls so a match is visible.
    pub fn reveal(&mut self, point: Point) {
        self.term.scroll_to_point(point);
        self.mark_dirty();
    }

    // Text.

    /// The logical line (following soft wraps) through `point`, with each character's grid point.
    pub fn logical_line(&self, point: Point) -> LogicalLine {
        let start = self.term.line_search_left(point);
        let end = self.term.line_search_right(point);
        let grid = self.term.grid();
        let mut line = LogicalLine::default();
        let mut cursor = start;
        loop {
            let cell = &grid[cursor];
            if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                line.points.push((line.text.len(), cursor));
                line.text.push(cell.c);
                for &extra in cell.zerowidth().unwrap_or_default() {
                    line.text.push(extra);
                }
            }
            if cursor == end {
                break;
            }
            cursor = cursor.add(&self.term, Boundary::Grid, 1);
        }
        // Trailing blanks are padding, not text.
        let trimmed = line.text.trim_end_matches(' ').len();
        line.text.truncate(trimmed);
        line.points.retain(|(offset, _)| *offset < trimmed);
        line
    }
}

/// A soft-wrapped line flattened to a string, for link detection.
#[derive(Default, Debug)]
pub struct LogicalLine {
    pub text: String,
    /// Byte offset in `text` of each cell's character, with that cell's grid point.
    pub points: Vec<(usize, Point)>,
}

impl LogicalLine {
    /// The grid point of the character at a byte offset.
    pub fn point_at(&self, offset: usize) -> Option<Point> {
        let index = self.points.partition_point(|(start, _)| *start <= offset);
        index.checked_sub(1).map(|index| self.points[index].1)
    }

    /// The byte offset of the character at a grid point.
    pub fn offset_of(&self, point: Point) -> Option<usize> {
        self.points.iter().find(|(_, p)| *p == point).map(|(offset, _)| *offset)
    }
}

fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
