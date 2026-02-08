use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::model::FsEntryKind;
use crate::theme::{ThemePalette, current_theme};

pub const HEADER_HEIGHT: u16 = 5;
pub const FOOTER_HEIGHT: u16 = 6;

#[derive(Debug, Clone)]
pub struct RowModel {
    pub name: String,
    pub kind: FsEntryKind,
    pub size_bytes: u64,
    pub path_display: String,
    pub is_loading: bool,
}

#[derive(Debug, Clone)]
pub enum DialogStateView {
    None,
    Confirm { target: String },
    TypePhrase { target: String, typed: String },
}

#[derive(Debug, Clone)]
pub struct ViewModel {
    pub current_root: String,
    pub disk_line: String,
    pub metric: String,
    pub sort_mode: String,
    pub scan_status: String,
    pub filter: String,
    pub filter_mode: bool,
    pub rows: Vec<RowModel>,
    pub selected_index: usize,
    pub table_scroll_offset: usize,
    pub show_name_column: bool,
    pub show_kind_column: bool,
    pub show_size_column: bool,
    pub show_relative_column: bool,
    pub show_path_column: bool,
    pub warning_line: Option<String>,
    pub message_line: Option<String>,
    pub delete_enabled: bool,
    pub dialog: DialogStateView,
    pub loading_hint: Option<String>,
    pub live_loading_line: Option<String>,
    pub help_modal_open: bool,
}

pub fn render(frame: &mut ratatui::Frame<'_>, model: &ViewModel) {
    let theme = current_theme();

    let chunks = Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Min(6),
        Constraint::Length(FOOTER_HEIGHT),
    ])
    .split(frame.area());
    let header_subtle_style = theme.text_style().add_modifier(Modifier::DIM);

    let header = Paragraph::new(vec![
        Line::styled(format!("Path: {}", model.current_root), theme.text_style()),
        Line::styled(model.disk_line.clone(), header_subtle_style),
        Line::styled(
            format!(
                "Metric: {} | Sort: {} | Status: {}",
                model.metric, model.sort_mode, model.scan_status
            ),
            header_subtle_style,
        ),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme.border_style())
            .title("StorageScope")
            .title_style(theme.panel_title_style()),
    );
    frame.render_widget(header, chunks[0]);

    render_table(frame, chunks[1], model, &theme);
    render_footer(frame, chunks[2], model, &theme);

    if !matches!(model.dialog, DialogStateView::None) {
        render_delete_dialog(frame, model, &theme);
    }

    if model.help_modal_open {
        render_help_dialog(frame, model, &theme);
    }
}

