use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::cli::Config;
use crate::delete::delete_target;
use crate::errors::AppError;
use crate::model::{FsEntryKind, NodeSummary, ScanEvent, ScanProgress, SizeMetric, SortMode};
use crate::platform::disk_usage;
use crate::scanner::{ScanSession, start_scan};
use crate::ui::{DialogStateView, FOOTER_HEIGHT, HEADER_HEIGHT, RowModel, ViewModel, format_bytes};

const MAX_EVENTS_PER_TICK: usize = 8192;
const SPINNER_FRAMES: [char; 4] = ['|', '/', '-', '\\'];
const MOUSE_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
const MOUSE_SCROLL_STEP: isize = 1;
const MOUSE_SCROLL_MIN_INTERVAL: Duration = Duration::from_millis(35);
const POLL_INTERVAL_WHILE_SCANNING: Duration = Duration::from_millis(24);
const POLL_INTERVAL_IDLE: Duration = Duration::from_millis(80);
const DISK_INFO_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
enum ScanState {
    Idle,
    Scanning(ScanProgress),
    Complete(ScanProgress),
    Error(String),
    Cancelled,
}

impl ScanState {
    fn as_status(&self) -> String {
        match self {
            Self::Idle => "idle".to_string(),
            Self::Scanning(progress) => format!(
                "scanning (entries: {}, warnings: {})",
                progress.visited_entries, progress.warnings
            ),
            Self::Complete(progress) => format!(
                "complete (entries: {}, warnings: {})",
                progress.visited_entries, progress.warnings
            ),
            Self::Error(message) => format!("error: {message}"),
            Self::Cancelled => "cancelled".to_string(),
        }
    }

    fn is_scanning(&self) -> bool {
        matches!(self, Self::Scanning(_))
    }

