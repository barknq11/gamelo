//! Line icons from the design (24-unit grid, 2-unit strokes), painted at any size.

use crate::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Glyph {
    Home,
    Library,
    Stats,
    Search,
    Couch,
    Settings,
    Heart,
    More,
    Grid,
    List,
}

/// Draw `g` filling `rect` (square) in `color`.
pub(crate) fn glyph(p: &egui::Painter, g: Glyph, rect: egui::Rect, color: egui::Color32) {
    let s = rect.width() / 24.0;
    let at = |x: f32, y: f32| rect.min + egui::vec2(x * s, y * s);
    let stroke = egui::Stroke::new((2.0 * s).max(1.4), color);
    let line = |pts: &[(f32, f32)]| {
        p.add(egui::Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let rounded = |x: f32, y: f32, w: f32, h: f32| {
        p.rect_stroke(egui::Rect::from_min_size(at(x, y), egui::vec2(w * s, h * s)), egui::CornerRadius::same((1.5 * s) as u8), stroke, egui::StrokeKind::Middle);
    };
    match g {
        Glyph::Home => line(&[(3.0, 11.0), (12.0, 4.0), (21.0, 11.0), (21.0, 20.0), (20.0, 21.0), (15.0, 21.0), (15.0, 15.0), (9.0, 15.0), (9.0, 21.0), (4.0, 21.0), (3.0, 20.0), (3.0, 11.0)]),
        Glyph::Library => {
            rounded(3.0, 3.0, 7.0, 9.0);
            rounded(14.0, 3.0, 7.0, 9.0);
            rounded(3.0, 15.0, 7.0, 6.0);
            rounded(14.0, 15.0, 7.0, 6.0);
        }
        Glyph::Grid => {
            rounded(4.0, 4.0, 6.0, 6.0);
            rounded(14.0, 4.0, 6.0, 6.0);
            rounded(4.0, 14.0, 6.0, 6.0);
            rounded(14.0, 14.0, 6.0, 6.0);
        }
        Glyph::Stats => {
            line(&[(4.0, 20.0), (4.0, 10.0)]);
            line(&[(10.0, 20.0), (10.0, 4.0)]);
            line(&[(16.0, 20.0), (16.0, 13.0)]);
            line(&[(22.0, 20.0), (2.0, 20.0)]);
        }
        Glyph::Search => {
            p.circle_stroke(at(11.0, 11.0), 7.0 * s, stroke);
            line(&[(20.0, 20.0), (16.5, 16.5)]);
        }
        Glyph::Couch => {
            // A controller: body outline, d-pad and two buttons.
            line(&[(6.7, 5.0), (17.3, 5.0), (19.8, 6.0), (21.3, 8.6), (22.0, 15.0), (20.6, 17.6), (18.0, 18.2), (16.6, 17.0), (15.0, 15.0), (9.0, 15.0), (7.4, 17.0), (6.0, 18.2), (3.4, 17.6), (2.0, 15.0), (2.7, 8.6), (4.2, 6.0), (6.7, 5.0)]);
            line(&[(6.0, 11.0), (10.0, 11.0)]);
            line(&[(8.0, 9.0), (8.0, 13.0)]);
            p.circle_filled(at(15.0, 12.0), 1.1 * s, color);
            p.circle_filled(at(18.0, 10.0), 1.1 * s, color);
        }
        Glyph::Settings => {
            // A cog: eight teeth around a ring, with a hole.
            let c = at(12.0, 12.0);
            let pts: Vec<egui::Pos2> = (0..=32)
                .map(|k| {
                    let a = k as f32 / 32.0 * std::f32::consts::TAU;
                    let tooth = (k / 2) % 2 == 0;
                    let r = if tooth { 9.5 } else { 7.3 } * s;
                    c + r * egui::vec2(a.cos(), a.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts, stroke));
            p.circle_stroke(c, 3.0 * s, stroke);
        }
        Glyph::Heart => {
            let c1 = at(8.0, 9.5);
            let c2 = at(16.0, 9.5);
            p.circle_filled(c1, 4.6 * s, color);
            p.circle_filled(c2, 4.6 * s, color);
            p.add(egui::Shape::convex_polygon(vec![at(3.6, 11.2), at(20.4, 11.2), at(12.0, 20.5)], color, egui::Stroke::NONE));
            p.rect_filled(egui::Rect::from_min_max(at(8.0, 7.0), at(16.0, 12.0)), 0, color);
        }
        Glyph::More => {
            for x in [5.0, 12.0, 19.0] {
                p.circle_filled(at(x, 12.0), 2.0 * s, color);
            }
        }
        Glyph::List => {
            for y in [6.0, 12.0, 18.0] {
                line(&[(8.0, y), (21.0, y)]);
                p.circle_filled(at(3.5, y), 1.2 * s, color);
            }
        }
    }
}
