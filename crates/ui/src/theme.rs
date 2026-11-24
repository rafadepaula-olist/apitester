use apitester_core::models::{HttpMethod, ThemeMode};
use egui::text::LayoutJob;
use egui::{Color32, FontId, Rounding, Stroke, TextFormat, Visuals};

/// High-contrast burnt-orange primary accent (`#CC4814`, WCAG AA >= 5.1:1 with pure white).
pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(204, 72, 20);

/// Semantic accent palette shared across components.
pub struct Palette {
    pub bg_panel: Color32,
    pub bg_elevated: Color32,
    pub border: Color32,
    pub text_primary: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub info: Color32,
}

impl Palette {
    pub fn for_mode(is_dark: bool) -> Self {
        if is_dark {
            Self {
                bg_panel: Color32::from_rgb(20, 23, 29),
                bg_elevated: Color32::from_rgb(29, 34, 43),
                border: Color32::from_rgb(46, 53, 66),
                text_primary: Color32::from_rgb(232, 237, 245),
                text_muted: Color32::from_rgb(139, 148, 165),
                accent: Color32::from_rgb(255, 108, 55),
                success: Color32::from_rgb(52, 211, 153),
                warning: Color32::from_rgb(251, 191, 36),
                error: Color32::from_rgb(248, 113, 113),
                info: Color32::from_rgb(96, 165, 250),
            }
        } else {
            Self {
                bg_panel: Color32::from_rgb(248, 250, 252),
                bg_elevated: Color32::from_rgb(255, 255, 255),
                border: Color32::from_rgb(218, 224, 234),
                text_primary: Color32::from_rgb(20, 25, 35),
                text_muted: Color32::from_rgb(100, 112, 130),
                accent: Color32::from_rgb(234, 88, 35),
                success: Color32::from_rgb(16, 150, 96),
                warning: Color32::from_rgb(202, 138, 4),
                error: Color32::from_rgb(220, 38, 38),
                info: Color32::from_rgb(37, 99, 235),
            }
        }
    }
}

/// Applies the dark or light visual theme to `egui::Context`.
pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let is_dark = !matches!(mode, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    let mut visuals = if is_dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    visuals.panel_fill = pal.bg_panel;
    visuals.window_fill = pal.bg_elevated;
    visuals.extreme_bg_color = if is_dark {
        Color32::from_rgb(15, 18, 23)
    } else {
        Color32::from_rgb(241, 245, 249)
    };
    visuals.faint_bg_color = pal.bg_elevated;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border);
    visuals.widgets.inactive.rounding = Rounding::same(4.0);
    visuals.widgets.hovered.rounding = Rounding::same(4.0);
    visuals.widgets.active.rounding = Rounding::same(4.0);
    visuals.selection.bg_fill = ACCENT_PRIMARY;
    visuals.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);

    ctx.set_visuals(visuals);
}

/// Color-coded HTTP method badge color.
pub fn method_color(method: &HttpMethod) -> Color32 {
    match method {
        HttpMethod::Get => Color32::from_rgb(52, 211, 153),      // Green
        HttpMethod::Post => Color32::from_rgb(251, 191, 36),     // Amber
        HttpMethod::Put => Color32::from_rgb(96, 165, 250),      // Blue
        HttpMethod::Patch => Color32::from_rgb(192, 132, 252),   // Purple
        HttpMethod::Delete => Color32::from_rgb(248, 113, 113),  // Red
        HttpMethod::Head => Color32::from_rgb(45, 212, 191),     // Teal
        HttpMethod::Options => Color32::from_rgb(156, 163, 175), // Slate
        HttpMethod::Custom(_) => Color32::from_rgb(244, 114, 182),
    }
}

/// Color-coded HTTP status badge color.
pub fn status_color(status: u16) -> Color32 {
    match status {
        200..=299 => Color32::from_rgb(52, 211, 153),
        300..=399 => Color32::from_rgb(96, 165, 250),
        400..=499 => Color32::from_rgb(251, 146, 60),
        500..=599 => Color32::from_rgb(248, 113, 113),
        _ => Color32::from_rgb(156, 163, 175),
    }
}

