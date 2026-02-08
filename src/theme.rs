use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct ThemePalette {
    source: String,
    use_color: bool,
    reverse_selection: bool,
    foreground: Color,
    accent: Color,
    muted: Color,
    warning: Color,
    danger: Color,
    loading: Color,
    selection_fg: Color,
    selection_bg: Color,
}

impl ThemePalette {
    pub fn detect() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Self::monochrome("no-color");
        }

        if let Some(theme) = Self::from_omarchy_current() {
            return theme;
        }

        Self::from_terminal_fallback()
    }

    fn monochrome(source: &str) -> Self {
        Self {
            source: source.to_string(),
            use_color: false,
            reverse_selection: true,
            foreground: Color::Reset,
            accent: Color::Reset,
            muted: Color::Reset,
            warning: Color::Reset,
            danger: Color::Reset,
            loading: Color::Reset,
            selection_fg: Color::Reset,
            selection_bg: Color::Reset,
        }
    }

    fn from_omarchy_current() -> Option<Self> {
        let path = omarchy_colors_path()?;
        Self::from_omarchy_path(&path)
    }

    fn from_omarchy_path(path: &Path) -> Option<Self> {
        let content = fs::read_to_string(&path).ok()?;
        let map = parse_color_assignments(&content);

        let foreground = map.get("foreground").copied().unwrap_or(Color::Reset);
        let accent = map
            .get("accent")
            .or_else(|| map.get("color4"))
            .copied()
            .unwrap_or(Color::Cyan);
        let muted = map
            .get("color8")
            .or_else(|| map.get("color7"))
            .copied()
            .unwrap_or(Color::DarkGray);
        let warning = map.get("color3").copied().unwrap_or(Color::Yellow);
        let danger = map.get("color1").copied().unwrap_or(Color::Red);
        let loading = map
            .get("color6")
            .or_else(|| map.get("color14"))
            .copied()
            .unwrap_or(accent);
        let selection_fg = map
            .get("selection_foreground")
            .copied()
            .unwrap_or(foreground);
        let selection_bg = map.get("selection_background").copied().unwrap_or(accent);

        Some(Self {
            source: "omarchy-current".to_string(),
            use_color: true,
            reverse_selection: false,
            foreground,
            accent,
            muted,
            warning,
            danger,
            loading,
            selection_fg,
            selection_bg,
        })
    }

    fn from_terminal_fallback() -> Self {
        let light = detect_light_background().unwrap_or(false);

        if light {
            Self {
                source: "terminal-fallback-light".to_string(),
                use_color: true,
                reverse_selection: true,
                foreground: Color::Reset,
                accent: Color::Blue,
                muted: Color::DarkGray,
                warning: Color::Red,
                danger: Color::Red,
                loading: Color::Blue,
                selection_fg: Color::Reset,
                selection_bg: Color::Reset,
            }
        } else {
            Self {
                source: "terminal-fallback-dark".to_string(),
                use_color: true,
                reverse_selection: true,
                foreground: Color::Reset,
                accent: Color::Cyan,
                muted: Color::DarkGray,
                warning: Color::Yellow,
                danger: Color::Red,
                loading: Color::Green,
                selection_fg: Color::Reset,
                selection_bg: Color::Reset,
            }
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn text_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.foreground)
        } else {
            Style::default()
        }
    }

    pub fn muted_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.muted)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    pub fn accent_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.accent)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn header_style(&self) -> Style {
        self.accent_style().add_modifier(Modifier::BOLD)
    }

    pub fn border_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.muted)
        } else {
            Style::default()
        }
    }

    pub fn panel_title_style(&self) -> Style {
        self.accent_style().add_modifier(Modifier::BOLD)
    }

    pub fn warning_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.warning)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn danger_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.danger)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn loading_style(&self) -> Style {
        if self.use_color {
            Style::default().fg(self.loading)
        } else {
            Style::default().add_modifier(Modifier::ITALIC)
        }
    }

    pub fn selected_style(&self) -> Style {
        if self.reverse_selection {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            let selection_fg = adaptive_selected_foreground(self.selection_fg, self.selection_bg);
            Style::default()
                .fg(selection_fg)
                .bg(self.selection_bg)
                .add_modifier(Modifier::BOLD)
        }
    }
}

