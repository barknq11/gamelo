//! Right-to-left text inside text boxes.
//!
//! egui edits text strictly left to right: it lays a field out run by run in
//! typing order, and its cursor code expects glyph `i` to be character `i`,
//! with x growing along the text. Arabic typed into a field came out with its
//! words in the wrong order.
//!
//! Fields whose text contains right-to-left script get a layouter that draws
//! the text in reading order (bidi algorithm, letters joined by `shaping`)
//! and then hands each glyph back to its character, so the galley's glyph list stays in typing order while
//! the glyphs sit at their visual positions. Typing, Backspace and the arrow
//! keys work on the text as typed. Right-to-left glyphs are stored mirrored
//! (start at their right edge, negative advance) so the caret sits on the
//! correct side. Clicks and the selection highlight assume x grows along the
//! text, so `text_field` handles those itself with `cursor_at` and
//! `paint_selection`.

use crate::shaping::needs_shaping;
use eframe::egui::{self, text::LayoutJob, Galley};
use std::sync::Arc;
use unicode_bidi::{BidiInfo, Direction, Level};

/// How a field draws its text.
pub struct FieldStyle {
    pub font: egui::FontId,
    pub color: egui::Color32,
    pub line_height: f32,
}

/// The text reads right to left: its first strong letter is Arabic, or it has
/// none and the interface is Arabic.
pub fn rtl_base(text: &str) -> bool {
    match unicode_bidi::get_base_direction(text) {
        Direction::Rtl => true,
        Direction::Ltr => false,
        Direction::Mixed => crate::i18n::rtl(),
    }
}

/// Bidi level of each character.
fn levels(text: &str, rtl: bool) -> Vec<Level> {
    let info = BidiInfo::new(text, Some(if rtl { Level::rtl() } else { Level::ltr() }));
    text.char_indices().map(|(b, _)| info.levels[b]).collect()
}

fn format(st: &FieldStyle) -> egui::TextFormat {
    egui::TextFormat { font_id: st.font.clone(), color: st.color, line_height: Some(st.line_height), ..Default::default() }
}

fn job(text: String, st: &FieldStyle, align: egui::Align) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text, st.font.clone(), st.color);
    job.halign = align;
    job.keep_trailing_whitespace = true;
    for section in &mut job.sections {
        section.format.line_height = Some(st.line_height);
    }
    job
}

/// Lam-alef ligatures: one presentation form for two letters.
fn lam_alef(c: char) -> bool {
    ('\u{FEF5}'..='\u{FEFC}').contains(&c)
}

/// For each logical character, the joined form that draws it, or None when
/// it's the second letter of a lam-alef ligature (drawn by the lam).
fn shaped_forms(chars: &[char], text: &str) -> Option<Vec<Option<char>>> {
    let shaped: Vec<char> = crate::shaping::reshape(text).chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut j = 0;
    while out.len() < chars.len() {
        let c = *shaped.get(j)?;
        out.push(Some(c));
        if lam_alef(c) {
            out.push(None);
        }
        j += 1;
    }
    (j == shaped.len() && out.len() == chars.len()).then_some(out)
}

