use std::f32::consts::TAU;

use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui, Vec2};

use crate::theme::Palette;

/// Crisp vector icons drawn with `egui::Painter` (zero font-glyph dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailIcon {
    Collections,
    Environments,
    History,
    Cookies,
    Runner,
    Proxy,
    Import,
    Settings,
    Sun,
    Moon,
}

/// Renders an Activity Rail button with an 18x18 vector icon and a full 9.5px label below it.
pub fn rail_icon_button(
    ui: &mut Ui,
    icon: RailIcon,
    label: &str,
    active: bool,
    pal: &Palette,
) -> Response {
    let size = Vec2::new(72.0, 44.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());

    let hovered = resp.hovered();
    let bg = if active {
        pal.accent_tint
    } else if hovered {
        pal.bg_hover
    } else {
        Color32::TRANSPARENT
    };

    let fg = if active {
        if pal.is_dark {
            Color32::WHITE
        } else {
            pal.accent_primary
        }
    } else if hovered {
        pal.text_primary
    } else {
        pal.text_secondary
    };

    let painter = ui.painter();
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, Rounding::same(6.0), bg);
    }
    if active {
        painter.vline(
            rect.left() - 2.0,
            rect.top() + 7.0..=rect.bottom() - 7.0,
            Stroke::new(2.0_f32, pal.accent_indicator),
        );
    }

    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.top() + 15.0),
        Vec2::splat(17.0),
    );
    draw_rail_icon(painter, icon_rect, icon, fg);

    painter.text(
        Pos2::new(rect.center().x, rect.bottom() - 4.0),
        Align2::CENTER_BOTTOM,
        label,
        FontId::proportional(9.5),
        fg,
    );

    resp
}

