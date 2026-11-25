use apitester_core::models::{HttpMethod, ThemeMode};
use egui::text::LayoutJob;
use egui::{
    Color32, FontData, FontDefinitions, FontFamily, FontId, Frame, Margin, Pos2, Response,
    RichText, Rounding, Stroke, TextFormat, TextStyle, Ui, Vec2, Visuals,
};

/// High-contrast burnt-orange primary accent (`#CC4814`, WCAG AA >= 5.1:1 with pure white).
pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(204, 72, 20);
/// Vibrant burnt-orange indicator/hover accent (`#E05A2B`).
pub const ACCENT_INDICATOR: Color32 = Color32::from_rgb(224, 90, 43);

/// Semantic surface & text color palette for Dark (Zinc/Obsidian) and Light (Crisp Slate) modes.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub is_dark: bool,
    pub bg_canvas: Color32,
    pub bg_sidebar: Color32,
    pub bg_surface: Color32,
    pub bg_input: Color32,
    pub bg_hover: Color32,
    pub border_subtle: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub accent_primary: Color32,
    pub accent_indicator: Color32,
    pub accent_tint: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub info: Color32,
}

impl Palette {
    pub fn for_mode(is_dark: bool) -> Self {
        if is_dark {
            Self {
                is_dark: true,
                bg_canvas: Color32::from_rgb(17, 18, 22),     // #111216
                bg_sidebar: Color32::from_rgb(22, 24, 30),    // #16181E
                bg_surface: Color32::from_rgb(28, 31, 38),    // #1C1F26
                bg_input: Color32::from_rgb(12, 13, 16),      // #0C0D10
                bg_hover: Color32::from_rgb(37, 41, 51),      // #252933
                border_subtle: Color32::from_rgb(42, 47, 58), // #2A2F3A
                text_primary: Color32::from_rgb(236, 239, 244),
                text_secondary: Color32::from_rgb(160, 168, 184),
                text_muted: Color32::from_rgb(107, 114, 128),
                accent_primary: ACCENT_PRIMARY,
                accent_indicator: ACCENT_INDICATOR,
                accent_tint: Color32::from_rgba_unmultiplied(224, 90, 43, 32),
                success: Color32::from_rgb(52, 211, 153),
                warning: Color32::from_rgb(251, 191, 36),
                error: Color32::from_rgb(248, 113, 113),
                info: Color32::from_rgb(96, 165, 250),
            }
        } else {
            Self {
                is_dark: false,
                bg_canvas: Color32::from_rgb(255, 255, 255),  // #FFFFFF
                bg_sidebar: Color32::from_rgb(246, 247, 249), // #F6F7F9
                bg_surface: Color32::from_rgb(240, 242, 245), // #F0F2F5
                bg_input: Color32::from_rgb(255, 255, 255),   // #FFFFFF
                bg_hover: Color32::from_rgb(229, 233, 240),   // #E5E9F0
                border_subtle: Color32::from_rgb(221, 226, 234),
                text_primary: Color32::from_rgb(17, 24, 39),
                text_secondary: Color32::from_rgb(75, 85, 99),
                text_muted: Color32::from_rgb(107, 114, 128),
                accent_primary: ACCENT_PRIMARY,
                accent_indicator: ACCENT_PRIMARY,
                accent_tint: Color32::from_rgba_unmultiplied(204, 72, 20, 22),
                success: Color32::from_rgb(5, 150, 105),
                warning: Color32::from_rgb(180, 83, 9),
                error: Color32::from_rgb(220, 38, 38),
                info: Color32::from_rgb(37, 99, 235),
            }
        }
    }
}