/// Formats JSON/XML/HTML/text with lightweight syntax highlighting for the response viewer.
pub fn highlight_body(text: &str, is_dark: bool, search: &str) -> LayoutJob {
    let trimmed = text.trim_start();
    let is_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    let is_markup = trimmed.starts_with('<');

    let mut job = LayoutJob::default();
    let font = FontId::monospace(12.5);

    let (c_default, c_key, c_str, c_num, c_kw, c_tag, c_match_bg) = if is_dark {
        (
            Color32::from_rgb(226, 232, 240),
            Color32::from_rgb(125, 211, 252),
            Color32::from_rgb(134, 239, 172),
            Color32::from_rgb(253, 186, 116),
            Color32::from_rgb(196, 181, 253),
            Color32::from_rgb(249, 168, 212),
            Color32::from_rgb(133, 77, 14),
        )
    } else {
        (
            Color32::from_rgb(30, 41, 59),
            Color32::from_rgb(2, 132, 199),
            Color32::from_rgb(22, 163, 74),
            Color32::from_rgb(217, 119, 6),
            Color32::from_rgb(124, 58, 237),
            Color32::from_rgb(190, 24, 93),
            Color32::from_rgb(254, 240, 138),
        )
    };

    let search_lower = search.to_ascii_lowercase();

    for line in text.split_inclusive('\n') {
        let highlight_line = !search_lower.is_empty()
            && line.to_ascii_lowercase().contains(&search_lower);
        let bg = if highlight_line {
            c_match_bg
        } else {
            Color32::TRANSPARENT
        };

        if is_json {
            highlight_json_line(&mut job, line, &font, bg, c_default, c_key, c_str, c_num, c_kw);
        } else if is_markup {
            highlight_markup_line(&mut job, line, &font, bg, c_default, c_tag, c_str);
        } else {
            job.append(
                line,
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: c_default,
                    background: bg,
                    ..Default::default()
                },
            );
        }
    }

    job
}

#[allow(clippy::too_many_arguments)]
fn highlight_json_line(
    job: &mut LayoutJob,
    line: &str,
    font: &FontId,
    bg: Color32,
    c_default: Color32,
    c_key: Color32,
    c_str: Color32,
    c_num: Color32,
    c_kw: Color32,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let token = &line[start..i];
            let is_key = line[i..].trim_start().starts_with(':');
            append_tok(job, token, font, if is_key { c_key } else { c_str }, bg);
        } else if b.is_ascii_digit() || (b == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit()) {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit() || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                i += 1;
            }
            append_tok(job, &line[start..i], font, c_num, bg);
        } else if line[i..].starts_with("true") {
            append_tok(job, "true", font, c_kw, bg);
            i += 4;
        } else if line[i..].starts_with("false") {
            append_tok(job, "false", font, c_kw, bg);
            i += 5;
        } else if line[i..].starts_with("null") {
            append_tok(job, "null", font, c_kw, bg);
            i += 4;
        } else {
            let start = i;
            i += line[i..].chars().next().map_or(1, char::len_utf8);
            append_tok(job, &line[start..i], font, c_default, bg);
        }
    }
}

fn highlight_markup_line(
    job: &mut LayoutJob,
    line: &str,
    font: &FontId,
    bg: Color32,
    c_default: Color32,
    c_tag: Color32,
    c_str: Color32,
) {
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        if open > 0 {
            append_tok(job, &rest[..open], font, c_default, bg);
        }
        let after = &rest[open..];
        if let Some(close) = after.find('>') {
            let tag = &after[..=close];
            if tag.contains('"') {
                append_tok(job, tag, font, c_str, bg);
            } else {
                append_tok(job, tag, font, c_tag, bg);
            }
            rest = &after[close + 1..];
        } else {
            append_tok(job, after, font, c_tag, bg);
            return;
        }
    }
    if !rest.is_empty() {
        append_tok(job, rest, font, c_default, bg);
    }
}

fn append_tok(job: &mut LayoutJob, text: &str, font: &FontId, color: Color32, background: Color32) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color,
            background,
            ..Default::default()
        },
    );
}