#[derive(Debug)]
struct ThemeCache {
    palette: ThemePalette,
    source_path: Option<PathBuf>,
    source_mtime: Option<SystemTime>,
    last_check: Instant,
}

impl ThemeCache {
    fn new() -> Self {
        let (palette, source_path, source_mtime) = detect_theme_with_source();
        Self {
            palette,
            source_path,
            source_mtime,
            last_check: Instant::now(),
        }
    }

    fn current(&mut self) -> ThemePalette {
        if self.last_check.elapsed() >= Duration::from_millis(250) {
            self.last_check = Instant::now();

            if std::env::var_os("NO_COLOR").is_some() {
                self.palette = ThemePalette::monochrome("no-color");
                self.source_path = None;
                self.source_mtime = None;
            } else if let Some(path) = omarchy_colors_path() {
                let mtime = modified_time(&path);
                let path_changed = self.source_path.as_ref() != Some(&path);
                let mtime_changed = self.source_mtime != mtime;

                if path_changed || mtime_changed || self.palette.source() != "omarchy-current" {
                    if let Some(palette) = ThemePalette::from_omarchy_path(&path) {
                        self.palette = palette;
                        self.source_path = Some(path);
                        self.source_mtime = mtime;
                    } else {
                        self.palette = ThemePalette::from_terminal_fallback();
                        self.source_path = None;
                        self.source_mtime = None;
                    }
                }
            } else if self.palette.source() == "omarchy-current" {
                self.palette = ThemePalette::from_terminal_fallback();
                self.source_path = None;
                self.source_mtime = None;
            }
        }

        self.palette.clone()
    }
}

pub fn current_theme() -> ThemePalette {
    static THEME: OnceLock<Mutex<ThemeCache>> = OnceLock::new();
    let cache = THEME.get_or_init(|| Mutex::new(ThemeCache::new()));
    match cache.lock() {
        Ok(mut guard) => guard.current(),
        Err(_) => ThemePalette::detect(),
    }
}

fn detect_theme_with_source() -> (ThemePalette, Option<PathBuf>, Option<SystemTime>) {
    if std::env::var_os("NO_COLOR").is_some() {
        return (ThemePalette::monochrome("no-color"), None, None);
    }

    if let Some(path) = omarchy_colors_path() {
        let mtime = modified_time(&path);
        if let Some(theme) = ThemePalette::from_omarchy_path(&path) {
            return (theme, Some(path), mtime);
        }
    }

    (ThemePalette::from_terminal_fallback(), None, None)
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn omarchy_colors_path() -> Option<PathBuf> {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(config_home).join("omarchy/current/theme/colors.toml");
        if path.exists() {
            return Some(path);
        }
    }

    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/omarchy/current/theme/colors.toml");
    if path.exists() { Some(path) } else { None }
}

fn parse_color_assignments(content: &str) -> HashMap<String, Color> {
    let mut map = HashMap::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key_raw, value_raw)) = line.split_once('=') else {
            continue;
        };

        let key = key_raw.trim();
        let value = value_raw.trim().trim_matches('"');
        if let Some(color) = parse_hex_color(value) {
            map.insert(key.to_string(), color);
        }
    }

    map
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    Some(Color::Rgb(r, g, b))
}

fn detect_light_background() -> Option<bool> {
    let value = std::env::var("COLORFGBG").ok()?;
    let last = value
        .split(';')
        .filter_map(|part| part.parse::<u8>().ok())
        .next_back()?;

    Some(matches!(last, 7 | 15))
}