fn render_table(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    model: &ViewModel,
    theme: &ThemePalette,
) {
    let visible_column_count = [
        model.show_name_column,
        model.show_kind_column,
        model.show_size_column,
        model.show_relative_column,
        model.show_path_column,
    ]
    .into_iter()
    .filter(|enabled| *enabled)
    .count();

    if visible_column_count == 0 {
        let empty = Paragraph::new(Line::styled(
            "All columns are hidden. Use Shift+N/K/S/R/P to show columns.",
            theme.warning_style(),
        ))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title("Children (drill down with Enter)")
                .title_style(theme.panel_title_style()),
        )
        .wrap(Wrap { trim: true });

        frame.render_widget(empty, area);
        return;
    }

    if model.rows.is_empty() {
        let (text, style) = if let Some(loading_hint) = &model.loading_hint {
            (loading_hint.clone(), theme.loading_style())
        } else {
            (
                "No entries to display for this path/filter.".to_string(),
                theme.muted_style(),
            )
        };

        let empty = Paragraph::new(Line::styled(text, style))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme.border_style())
                    .title("Children (drill down with Enter)")
                    .title_style(theme.panel_title_style()),
            )
            .wrap(Wrap { trim: true });

        frame.render_widget(empty, area);
        return;
    }

    let max_size = model
        .rows
        .iter()
        .map(|row| row.size_bytes)
        .max()
        .unwrap_or(1);

    let visible_rows = area.height.saturating_sub(3) as usize;
    let start = model
        .table_scroll_offset
        .min(model.rows.len().saturating_sub(1));
    let rows = model
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(visible_rows)
        .map(|(idx, row)| {
            let selected = idx == model.selected_index;
            let style = if selected {
                theme.selected_style()
            } else if row.is_loading {
                theme.loading_style()
            } else {
                theme.text_style()
            };

            let name = if row.is_loading {
                format!("{} [loading]", row.name)
            } else {
                row.name.clone()
            };
            let mut row_cells = Vec::with_capacity(visible_column_count);
            if model.show_name_column {
                row_cells.push(Cell::from(name));
            }
            if model.show_kind_column {
                row_cells.push(Cell::from(row.kind.to_string()));
            }
            if model.show_size_column {
                row_cells.push(Cell::from(format_bytes(row.size_bytes)));
            }
            if model.show_relative_column {
                let bar = make_bar_line(row.size_bytes, max_size, 18, theme, selected);
                row_cells.push(Cell::from(bar));
            }
            if model.show_path_column {
                row_cells.push(Cell::from(row.path_display.clone()));
            }

            Row::new(row_cells).style(style)
        });

    let mut widths = Vec::with_capacity(visible_column_count);
    let mut header_cells = Vec::with_capacity(visible_column_count);
    if model.show_name_column {
        widths.push(Constraint::Length(28));
        header_cells.push(Cell::from(hotkey_label_line("Name", "N", theme)));
    }
    if model.show_kind_column {
        widths.push(Constraint::Length(8));
        header_cells.push(Cell::from(hotkey_label_line("Kind", "K", theme)));
    }
    if model.show_size_column {
        widths.push(Constraint::Length(12));
        header_cells.push(Cell::from(hotkey_label_line("Size", "S", theme)));
    }
    if model.show_relative_column {
        widths.push(Constraint::Length(20));
        header_cells.push(Cell::from(hotkey_label_line("Relative", "R", theme)));
    }
    if model.show_path_column {
        widths.push(Constraint::Min(10));
        header_cells.push(Cell::from(hotkey_label_line("Path", "P", theme)));
    }

    let table = Table::new(rows, widths)
        .header(Row::new(header_cells).style(theme.header_style()))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title("Children (drill down with Enter)")
                .title_style(theme.panel_title_style()),
        );

    frame.render_widget(table, area);
}

fn render_footer(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    model: &ViewModel,
    theme: &ThemePalette,
) {
    let mut lines = Vec::new();

    if model.filter_mode {
        lines.push(Line::styled(
            format!("Filter (/): {}_", model.filter),
            theme.accent_style(),
        ));
    } else {
        lines.push(Line::styled(
            format!("Filter: {}", model.filter),
            theme.muted_style(),
        ));
    }

    lines.push(Line::styled(
        "Legend: ?/F1 help | q quit | j/k move | Enter open | h/back up | / filter | wheel scroll | click select",
        theme.accent_style(),
    ));
    let mut quick_actions = String::from("Actions: s sort | m metric | r rescan");
    if model.delete_enabled {
        quick_actions.push_str(" | d delete");
    }
    lines.push(Line::styled(quick_actions, theme.accent_style()));
    lines.push(build_column_toggle_line(model, theme));

    lines.push(Line::styled(
        format!("Theme source: {}", theme.source()),
        theme.muted_style(),
    ));

    if let Some(message) = &model.message_line {
        lines.push(Line::styled(message.clone(), theme.text_style()));
    } else if let Some(warning) = &model.warning_line {
        lines.push(Line::styled(
            format!("Warning: {warning}"),
            theme.warning_style(),
        ));
    }

    if let Some(live_line) = &model.live_loading_line {
        lines.push(Line::styled(live_line.clone(), theme.loading_style()));
    }

    let footer = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.border_style())
                .title("Status")
                .title_style(theme.panel_title_style()),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(footer, area);
}