    fn progress(&self) -> Option<&ScanProgress> {
        match self {
            Self::Scanning(progress) | Self::Complete(progress) => Some(progress),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum DeleteDialog {
    None,
    Confirm { target: PathBuf },
    TypePhrase { target: PathBuf, typed: String },
}

pub struct App {
    config: Config,
    startup_root: PathBuf,
    current_root: PathBuf,
    active_scan_root: Option<PathBuf>,
    nodes: HashMap<PathBuf, NodeSummary>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    selected_index: usize,
    table_scroll_offset: usize,
    sort_mode: SortMode,
    metric: SizeMetric,
    filter: String,
    filter_mode: bool,
    warnings: Vec<String>,
    message: Option<String>,
    scan_state: ScanState,
    scanner: Option<ScanSession>,
    quit: bool,
    delete_dialog: DeleteDialog,
    help_modal_open: bool,
    spinner_tick: usize,
    last_frame_area: Rect,
    last_mouse_click: Option<(usize, Instant)>,
    last_mouse_scroll: Option<Instant>,
    disk_line_cache: Option<String>,
    disk_line_cache_root: Option<PathBuf>,
    disk_line_last_update: Option<Instant>,
}

impl App {
    pub fn new(config: Config) -> Self {
        Self {
            startup_root: config.startup_root.clone(),
            current_root: config.startup_root.clone(),
            active_scan_root: None,
            metric: config.initial_metric,
            sort_mode: SortMode::SizeDesc,
            config,
            nodes: HashMap::new(),
            children: HashMap::new(),
            selected_index: 0,
            table_scroll_offset: 0,
            filter: String::new(),
            filter_mode: false,
            warnings: Vec::new(),
            message: None,
            scan_state: ScanState::Idle,
            scanner: None,
            quit: false,
            delete_dialog: DeleteDialog::None,
            help_modal_open: false,
            spinner_tick: 0,
            last_frame_area: Rect::default(),
            last_mouse_click: None,
            last_mouse_scroll: None,
            disk_line_cache: None,
            disk_line_cache_root: None,
            disk_line_last_update: None,
        }
    }

    pub fn run(&mut self) -> Result<(), AppError> {
        self.start_scan_at(self.startup_root.clone());

        enable_raw_mode().map_err(|error| AppError::Terminal(error.to_string()))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .map_err(|error| AppError::Terminal(error.to_string()))?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|error| AppError::Terminal(error.to_string()))?;

        let run_result = self.event_loop(&mut terminal);

        let _ = disable_raw_mode();
        let _ = execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = terminal.show_cursor();

        run_result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<(), AppError> {
        while !self.quit {
            self.drain_scan_events();
            self.spinner_tick = self.spinner_tick.wrapping_add(1);
            let size = terminal
                .size()
                .map_err(|error| AppError::Terminal(error.to_string()))?;
            self.last_frame_area = Rect::new(0, 0, size.width, size.height);

            let model = self.build_view_model();
            terminal
                .draw(|frame| crate::ui::render(frame, &model))
                .map_err(|error| AppError::Terminal(error.to_string()))?;

            let poll_interval = if self.scan_state.is_scanning() {
                POLL_INTERVAL_WHILE_SCANNING
            } else {
                POLL_INTERVAL_IDLE
            };

            if event::poll(poll_interval).map_err(|error| AppError::Terminal(error.to_string()))? {
                match event::read().map_err(|error| AppError::Terminal(error.to_string()))? {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Press {
                            self.handle_key(key)?;
                        }
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse(mouse)?;
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<(), AppError> {
        if self.help_modal_open {
            if matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::Down(MouseButton::Right)
                    | MouseEventKind::Down(MouseButton::Middle)
            ) {
                self.help_modal_open = false;
            }
            return Ok(());
        }

        if !matches!(self.delete_dialog, DeleteDialog::None) {
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.can_process_scroll() {
                    self.move_selection(MOUSE_SCROLL_STEP);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.can_process_scroll() {
                    self.move_selection(-MOUSE_SCROLL_STEP);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = self.mouse_row_to_index(mouse.column, mouse.row) {
                    let now = Instant::now();
                    let is_double_click = self
                        .last_mouse_click
                        .map(|(last_index, last_time)| {
                            last_index == index
                                && now.duration_since(last_time) <= MOUSE_DOUBLE_CLICK_WINDOW
                        })
                        .unwrap_or(false);

                    self.selected_index = index;
                    self.ensure_selection_in_bounds();

                    if is_double_click {
                        self.drill_into_selection();
                        self.last_mouse_click = None;
                    } else {
                        self.last_mouse_click = Some((index, now));
                    }
                } else {
                    self.last_mouse_click = None;
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.point_in_table(mouse.column, mouse.row) {
                    self.navigate_to_parent();
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn can_process_scroll(&mut self) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_mouse_scroll
            && now.duration_since(last) < MOUSE_SCROLL_MIN_INTERVAL
        {
            return false;
        }

        self.last_mouse_scroll = Some(now);
        true
    }

    fn point_in_table(&self, column: u16, row: u16) -> bool {
        let table = self.table_area();

        column >= table.x
            && column < table.x.saturating_add(table.width)
            && row >= table.y
            && row < table.y.saturating_add(table.height)
    }

    fn mouse_row_to_index(&self, column: u16, row: u16) -> Option<usize> {
        let table = self.table_area();

        if table.width <= 2 || table.height <= 3 {
            return None;
        }

        let inner_left = table.x + 1;
        let inner_right = table.x + table.width - 1;
        let data_top = table.y + 2;
        let data_bottom = table.y + table.height - 1;

        if column < inner_left || column >= inner_right || row < data_top || row >= data_bottom {
            return None;
        }

        let relative_index = (row - data_top) as usize;
        let index = self.table_scroll_offset.saturating_add(relative_index);
        let len = self.visible_node_paths().len();
        (index < len).then_some(index)
    }

    fn table_area(&self) -> Rect {
        let chunks = Layout::vertical([
            Constraint::Length(HEADER_HEIGHT),
            Constraint::Min(6),
            Constraint::Length(FOOTER_HEIGHT),
        ])
        .split(self.last_frame_area);
        chunks[1]
    }

    fn table_visible_rows(&self) -> usize {
        self.table_area().height.saturating_sub(3) as usize
    }

    fn start_scan_at(&mut self, root: PathBuf) {
        if let Some(mut scan) = self.scanner.take() {
            scan.stop();
        }

        self.prune_subtree(&root);
        self.warnings.clear();
        self.selected_index = 0;
        self.table_scroll_offset = 0;
        self.scan_state = ScanState::Scanning(ScanProgress::default());
        self.active_scan_root = Some(root.clone());

        let mut options = self.config.scan_options.clone();
        options.root = root;

        self.scanner = Some(start_scan(options));
    }

    fn prune_subtree(&mut self, root: &Path) {
        let to_remove: Vec<PathBuf> = self
            .nodes
            .keys()
            .filter(|path| Self::in_subtree(path, root))
            .cloned()
            .collect();

        for path in &to_remove {
            self.nodes.remove(path);
            self.children.remove(path);
        }

        for child_paths in self.children.values_mut() {
            child_paths.retain(|path| !Self::in_subtree(path, root));
        }
    }

    fn in_subtree(path: &Path, root: &Path) -> bool {
        path == root || path.starts_with(root)
    }

    fn drain_scan_events(&mut self) {
        let mut should_stop_scanner = false;
        let mut processed = 0_usize;

        while processed < MAX_EVENTS_PER_TICK {
            let event = if let Some(scanner) = &self.scanner {
                match scanner.receiver().try_recv() {
                    Ok(event) => event,
                    Err(_) => break,
                }
            } else {
                break;
            };

            processed += 1;

            match event {
                ScanEvent::Reset { root } => {
                    self.active_scan_root = Some(root);
                    self.scan_state = ScanState::Scanning(ScanProgress::default());
                }
                ScanEvent::NodeUpdated(node) => self.upsert_node(node),
                ScanEvent::Progress(progress) => {
                    self.scan_state = ScanState::Scanning(progress);
                }
                ScanEvent::Warning { path, message } => {
                    self.warnings.push(format!("{}: {message}", path.display()));
                }
                ScanEvent::Complete(progress) => {
                    self.scan_state = ScanState::Complete(progress);
                    self.active_scan_root = None;
                    should_stop_scanner = true;
                    break;
                }
                ScanEvent::Error(message) => {
                    self.scan_state = ScanState::Error(message);
                    self.active_scan_root = None;
                    should_stop_scanner = true;
                    break;
                }
                ScanEvent::Cancelled => {
                    self.scan_state = ScanState::Cancelled;
                    self.active_scan_root = None;
                    should_stop_scanner = true;
                    break;
                }
            }
        }

        if should_stop_scanner {
            if let Some(mut scanner) = self.scanner.take() {
                scanner.stop();
            }
        }

        self.ensure_selection_in_bounds();
    }

    fn upsert_node(&mut self, node: NodeSummary) {
        let path = node.path.clone();
        let parent = path.parent().map(Path::to_path_buf);
        let is_new = self.nodes.insert(path.clone(), node).is_none();

        if is_new {
            if let Some(parent_path) = parent {
                self.children.entry(parent_path).or_default().push(path);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<(), AppError> {
        if self.help_modal_open {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::F(1) | KeyCode::Char('q') => {
                    self.help_modal_open = false;
                }
                _ => {}
            }
            return Ok(());
        }

        if self.handle_delete_dialog_key(&key)? {
            return Ok(());
        }

        if self.filter_mode {
            self.handle_filter_key(key);
            return Ok(());
        }

        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Enter => self.drill_into_selection(),
            KeyCode::Backspace | KeyCode::Char('h') => self.navigate_to_parent(),
            KeyCode::Char('s') => {
                self.sort_mode = self.sort_mode.cycle();
                self.ensure_selection_in_bounds();
            }
            KeyCode::Char('m') => self.metric = self.metric.toggle(),
            KeyCode::Char('r') => self.start_scan_at(self.current_root.clone()),
            KeyCode::Char('/') => self.filter_mode = true,
            KeyCode::Char('?') | KeyCode::F(1) => self.help_modal_open = true,
            KeyCode::Esc => {
                self.filter.clear();
                self.message = None;
                self.ensure_selection_in_bounds();
            }
            KeyCode::Char('d') => {
                if !self.config.no_delete {
                    if let Some(node) = self.selected_node() {
                        self.delete_dialog = DeleteDialog::Confirm {
                            target: node.path.clone(),
                        };
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter_mode = false;
                self.filter.clear();
            }
            KeyCode::Enter => self.filter_mode = false,
            KeyCode::Backspace => {
                self.filter.pop();
                self.ensure_selection_in_bounds();
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.filter.push(ch);
                    self.ensure_selection_in_bounds();
                }
            }
            _ => {}
        }
    }

    fn handle_delete_dialog_key(&mut self, key: &KeyEvent) -> Result<bool, AppError> {
        match &mut self.delete_dialog {
            DeleteDialog::None => Ok(false),
            DeleteDialog::Confirm { target } => {
                match key.code {
                    KeyCode::Esc => self.delete_dialog = DeleteDialog::None,
                    KeyCode::Enter => {
                        self.delete_dialog = DeleteDialog::TypePhrase {
                            target: target.clone(),
                            typed: String::new(),
                        }
                    }
                    _ => {}
                }
                Ok(true)
            }
            DeleteDialog::TypePhrase { target, typed } => {
                match key.code {
                    KeyCode::Esc => self.delete_dialog = DeleteDialog::None,
                    KeyCode::Backspace => {
                        typed.pop();
                    }
                    KeyCode::Char(ch) => {
                        if !ch.is_control() {
                            typed.push(ch);
                        }
                    }
                    KeyCode::Enter => {
                        if typed == "DELETE" {
                            match delete_target(target, &self.current_root) {
                                Ok(()) => {
                                    self.message =
                                        Some(format!("Deleted {}", target.to_string_lossy()));
                                    self.delete_dialog = DeleteDialog::None;
                                    self.start_scan_at(self.current_root.clone());
                                }
                                Err(error) => {
                                    self.message = Some(error.to_string());
                                    self.delete_dialog = DeleteDialog::None;
                                }
                            }
                        } else {
                            self.message = Some("Type DELETE exactly to confirm".to_string());
                        }
                    }
                    _ => {}
                }
                Ok(true)
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible_node_paths().len();
        if len == 0 {
            self.selected_index = 0;
            self.table_scroll_offset = 0;
            return;
        }

        let current = self.selected_index as isize;
        let max = (len - 1) as isize;
        let next = (current + delta).clamp(0, max);
        self.selected_index = next as usize;
        self.sync_table_scroll(len);
    }

    fn drill_into_selection(&mut self) {
        if let Some(node) = self.selected_node() {
            if matches!(node.kind, FsEntryKind::Dir | FsEntryKind::Symlink) {
                self.current_root = node.path.clone();
                self.selected_index = 0;
                self.ensure_selection_in_bounds();
            }
        }
    }

    fn navigate_to_parent(&mut self) {
        if self.current_root == self.startup_root {
            return;
        }

        if let Some(parent) = self.current_root.parent() {
            self.current_root = parent.to_path_buf();
            self.selected_index = 0;
            self.ensure_selection_in_bounds();
        }
    }

    fn selected_node(&self) -> Option<NodeSummary> {
        let paths = self.visible_node_paths();
        let path = paths.get(self.selected_index)?;
        self.nodes.get(path).cloned()
    }

    fn visible_node_paths(&self) -> Vec<PathBuf> {
        let mut paths = self
            .children
            .get(&self.current_root)
            .cloned()
            .unwrap_or_default();

        let filter = self.filter.to_lowercase();
        if !filter.is_empty() {
            paths.retain(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let full_path = path.to_string_lossy().to_lowercase();
                name.contains(&filter) || full_path.contains(&filter)
            });
        }

        paths.retain(|path| self.nodes.contains_key(path));

        match self.sort_mode {
            SortMode::SizeDesc => paths.sort_by(|a, b| {
                let left = self
                    .nodes
                    .get(a)
                    .map(|node| node.metric_bytes(self.metric))
                    .unwrap_or_default();
                let right = self
                    .nodes
                    .get(b)
                    .map(|node| node.metric_bytes(self.metric))
                    .unwrap_or_default();
                right.cmp(&left).then_with(|| a.cmp(b))
            }),
            SortMode::SizeAsc => paths.sort_by(|a, b| {
                let left = self
                    .nodes
                    .get(a)
                    .map(|node| node.metric_bytes(self.metric))
                    .unwrap_or_default();
                let right = self
                    .nodes
                    .get(b)
                    .map(|node| node.metric_bytes(self.metric))
                    .unwrap_or_default();
                left.cmp(&right).then_with(|| a.cmp(b))
            }),
            SortMode::Name => paths.sort_by(|a, b| {
                let a_name = a
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let b_name = b
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                a_name.cmp(&b_name)
            }),
        }

        paths
    }

    fn ensure_selection_in_bounds(&mut self) {
        let len = self.visible_node_paths().len();
        if len == 0 {
            self.selected_index = 0;
            self.table_scroll_offset = 0;
        } else if self.selected_index >= len {
            self.selected_index = len - 1;
            self.sync_table_scroll(len);
        } else {
            self.sync_table_scroll(len);
        }
    }

    fn sync_table_scroll(&mut self, len: usize) {
        self.table_scroll_offset = compute_scroll_offset(
            self.table_scroll_offset,
            self.selected_index,
            len,
            self.table_visible_rows(),
        );
    }

    fn build_view_model(&mut self) -> ViewModel {
        let rows: Vec<RowModel> = self
            .visible_node_paths()
            .into_iter()
            .filter_map(|path| self.nodes.get(&path))
            .map(|node| RowModel {
                name: node
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| node.path.to_string_lossy().into_owned()),
                kind: node.kind,
                size_bytes: node.metric_bytes(self.metric),
                path_display: node.path.to_string_lossy().into_owned(),
                is_loading: !node.is_complete,
            })
            .collect();
        let show_loading_hint = rows.is_empty() && self.scan_state.is_scanning();

        let dialog = match &self.delete_dialog {
            DeleteDialog::None => DialogStateView::None,
            DeleteDialog::Confirm { target } => DialogStateView::Confirm {
                target: target.to_string_lossy().into_owned(),
            },
            DeleteDialog::TypePhrase { target, typed } => DialogStateView::TypePhrase {
                target: target.to_string_lossy().into_owned(),
                typed: typed.clone(),
            },
        };

        ViewModel {
            current_root: self.current_root.to_string_lossy().into_owned(),
            disk_line: self.current_disk_line(),
            metric: self.metric.as_str().to_string(),
            sort_mode: self.sort_mode.as_str().to_string(),
            scan_status: self.scan_state.as_status(),
            filter: self.filter.clone(),
            filter_mode: self.filter_mode,
            rows,
            selected_index: self.selected_index,
            table_scroll_offset: self.table_scroll_offset,
            warning_line: self.warnings.last().cloned(),
            message_line: self.message.clone(),
            delete_enabled: !self.config.no_delete,
            dialog,
            loading_hint: if show_loading_hint {
                let spinner = SPINNER_FRAMES[self.spinner_tick % SPINNER_FRAMES.len()];
                let progress = self.scan_state.progress().cloned().unwrap_or_default();
                Some(format!(
                    "{spinner} scanning... visited {} entries, warnings {}",
                    progress.visited_entries, progress.warnings
                ))
            } else {
                None
            },
            live_loading_line: if self.scan_state.is_scanning() {
                let spinner = SPINNER_FRAMES[self.spinner_tick % SPINNER_FRAMES.len()];
                let root = self
                    .active_scan_root
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.current_root.to_string_lossy().into_owned());
                Some(format!(
                    "{spinner} live scan in progress for {root}; results update as they are discovered"
                ))
            } else {
                None
            },
            help_modal_open: self.help_modal_open,
        }
    }

    fn current_disk_line(&mut self) -> String {
        let now = Instant::now();
        let path_changed = self.disk_line_cache_root.as_ref() != Some(&self.current_root);
        let stale = self
            .disk_line_last_update
            .map(|last| now.duration_since(last) >= DISK_INFO_REFRESH_INTERVAL)
            .unwrap_or(true);

        if path_changed || stale || self.disk_line_cache.is_none() {
            self.disk_line_cache = Some(build_disk_line(&self.current_root));
            self.disk_line_cache_root = Some(self.current_root.clone());
            self.disk_line_last_update = Some(now);
        }

        self.disk_line_cache
            .clone()
            .unwrap_or_else(|| "Disk: unavailable".to_string())
    }
}

fn build_disk_line(path: &Path) -> String {
    let Some(usage) = disk_usage(path) else {
        return "Disk: unavailable".to_string();
    };

    let total = usage.total_bytes;
    let free = usage.available_bytes.min(total);
    let used = total.saturating_sub(free);
    let used_pct = if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64) * 100.0
    };

    format!(
        "Disk: {} total | {} used ({used_pct:.1}%) | {} free",
        format_bytes(total),
        format_bytes(used),
        format_bytes(free),
    )
}

fn compute_scroll_offset(
    current_offset: usize,
    selected_index: usize,
    len: usize,
    visible_rows: usize,
) -> usize {
    if len == 0 {
        return 0;
    }

    if visible_rows == 0 {
        return selected_index.min(len - 1);
    }

    let max_offset = len.saturating_sub(visible_rows);
    let mut offset = current_offset.min(max_offset);

    if selected_index < offset {
        offset = selected_index;
    } else if selected_index >= offset.saturating_add(visible_rows) {
        offset = selected_index
            .saturating_add(1)
            .saturating_sub(visible_rows);
    }

    offset.min(max_offset)
}

#[cfg(test)]
mod tests {
    use super::compute_scroll_offset;

    #[test]
    fn keeps_selection_visible_when_moving_down() {
        assert_eq!(compute_scroll_offset(0, 0, 100, 5), 0);
        assert_eq!(compute_scroll_offset(0, 4, 100, 5), 0);
        assert_eq!(compute_scroll_offset(0, 5, 100, 5), 1);
        assert_eq!(compute_scroll_offset(1, 6, 100, 5), 2);
    }

    #[test]
    fn clamps_offset_for_small_lists() {
        assert_eq!(compute_scroll_offset(9, 1, 3, 10), 0);
        assert_eq!(compute_scroll_offset(4, 2, 3, 2), 1);
    }

    #[test]
    fn handles_zero_visible_rows() {
        assert_eq!(compute_scroll_offset(0, 7, 10, 0), 7);
        assert_eq!(compute_scroll_offset(3, 2, 0, 0), 0);
    }
}