fn adaptive_selected_foreground(preferred_fg: Color, background: Color) -> Color {
    let Some(bg_rgb) = color_to_rgb(background) else {
        return preferred_fg;
    };

    let preferred_contrast = color_to_rgb(preferred_fg)
        .map(|fg_rgb| contrast_ratio(fg_rgb, bg_rgb))
        .unwrap_or(0.0);

    // Use the theme-provided foreground when it is already readable.
    if preferred_contrast >= 4.5 {
        return preferred_fg;
    }

    let black = Color::Rgb(0, 0, 0);
    let white = Color::Rgb(255, 255, 255);
    let black_contrast = contrast_ratio((0, 0, 0), bg_rgb);
    let white_contrast = contrast_ratio((255, 255, 255), bg_rgb);

    if black_contrast >= white_contrast {
        black
    } else {
        white
    }
}

fn color_to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Black => Some((0, 0, 0)),
        Color::DarkGray => Some((85, 85, 85)),
        Color::Gray => Some((170, 170, 170)),
        Color::White => Some((255, 255, 255)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::Indexed(index) => Some(indexed_ansi_to_rgb(index)),
        Color::Reset => None,
    }
}

fn indexed_ansi_to_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        return match index {
            0 => (0, 0, 0),
            1 => (128, 0, 0),
            2 => (0, 128, 0),
            3 => (128, 128, 0),
            4 => (0, 0, 128),
            5 => (128, 0, 128),
            6 => (0, 128, 128),
            7 => (192, 192, 192),
            8 => (128, 128, 128),
            9 => (255, 0, 0),
            10 => (0, 255, 0),
            11 => (255, 255, 0),
            12 => (0, 0, 255),
            13 => (255, 0, 255),
            14 => (0, 255, 255),
            _ => (255, 255, 255),
        };
    }

    if index <= 231 {
        let i = index - 16;
        let r = i / 36;
        let g = (i % 36) / 6;
        let b = i % 6;
        let to_value = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return (to_value(r), to_value(g), to_value(b));
    }

    let gray = 8 + (index - 232) * 10;
    (gray, gray, gray)
}

fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let l1 = relative_luminance(a);
    let l2 = relative_luminance(b);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

fn relative_luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let channel = |value: u8| {
        let c = value as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };

    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

#[cfg(test)]
mod tests {
    use super::{adaptive_selected_foreground, parse_color_assignments, parse_hex_color};
    use ratatui::style::Color;

    #[test]
    fn parses_hex_color() {
        assert_eq!(
            parse_hex_color("#112233"),
            Some(Color::Rgb(0x11, 0x22, 0x33))
        );
        assert_eq!(
            parse_hex_color("112233"),
            Some(Color::Rgb(0x11, 0x22, 0x33))
        );
        assert_eq!(parse_hex_color("#123"), None);
    }

    #[test]
    fn parses_toml_assignments() {
        let parsed = parse_color_assignments(
            "accent = \"#7aa2f7\"\ncolor1 = \"#f7768e\"\nignored = \"bad\"\n",
        );
        assert_eq!(parsed.get("accent"), Some(&Color::Rgb(0x7a, 0xa2, 0xf7)));
        assert_eq!(parsed.get("color1"), Some(&Color::Rgb(0xf7, 0x76, 0x8e)));
        assert!(parsed.get("ignored").is_none());
    }

    #[test]
    fn adaptive_selection_uses_dark_text_on_light_bg() {
        let fg = adaptive_selected_foreground(Color::Rgb(255, 255, 255), Color::Rgb(170, 200, 255));
        assert_eq!(fg, Color::Rgb(0, 0, 0));
    }

    #[test]
    fn adaptive_selection_uses_light_text_on_dark_bg() {
        let fg = adaptive_selected_foreground(Color::Rgb(0, 0, 0), Color::Rgb(20, 30, 80));
        assert_eq!(fg, Color::Rgb(255, 255, 255));
    }
}