fn render_delete_dialog(frame: &mut ratatui::Frame<'_>, model: &ViewModel, theme: &ThemePalette) {
    let area = centered_rect(70, 40, frame.area());
    frame.render_widget(Clear, area);

    let text = match &model.dialog {
        DialogStateView::None => String::new(),
        DialogStateView::Confirm { target } => format!(
            "Delete target?\n\n{}\n\nPress Enter to continue or Esc to cancel.",
            target
        ),
        DialogStateView::TypePhrase { target, typed } => format!(
            "Type DELETE to confirm removal:\n\n{}\n\nInput: {}",
            target, typed
        ),
    };

    let dialog = Paragraph::new(text)
        .block(
            Block::default()
                .title("Delete Confirmation")
                .borders(Borders::ALL)
                .border_style(theme.danger_style())
                .title_style(theme.danger_style())
                .style(theme.danger_style()),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(dialog, area);
}

fn render_help_dialog(frame: &mut ratatui::Frame<'_>, model: &ViewModel, theme: &ThemePalette) {
    let area = centered_rect(80, 80, frame.area());
    frame.render_widget(Clear, area);

    let delete_line = if model.delete_enabled {
        "d: delete selected item (requires typing DELETE)"
    } else {
        "d: delete is disabled in this session (--no-delete)"
    };

    let text = format!(
        "StorageScope Help\n\n\
Navigation:\n\
  j / k or Up / Down: move selection\n\
  Enter: open selected directory\n\
  h or Backspace: go to parent directory\n\n\
Scan and View:\n\
  r: rescan current path\n\
  s: cycle sort mode\n\
  Shift+N: toggle Name column\n\
  Shift+K: toggle Kind column\n\
  Shift+S: toggle Size column\n\
  Shift+R: toggle Relative column\n\
  Shift+P: toggle Path column\n\
  m: toggle size metric (allocated/apparent)\n\
  /: filter by name/path\n\
  Esc: clear filter or close dialog\n\n\
Mouse:\n\
  Wheel: scroll selection\n\
  Left click: select row\n\
  Double left click: open selected directory\n\
  Right click in table: go to parent directory\n\n\
Safety:\n\
  {delete_line}\n\n\
Loading Indicators:\n\
  [loading] on a row means directory size is still being calculated\n\
  Footer spinner means scan is still in progress and rows may update\n\n\
Help:\n\
  ? or F1: open/close this help\n\
  q: quit app (or close help when this modal is open)"
    );

    let dialog = Paragraph::new(text)
        .block(
            Block::default()
                .title("Help")
                .borders(Borders::ALL)
                .border_style(theme.accent_style())
                .title_style(theme.accent_style())
                .style(theme.accent_style()),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(dialog, area);
}

fn build_column_toggle_line(model: &ViewModel, theme: &ThemePalette) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("Columns (Shift+): ", theme.accent_style()));
    append_column_toggle(
        &mut spans,
        "N",
        "Name",
        model.show_name_column,
        false,
        theme,
    );
    append_column_toggle(&mut spans, "K", "Kind", model.show_kind_column, true, theme);
    append_column_toggle(&mut spans, "S", "Size", model.show_size_column, true, theme);
    append_column_toggle(
        &mut spans,
        "R",
        "Relative",
        model.show_relative_column,
        true,
        theme,
    );
    append_column_toggle(&mut spans, "P", "Path", model.show_path_column, true, theme);
    Line::from(spans)
}

fn append_column_toggle(
    spans: &mut Vec<Span<'static>>,
    key: &str,
    label: &str,
    enabled: bool,
    prepend_separator: bool,
    theme: &ThemePalette,
) {
    if prepend_separator {
        spans.push(Span::styled(" | ", theme.accent_style()));
    }

    let label_style = if enabled {
        theme.accent_style()
    } else {
        theme.muted_style()
    };
    let state = if enabled { "on" } else { "off" };
    let key_style = hotkey_key_style(theme);
    let mut chars = label.chars();
    let first = chars.next();
    let rest: String = chars.collect();

    // btop-like cue: highlight the hotkey letter inside the label itself.
    if let Some(first_char) = first {
        if first_char.eq_ignore_ascii_case(&key.chars().next().unwrap_or(first_char)) {
            spans.push(Span::styled(first_char.to_string(), key_style));
            spans.push(Span::styled(format!("{rest}[{state}]"), label_style));
            return;
        }
    }

    spans.push(Span::styled(key.to_string(), key_style));
    spans.push(Span::styled(format!(" {label}[{state}]"), label_style));
}