pub fn draw_rail_icon(painter: &Painter, r: Rect, icon: RailIcon, color: Color32) {
    let s = Stroke::new(1.5_f32, color);
    let c = r.center();
    let w = r.width();
    let h = r.height();

    match icon {
        RailIcon::Collections => {
            // Folder outline with top-left tab
            let pts = vec![
                Pos2::new(r.left() + 1.0, r.top() + 3.0),
                Pos2::new(r.left() + w * 0.42, r.top() + 3.0),
                Pos2::new(r.left() + w * 0.54, r.top() + 5.5),
                Pos2::new(r.right() - 1.0, r.top() + 5.5),
                Pos2::new(r.right() - 1.0, r.bottom() - 2.0),
                Pos2::new(r.left() + 1.0, r.bottom() - 2.0),
            ];
            painter.add(Shape::closed_line(pts, s));
        }
        RailIcon::Environments => {
            // Three horizontal sliders with staggered knobs
            for (row, knob_t) in [(0.22_f32, 0.30_f32), (0.50, 0.72), (0.78, 0.42)] {
                let y = r.top() + h * row;
                let kx = r.left() + 2.0 + (w - 4.0) * knob_t;
                painter.line_segment([Pos2::new(r.left() + 1.0, y), Pos2::new(kx - 2.5, y)], s);
                painter.line_segment([Pos2::new(kx + 2.5, y), Pos2::new(r.right() - 1.0, y)], s);
                painter.circle_stroke(Pos2::new(kx, y), 2.2, s);
            }
        }
        RailIcon::History => {
            // Clock circle + hour/minute hands
            let radius = w.min(h) * 0.44;
            painter.circle_stroke(c, radius, s);
            painter.line_segment([c, Pos2::new(c.x, c.y - radius * 0.58)], s);
            painter.line_segment([c, Pos2::new(c.x + radius * 0.48, c.y + radius * 0.15)], s);
        }
        RailIcon::Cookies => {
            // Cookie circle with 4 chocolate-chip dots
            let radius = w.min(h) * 0.44;
            painter.circle_stroke(c, radius, s);
            for (dx, dy) in [(-0.32_f32, -0.22_f32), (0.28, -0.28), (-0.18, 0.30), (0.30, 0.20)] {
                painter.circle_filled(
                    Pos2::new(c.x + radius * dx, c.y + radius * dy),
                    1.3,
                    color,
                );
            }
        }
        RailIcon::Runner => {
            // Play triangle inside circle
            let radius = w.min(h) * 0.45;
            painter.circle_stroke(c, radius, s);
            let tri = vec![
                Pos2::new(c.x - radius * 0.28, c.y - radius * 0.42),
                Pos2::new(c.x + radius * 0.46, c.y),
                Pos2::new(c.x - radius * 0.28, c.y + radius * 0.42),
            ];
            painter.add(Shape::convex_polygon(tri, color, Stroke::NONE));
        }
        RailIcon::Proxy => {
            // Bidirectional horizontal arrows
            let y1 = c.y - h * 0.22;
            let y2 = c.y + h * 0.22;
            let x0 = r.left() + 2.0;
            let x1 = r.right() - 2.0;
            // Top arrow ->
            painter.line_segment([Pos2::new(x0, y1), Pos2::new(x1, y1)], s);
            painter.line_segment([Pos2::new(x1 - 3.5, y1 - 3.0), Pos2::new(x1, y1)], s);
            painter.line_segment([Pos2::new(x1 - 3.5, y1 + 3.0), Pos2::new(x1, y1)], s);
            // Bottom arrow <-
            painter.line_segment([Pos2::new(x0, y2), Pos2::new(x1, y2)], s);
            painter.line_segment([Pos2::new(x0 + 3.5, y2 - 3.0), Pos2::new(x0, y2)], s);
            painter.line_segment([Pos2::new(x0 + 3.5, y2 + 3.0), Pos2::new(x0, y2)], s);
        }
        RailIcon::Import => {
            // Downward arrow into tray
            let top = r.top() + 1.5;
            let mid_y = c.y + 1.5;
            painter.line_segment([Pos2::new(c.x, top), Pos2::new(c.x, mid_y)], s);
            painter.line_segment([Pos2::new(c.x - 3.5, mid_y - 3.5), Pos2::new(c.x, mid_y)], s);
            painter.line_segment([Pos2::new(c.x + 3.5, mid_y - 3.5), Pos2::new(c.x, mid_y)], s);
            let tray = vec![
                Pos2::new(r.left() + 2.0, c.y + 1.0),
                Pos2::new(r.left() + 2.0, r.bottom() - 2.0),
                Pos2::new(r.right() - 2.0, r.bottom() - 2.0),
                Pos2::new(r.right() - 2.0, c.y + 1.0),
            ];
            painter.add(Shape::line(tray, s));
        }
        RailIcon::Settings => {
            // Gear: hub circle + 6 radial teeth
            let inner = w.min(h) * 0.20;
            let outer = w.min(h) * 0.34;
            let tooth = w.min(h) * 0.46;
            painter.circle_stroke(c, inner, s);
            painter.circle_stroke(c, outer, s);
            for i in 0..6 {
                let a = (i as f32 / 6.0) * TAU;
                let (sin_a, cos_a) = a.sin_cos();
                painter.line_segment(
                    [
                        Pos2::new(c.x + cos_a * outer, c.y + sin_a * outer),
                        Pos2::new(c.x + cos_a * tooth, c.y + sin_a * tooth),
                    ],
                    Stroke::new(2.0_f32, color),
                );
            }
        }
        RailIcon::Sun => {
            // Sun circle + 8 rays
            let inner = w.min(h) * 0.22;
            let r0 = w.min(h) * 0.33;
            let r1 = w.min(h) * 0.47;
            painter.circle_stroke(c, inner, s);
            for i in 0..8 {
                let a = (i as f32 / 8.0) * TAU;
                let (sin_a, cos_a) = a.sin_cos();
                painter.line_segment(
                    [
                        Pos2::new(c.x + cos_a * r0, c.y + sin_a * r0),
                        Pos2::new(c.x + cos_a * r1, c.y + sin_a * r1),
                    ],
                    s,
                );
            }
        }
        RailIcon::Moon => {
            // Crescent moon arc
            let radius = w.min(h) * 0.42;
            let mut pts = Vec::new();
            for i in 0..=14 {
                let a = 0.65 + (i as f32 / 14.0) * 4.4;
                pts.push(Pos2::new(c.x + a.cos() * radius, c.y + a.sin() * radius));
            }
            for i in (0..=10).rev() {
                let a = 1.0 + (i as f32 / 10.0) * 3.7;
                let ox = c.x + radius * 0.36;
                let oy = c.y - radius * 0.16;
                pts.push(Pos2::new(ox + a.cos() * (radius * 0.78), oy + a.sin() * (radius * 0.78)));
            }
            painter.add(Shape::closed_line(pts, s));
        }
    }
}