/// Loads crisp system Sans (UI) and Monospace (code) fonts if installed, with graceful fallback.
pub fn load_system_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    let sans_candidates = [
        "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        "/usr/share/fonts/truetype/roboto/unhinted/RobotoTTF/Roboto-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    for path in sans_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system_sans".to_owned(), FontData::from_owned(bytes));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system_sans".to_owned());
            break;
        }
    }

    let mono_candidates = [
        "/usr/share/fonts/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/truetype/firacode/FiraCode-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ];
    for path in mono_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("system_mono".to_owned(), FontData::from_owned(bytes));
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "system_mono".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

/// Applies the Zinc/Obsidian Dark or Crisp Slate Light design system to `egui::Context`.
pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let is_dark = !matches!(mode, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    let mut style = (*ctx.style()).clone();

    // Typography scale
    style.text_styles = [
        (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
    ]
    .into();

    // Spacing & geometry
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size = Vec2::new(36.0, 28.0);
    style.spacing.window_margin = Margin::same(14.0);
    style.spacing.indent = 16.0;

    let mut visuals = if is_dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };

    visuals.panel_fill = pal.bg_canvas;
    visuals.window_fill = pal.bg_surface;
    visuals.extreme_bg_color = pal.bg_input;
    visuals.faint_bg_color = pal.bg_surface;
    visuals.override_text_color = Some(pal.text_primary);

    // Subtle, non-jittery widget states (zero heavy grey boxes!)
    let r6 = Rounding::same(6.0);
    visuals.widgets.noninteractive.bg_fill = pal.bg_surface;
    visuals.widgets.noninteractive.weak_bg_fill = pal.bg_surface;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, pal.border_subtle);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, pal.text_secondary);
    visuals.widgets.noninteractive.rounding = r6;

    visuals.widgets.inactive.bg_fill = pal.bg_surface;
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, pal.border_subtle);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, pal.text_primary);
    visuals.widgets.inactive.rounding = r6;

    visuals.widgets.hovered.bg_fill = pal.bg_hover;
    visuals.widgets.hovered.weak_bg_fill = pal.bg_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, pal.border_subtle);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, pal.text_primary);
    visuals.widgets.hovered.rounding = r6;
    visuals.widgets.hovered.expansion = 0.0;

    visuals.widgets.active.bg_fill = pal.bg_hover;
    visuals.widgets.active.weak_bg_fill = pal.bg_hover;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, pal.accent_indicator);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, pal.text_primary);
    visuals.widgets.active.rounding = r6;
    visuals.widgets.active.expansion = 0.0;

    visuals.widgets.open.bg_fill = pal.bg_surface;
    visuals.widgets.open.weak_bg_fill = pal.bg_hover;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, pal.accent_indicator);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, pal.text_primary);
    visuals.widgets.open.rounding = r6;

    // Subtle tinted selection (never solid orange pills everywhere!)
    visuals.selection.bg_fill = pal.accent_tint;
    visuals.selection.stroke = Stroke::new(1.0_f32, pal.text_primary);
    visuals.window_rounding = Rounding::same(8.0);
    visuals.menu_rounding = Rounding::same(8.0);

    style.visuals = visuals;
    ctx.set_style(style);
}

/// Color-coded HTTP method badge color tuned for Dark or Light backgrounds.
pub fn method_color_for_mode(method: &HttpMethod, is_dark: bool) -> Color32 {
    if is_dark {
        match method {
            HttpMethod::Get => Color32::from_rgb(52, 211, 153),
            HttpMethod::Post => Color32::from_rgb(251, 191, 36),
            HttpMethod::Put => Color32::from_rgb(96, 165, 250),
            HttpMethod::Patch => Color32::from_rgb(192, 132, 252),
            HttpMethod::Delete => Color32::from_rgb(248, 113, 113),
            HttpMethod::Head => Color32::from_rgb(45, 212, 191),
            HttpMethod::Options => Color32::from_rgb(156, 163, 175),
            HttpMethod::Custom(_) => Color32::from_rgb(244, 114, 182),
        }
    } else {
        match method {
            HttpMethod::Get => Color32::from_rgb(5, 150, 105),
            HttpMethod::Post => Color32::from_rgb(180, 83, 9),
            HttpMethod::Put => Color32::from_rgb(37, 99, 235),
            HttpMethod::Patch => Color32::from_rgb(124, 58, 237),
            HttpMethod::Delete => Color32::from_rgb(220, 38, 38),
            HttpMethod::Head => Color32::from_rgb(13, 148, 136),
            HttpMethod::Options => Color32::from_rgb(100, 116, 139),
            HttpMethod::Custom(_) => Color32::from_rgb(190, 24, 93),
        }
    }
}

/// Color-coded HTTP method badge color (dark-mode default convenience wrapper).
pub fn method_color(method: &HttpMethod) -> Color32 {
    method_color_for_mode(method, true)
}

/// Color-coded HTTP status badge color tuned for Dark or Light backgrounds.
pub fn status_color_for_mode(status: u16, is_dark: bool) -> Color32 {
    if is_dark {
        match status {
            200..=299 => Color32::from_rgb(52, 211, 153),
            300..=399 => Color32::from_rgb(96, 165, 250),
            400..=499 => Color32::from_rgb(251, 146, 60),
            500..=599 => Color32::from_rgb(248, 113, 113),
            _ => Color32::from_rgb(156, 163, 175),
        }
    } else {
        match status {
            200..=299 => Color32::from_rgb(5, 150, 105),
            300..=399 => Color32::from_rgb(37, 99, 235),
            400..=499 => Color32::from_rgb(234, 88, 12),
            500..=599 => Color32::from_rgb(220, 38, 38),
            _ => Color32::from_rgb(100, 116, 139),
        }
    }
}