fn hotkey_key_style(theme: &ThemePalette) -> Style {
    // Color can clash across themes; add modifiers so this stays visible.
    theme
        .warning_style()
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

fn hotkey_label_line(label: &str, key: &str, theme: &ThemePalette) -> Line<'static> {
    let mut spans = Vec::new();
    let mut chars = label.chars();
    let first = chars.next();
    let rest: String = chars.collect();
    let key_style = hotkey_key_style(theme);
    let label_style = theme.header_style();

    if let Some(first_char) = first {
        if first_char.eq_ignore_ascii_case(&key.chars().next().unwrap_or(first_char)) {
            spans.push(Span::styled(first_char.to_string(), key_style));
            spans.push(Span::styled(rest, label_style));
            return Line::from(spans);
        }
    }

    spans.push(Span::styled(label.to_string(), label_style));
    Line::from(spans)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn make_bar_line(
    value: u64,
    max: u64,
    width: usize,
    theme: &ThemePalette,
    selected: bool,
) -> Line<'static> {
    if width == 0 {
        return Line::default();
    }

    let total_units = width * 8;
    let mut filled_units = if max == 0 || value == 0 {
        0
    } else {
        ((value as f64 / max as f64) * total_units as f64).round() as usize
    };
    if value > 0 && filled_units == 0 {
        filled_units = 1;
    }
    if filled_units > total_units {
        filled_units = total_units;
    }

    let full_blocks = filled_units / 8;
    let partial_block = filled_units % 8;
    let position_ratio = |idx: usize| -> f64 {
        if width <= 1 {
            0.0
        } else {
            idx as f64 / (width - 1) as f64
        }
    };
    const PARTIALS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

    let mut spans = Vec::with_capacity(width);
    if selected && !theme.uses_reverse_selection() {
        let selection_bg = theme.selected_background_color().unwrap_or_default();
        let track_style = Style::default()
            .fg(theme.bar_track_color())
            .bg(selection_bg);

        for idx in 0..width {
            if idx < full_blocks {
                let fill_style = Style::default()
                    .fg(theme.bar_fill_color(position_ratio(idx)))
                    .bg(selection_bg);
                spans.push(Span::styled("█", fill_style));
            } else if idx == full_blocks && partial_block > 0 && full_blocks < width {
                let fill = theme.bar_fill_color(position_ratio(idx));
                let partial_style = Style::default().fg(fill).bg(selection_bg);
                spans.push(Span::styled(
                    PARTIALS[partial_block - 1].to_string(),
                    partial_style,
                ));
            } else {
                spans.push(Span::styled("·", track_style));
            }
        }

        return Line::from(spans);
    }

    let track_style = Style::default().bg(theme.bar_track_color());
    for idx in 0..width {
        if idx < full_blocks {
            let fill_style = Style::default().bg(theme.bar_fill_color(position_ratio(idx)));
            spans.push(Span::styled(" ", fill_style));
        } else if idx == full_blocks && partial_block > 0 && full_blocks < width {
            let fill = theme.bar_fill_color(position_ratio(idx));
            let partial_style = Style::default().fg(fill).bg(theme.bar_track_color());
            spans.push(Span::styled(
                PARTIALS[partial_block - 1].to_string(),
                partial_style,
            ));
        } else {
            spans.push(Span::styled(" ", track_style));
        }
    }

    Line::from(spans)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{format_bytes, make_bar_line};
    use crate::theme::current_theme;

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
    }

    #[test]
    fn renders_pretty_bar() {
        let theme = current_theme();
        let mid = make_bar_line(50, 100, 10, &theme, false);
        assert_eq!(mid.width(), 10);

        let empty = make_bar_line(0, 100, 8, &theme, false);
        assert_eq!(empty.width(), 8);

        let full = make_bar_line(100, 100, 6, &theme, false);
        assert_eq!(full.width(), 6);
    }
}