/// Lay out a single-line field's text (see the module docs).
///
/// egui 0.37's shaper mangles the glyph list of right-to-left runs (extra
/// zero-width glyphs), so it's never handed one: the letters arrive already
/// joined and in reading order, each in a layout section of its own, which
/// makes every Arabic run a single glyph.
pub fn layout(ui: &egui::Ui, text: &str, st: &FieldStyle, align: egui::Align) -> Arc<Galley> {
    let plain = || ui.fonts_mut(|f| f.layout_job(job(text.to_string(), st, align)));
    if !needs_shaping(text) {
        return plain();
    }
    let chars: Vec<char> = text.chars().collect();
    let Some(forms) = shaped_forms(&chars, text) else {
        return plain();
    };
    let rtl = rtl_base(text);
    let levels = levels(text, rtl);
    // What gets drawn, left to right: (form, logical index of the character).
    let units: Vec<(char, usize)> = BidiInfo::reorder_visual(&levels)
        .into_iter()
        .filter_map(|i| forms[i].map(|c| (crate::shaping::mirror_in(c, levels[i].is_rtl()), i)))
        .collect();
    let mut shown = LayoutJob { halign: align, keep_trailing_whitespace: true, ..Default::default() };
    let fmt = format(st);
    for (k, &(c, _)) in units.iter().enumerate() {
        let start = egui::text::ByteIndex(shown.text.len());
        shown.text.push(c);
        let end = egui::text::ByteIndex(shown.text.len());
        // Arabic letters stand alone; other text joins the previous section.
        let alone = crate::shaping::arabic_font_char(c);
        let prev_alone = k > 0 && crate::shaping::arabic_font_char(units[k - 1].0);
        match shown.sections.last_mut() {
            Some(last) if !alone && !prev_alone => last.byte_range.end = end,
            _ => shown.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: start..end, format: fmt.clone() }),
        }
    }
    let galley = ui.fonts_mut(|f| f.layout_job(shown));

    let mut g = (*galley).clone();
    let Some(placed) = g.rows.first_mut() else {
        return plain();
    };
    let row = Arc::make_mut(&mut placed.row);
    if row.glyphs.len() != units.len() {
        return plain();
    }
    let mut by_char: Vec<Option<egui::epaint::text::Glyph>> = vec![None; chars.len()];
    for (glyph, &(_, i)) in row.glyphs.iter().zip(&units) {
        let mut glyph = *glyph;
        if levels[i].is_rtl() {
            // Stored from its right edge, so the caret before it sits on the right.
            glyph.pos.x += glyph.advance_width;
            glyph.advance_width = -glyph.advance_width;
        }
        by_char[i] = Some(glyph);
    }
    // The alef of a lam-alef: a zero-width glyph at the ligature's far side.
    for i in 0..chars.len() {
        if by_char[i].is_none() {
            let mut glyph = by_char.get(i.wrapping_sub(1)).copied().flatten().unwrap_or(row.glyphs[0]);
            glyph.chr = chars[i];
            glyph.pos.x += glyph.advance_width;
            glyph.advance_width = 0.0;
            by_char[i] = Some(glyph);
        }
    }
    let glyphs: Vec<_> = by_char.into_iter().flatten().collect();
    // The caret after the last character: the far left for right-to-left text.
    if rtl {
        row.size.x = glyphs.iter().map(|g| span(g).0).fold(f32::INFINITY, f32::min);
    }
    row.glyphs = glyphs;
    g.job = Arc::new(job(text.to_string(), st, align));
    Arc::new(g)
}

/// Left and right edge of a glyph, whichever way it's stored.
fn span(g: &egui::epaint::text::Glyph) -> (f32, f32) {
    let (a, b) = (g.pos.x, g.pos.x + g.advance_width);
    (a.min(b), a.max(b))
}

/// The character index a click at galley-relative `x` puts the caret at.
pub fn cursor_at(galley: &Galley, text: &str, x: f32) -> usize {
    let Some(placed) = galley.rows.first() else {
        return 0;
    };
    let x = x - placed.pos.x;
    let glyphs = &placed.row.glyphs;
    let rtl = rtl_base(text);
    let levels = levels(text, rtl);
    for (i, g) in glyphs.iter().enumerate() {
        let (l, r) = span(g);
        if x >= l && x < r {
            let right_half = x > (l + r) / 2.0;
            let after = if levels.get(i).is_some_and(|l| l.is_rtl()) { !right_half } else { right_half };
            return i + after as usize;
        }
    }
    let (min, max) = glyphs.iter().map(span).fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), (l, r)| (a.min(l), b.max(r)));
    let n = glyphs.len();
    match (x < (min + max) / 2.0, rtl) {
        (true, true) | (false, false) => n,
        _ => 0,
    }
}

/// Highlight the selected characters (egui's own highlight assumes x grows
/// along the text, so it can't draw a right-to-left selection).
pub fn paint_selection(ui: &egui::Ui, galley: &Galley, galley_pos: egui::Pos2, range: std::ops::Range<usize>, clip: egui::Rect) {
    let Some(placed) = galley.rows.first() else {
        return;
    };
    let painter = ui.painter().with_clip_rect(clip);
    let color = ui.visuals().selection.bg_fill.gamma_multiply(0.55);
    let rect = placed.rect().translate(galley_pos.to_vec2());
    for g in placed.row.glyphs.get(range).unwrap_or_default() {
        let (l, r) = span(g);
        let r = egui::Rect::from_x_y_ranges(rect.left() + l..=rect.left() + r, rect.y_range());
        painter.rect_filled(r, 0.0, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_direction_follows_the_first_letter() {
        assert!(rtl_base("مرحبا Hades"));
        assert!(!rtl_base("Hades مرحبا"));
    }
}