/// Color-coded HTTP status badge color (dark-mode default convenience wrapper).
pub fn status_color(status: u16) -> Color32 {
    status_color_for_mode(status, true)
}

/// Modern underline-style tab (transparent fill + crisp 2px bottom accent bar when active).
pub fn underline_tab(ui: &mut Ui, selected: bool, label: &str, pal: &Palette) -> Response {
    let text_color = if selected {
        pal.text_primary
    } else {
        pal.text_secondary
    };
    let text = if selected {
        RichText::new(label).strong().color(text_color)
    } else {
        RichText::new(label).color(text_color)
    };

    let btn = egui::Button::new(text)
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .rounding(Rounding::same(4.0));
    let resp = ui.add(btn);

    if selected {
        let y = resp.rect.bottom() - 1.0;
        ui.painter().hline(
            resp.rect.left() + 4.0..=resp.rect.right() - 4.0,
            y,
            Stroke::new(2.0_f32, pal.accent_indicator),
        );
    }

    resp
}

/// High-contrast primary CTA button (`#CC4814` fill, bold white text).
pub fn primary_button(ui: &mut Ui, label: &str, min_size: Vec2) -> Response {
    ui.add(
        egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
            .fill(ACCENT_PRIMARY)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(6.0))
            .min_size(min_size),
    )
}

/// Subtle ghost button (transparent at rest, soft hover fill, no border clutter).
pub fn ghost_button(ui: &mut Ui, label: &str, pal: &Palette) -> Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(pal.text_secondary))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(6.0)),
    )
}

/// Tinted pill badge for HTTP methods (used in sidebar tree and history).
pub fn method_badge(ui: &mut Ui, method: &HttpMethod, is_dark: bool) {
    let color = method_color_for_mode(method, is_dark);
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if is_dark { 32 } else { 26 });
    let short = match method {
        HttpMethod::Delete => "DEL",
        HttpMethod::Options => "OPT",
        HttpMethod::Patch => "PAT",
        other => other.as_str(),
    };
    Frame::none()
        .fill(bg)
        .rounding(Rounding::same(4.0))
        .inner_margin(Margin::symmetric(5.0, 1.5))
        .show(ui, |ui| {
            ui.set_min_width(30.0);
            ui.label(RichText::new(short).small().strong().color(color));
        });
}

/// Tinted pill badge for HTTP status codes (e.g. `200 OK`).
pub fn status_badge(ui: &mut Ui, status: u16, status_text: &str, is_dark: bool) {
    let color = status_color_for_mode(status, is_dark);
    let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), if is_dark { 34 } else { 26 });
    let label = if status_text.is_empty() {
        format!("{status}")
    } else {
        format!("{status} {status_text}")
    };
    Frame::none()
        .fill(bg)
        .rounding(Rounding::same(4.0))
        .inner_margin(Margin::symmetric(8.0, 2.5))
        .show(ui, |ui| {
            ui.label(RichText::new(label).small().strong().color(color));
        });
}

/// Elevated surface card with subtle 1px border and clean inner padding.
pub fn section_card<R>(ui: &mut Ui, pal: &Palette, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::none()
        .fill(pal.bg_surface)
        .stroke(Stroke::new(1.0_f32, pal.border_subtle))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(12.0))
        .show(ui, add_contents)
        .inner
}

/// Paints a subtle 1px horizontal divider using `pal.border_subtle`.
pub fn subtle_divider(ui: &mut Ui, pal: &Palette) {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), 1.0)).1;
    ui.painter().line_segment(
        [Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)],
        Stroke::new(1.0_f32, pal.border_subtle),
    );
}

/// Formats JSON/XML/HTML/text with lightweight syntax highlighting for the response viewer.
pub fn highlight_body(text: &str, is_dark: bool, search: &str) -> LayoutJob {
    let trimmed = text.trim_start();
    let is_json = trimmed.starts_with('{') || trimmed.starts_with('[');
    let is_markup = trimmed.starts_with('<');

    let mut job = LayoutJob::default();
    let font = FontId::monospace(13.0);

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
        let highlight_line =
            !search_lower.is_empty() && line.to_ascii_lowercase().contains(&search_lower);
        let bg = if highlight_line {
            c_match_bg
        } else {
            Color32::TRANSPARENT
        };

        if is_json {
            highlight_json_line(
                &mut job, line, &font, bg, c_default, c_key, c_str, c_num, c_kw,
            );
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
        } else if b.is_ascii_digit()
            || (b == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-'))
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
