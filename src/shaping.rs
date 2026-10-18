//! Arabic text for egui, which draws glyphs one by one, left to right, with no
//! shaping: letters are swapped for their joined presentation forms, then each
//! line is put into visual order with the Unicode bidi algorithm.
//!
//! egui 0.37 runs each same-font run through harfrust, which reverses runs of
//! Arabic script itself. The bundled Arabic font comes first in every family
//! and holds only Arabic-block glyphs, so those runs are exactly the maximal
//! stretches of Arabic-block characters; `for_egui` pre-reverses them so the
//! shaper's reversal lands them back in visual order.

use ar_reshaper::prelude::*;
use unicode_bidi::{BidiInfo, Level};

thread_local! {
    static RESHAPER: ArabicReshaper = {
        let mut cfg = ReshaperConfig::default();
        // The bundled font has the lam-alef forms but not the "Allah" ligature.
        cfg.update_ligature(LigatureNames::ARABIC_LIGATURE_ALLAH, false);
        cfg.to_reshaper()
    };
}

/// `s` with Arabic letters swapped for their joined presentation forms, in
/// logical order.
pub fn reshape(s: &str) -> String {
    RESHAPER.with(|r| r.reshape(s))
}

/// Contains right-to-left script that needs shaping/reordering.
pub fn needs_shaping(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// Mirror bracket-like characters drawn inside a right-to-left run.
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        c => c,
    }
}

/// `c` as drawn in a right-to-left run when `rtl` (brackets face the other way).
pub fn mirror_in(c: char, rtl: bool) -> char {
    if rtl { mirror(c) } else { c }
}

/// Shaped, visual-order text. `rtl_base` sets the paragraph direction (an
/// Arabic interface); otherwise it follows the first strong character.
pub fn visual(s: &str, rtl_base: bool) -> String {
    s.split('\n')
        .map(|line| visual_line(line, rtl_base))
        .collect::<Vec<_>>()
        .join("\n")
}

fn visual_line(line: &str, rtl_base: bool) -> String {
    // A line with no right-to-left letters (a path in an Arabic message) keeps its order.
    if !needs_shaping(line) {
        return strip_isolates(line);
    }
    let shaped = RESHAPER.with(|r| r.reshape(line));
    let base = rtl_base.then(Level::rtl);
    let info = BidiInfo::new(&shaped, base);
    let Some(para) = info.paragraphs.first() else {
        return shaped;
    };
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut out = String::with_capacity(shaped.len());
    for run in runs {
        let text = &shaped[run.clone()];
        if levels[run.start].is_rtl() {
            out.extend(text.chars().rev().map(mirror));
        } else {
            out.push_str(text);
        }
    }
    for_egui(&strip_isolates(&out))
}

/// Drop bidi isolate marks (they've done their job once the text is ordered).
pub fn strip_isolates(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\u{2066}'..='\u{2069}')).collect()
}

/// A character egui draws with the bundled Arabic font (see the module docs).
pub fn arabic_font_char(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x08FF | 0xFB50..=0xFEFF | 0x200C..=0x200F)
}

/// Undo, in advance, the reversal harfrust applies to each Arabic-font run.
fn for_egui(visual: &str) -> String {
    let chars: Vec<char> = visual.chars().collect();
    egui_order(&chars).into_iter().map(|k| chars[k]).collect()
}

/// The order to hand `chars` (already in visual order) to egui in: each
/// Arabic-font run reversed, so harfrust's own reversal puts it back. It only
/// reverses runs it guesses are Arabic script, i.e. ones with a letter or an
/// Arabic digit; punctuation-only runs keep their order.
pub fn egui_order(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if !arabic_font_char(chars[i]) {
            out.push(i);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && arabic_font_char(chars[i]) {
            i += 1;
        }
        let run = &chars[start..i];
        if run.iter().any(|&c| c.is_alphabetic() || matches!(c as u32, 0x0660..=0x0669 | 0x06F0..=0x06F9)) {
            out.extend((start..i).rev());
        } else {
            out.extend(start..i);
        }
    }
    out
}

/// Break logical `text` into lines no wider than `width` (as measured by
/// `measure` on the drawn form), then put each line in visual order. Needed
/// because wrapping already-reordered text would read the lines bottom-up.
pub fn wrap_visual(text: &str, width: f32, rtl_base: bool, mut measure: impl FnMut(&str) -> f32) -> String {
    let mut lines: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let candidate = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if !cur.is_empty() && measure(&visual_line(&candidate, rtl_base)) > width {
                lines.push(visual_line(&cur, rtl_base));
                cur = word.to_string();
            } else {
                cur = candidate;
            }
        }
        lines.push(visual_line(&cur, rtl_base));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_is_joined_and_reversed() {
        // "سلام" → seen-initial, lam-alef ligature, meem-final, drawn right to left.
        let v = visual("سلام", true);
        assert_eq!(v.chars().count(), 3, "{v:?}");
        assert!(v.chars().all(|c| ('\u{FE70}'..='\u{FEFF}').contains(&c)));
        // A single Arabic run stays in logical order: egui's shaper reverses it
        // so the first letter (seen) lands on the right.
        assert_eq!(v.chars().next(), Some('\u{FEB3}'));
        // Word order is visual: the first word is drawn rightmost.
        let two = visual("واحد اثنان", true);
        assert!(two.starts_with(&visual("اثنان", true)), "{two}");
        // Latin and numbers inside Arabic keep their own order.
        let mixed = visual("شغّل Hades الآن 12", true);
        assert!(mixed.contains("Hades"), "{mixed}");
        assert!(mixed.contains("12"), "{mixed}");
        assert!(mixed.find("12").unwrap() < mixed.find("Hades").unwrap());
        assert!(!needs_shaping("Hollow Knight"));
        assert_eq!(visual("(a)", false), "(a)");
        // An isolated path keeps its leading slash on its own (left) end.
        let path = visual("يثبت في \u{2066}/home/x\u{2069}", true);
        assert!(path.starts_with("/home/x "), "{path}");
    }

    #[test]
    fn wrapping_keeps_the_first_words_on_the_first_line() {
        let text = "واحد اثنان ثلاثة أربعة";
        let w = wrap_visual(text, 30.0, true, |s| s.chars().count() as f32 * 3.0);
        let first = w.lines().next().unwrap();
        // The first line holds the first word(s) of the sentence.
        assert!(visual("واحد", true).chars().all(|c| first.contains(c)), "{w}");
        assert!(w.lines().count() >= 2);
    }
}

