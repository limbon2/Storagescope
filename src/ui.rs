use std::cmp;

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use crate::model::FsEntryKind;
use crate::theme::{ThemePalette, current_theme};

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
    pub metric: String,
    pub sort_mode: String,
    pub scan_status: String,
    pub filter: String,
    pub filter_mode: bool,
    pub rows: Vec<RowModel>,
    pub selected_index: usize,
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
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(5),
    ])
    .split(frame.area());

    let header = Paragraph::new(vec![
        Line::styled(format!("Path: {}", model.current_root), theme.text_style()),
        Line::styled(
            format!(
                "Metric: {} | Sort: {} | Status: {}",
                model.metric, model.sort_mode, model.scan_status
            ),
            theme.muted_style(),
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

    let rows = model.rows.iter().enumerate().map(|(idx, row)| {
        let selected = idx == model.selected_index;
        let style = if selected {
            theme.selected_style()
        } else if row.is_loading {
            theme.loading_style()
        } else {
            theme.text_style()
        };

        let bar = make_bar(row.size_bytes, max_size, 18);
        let name = if row.is_loading {
            format!("{} [loading]", row.name)
        } else {
            row.name.clone()
        };
        let row_cells = vec![
            Cell::from(name),
            Cell::from(row.kind.to_string()),
            Cell::from(format_bytes(row.size_bytes)),
            Cell::from(bar),
            Cell::from(row.path_display.clone()),
        ];

        Row::new(row_cells).style(style)
    });

    let widths = [
        Constraint::Length(28),
        Constraint::Length(8),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Min(10),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["Name", "Kind", "Size", "Relative", "Path"]).style(theme.header_style()),
        )
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
        "Legend: ?/F1 help | q quit | j/k move | Enter open | h/back up | / filter",
        theme.accent_style(),
    ));
    let mut quick_actions = String::from("Actions: s sort | m metric | r rescan");
    if model.delete_enabled {
        quick_actions.push_str(" | d delete");
    }
    lines.push(Line::styled(quick_actions, theme.accent_style()));

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
  m: toggle size metric (allocated/apparent)\n\
  /: filter by name/path\n\
  Esc: clear filter or close dialog\n\n\
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

fn make_bar(value: u64, max: u64, width: usize) -> String {
    if max == 0 || width == 0 {
        return String::new();
    }
    let ratio = value as f64 / max as f64;
    let filled = cmp::max(1, (ratio * width as f64).round() as usize).min(width);
    let mut out = String::with_capacity(width);
    out.push_str(&"#".repeat(filled));
    out.push_str(&".".repeat(width - filled));
    out
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
    use super::format_bytes;

    #[test]
    fn formats_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(12), "12 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
    }
}
