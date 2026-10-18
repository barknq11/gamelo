//! Interface language. English strings are the keys; `tr` looks them up in
//! the active language's table. Arabic text is shaped (joined letter forms)
//! and put in visual order here, because egui lays text out left to right
//! one glyph at a time.
//!
//! Text that has already been through `tr` contains presentation forms and
//! passes through `tr` again unchanged, so helpers can translate whatever
//! they're given without double-processing.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ar,
}

impl Lang {
    pub fn from_key(key: &str) -> Lang {
        match key {
            "ar" => Lang::Ar,
            "en" => Lang::En,
            // Unset: follow the desktop's locale.
            _ => {
                let env = ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .iter()
                    .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
                    .unwrap_or_default();
                if env.starts_with("ar") { Lang::Ar } else { Lang::En }
            }
        }
    }
}

/// Global so worker threads build their messages in the same language.
static LANG: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static CACHE: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Tests pick a language per thread, so they can run in parallel.
    static OVERRIDE: std::cell::Cell<Option<Lang>> = const { std::cell::Cell::new(None) };
}

pub fn set_lang(l: Lang) {
    if lang() != l {
        LANG.store(l as u8, Ordering::Relaxed);
        CACHE.with(|c| c.borrow_mut().clear());
    }
}

pub fn lang() -> Lang {
    if let Some(l) = OVERRIDE.with(|o| o.get()) {
        return l;
    }
    if LANG.load(Ordering::Relaxed) == Lang::Ar as u8 { Lang::Ar } else { Lang::En }
}

/// Right-to-left layout is in effect.
pub fn rtl() -> bool {
    lang() == Lang::Ar
}

fn table() -> Option<&'static HashMap<&'static str, &'static str>> {
    use std::sync::OnceLock;
    static AR: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    match lang() {
        Lang::Ar => Some(AR.get_or_init(|| crate::i18n_ar::AR.iter().copied().collect())),
        Lang::En => None,
    }
}

/// Already shaped by us (contains Arabic presentation forms).
fn shaped(s: &str) -> bool {
    s.chars().any(|c| ('\u{FB50}'..='\u{FEFF}').contains(&c))
}

/// The translation of `s` in logical order (unshaped), or `s` itself.
pub fn logical(s: &str) -> &str {
    table().and_then(|t| t.get(s).copied()).unwrap_or(s)
}

/// `s` in the interface language, ready to draw.
pub fn tr(s: &str) -> String {
    if lang() == Lang::En || shaped(s) {
        return disp(s);
    }
    CACHE.with(|c| {
        if let Some(v) = c.borrow().get(s) {
            return v.clone();
        }
        let l = logical(s);
        // Untranslated, Latin-only text keeps its order (a path, a version…).
        let v = if crate::shaping::needs_shaping(l) { crate::shaping::visual(l, rtl()) } else { l.to_string() };
        c.borrow_mut().insert(s.to_string(), v.clone());
        v
    })
}

/// A template with `{}` placeholders, translated and filled in order, in
/// logical order (for wrapping helpers).
///
/// In a right-to-left interface, values without Arabic (paths, names,
/// numbers) are wrapped in bidi isolates, so a path's leading "/" or a
/// trailing "…" stays with the value instead of drifting to the Arabic side.
/// The isolates are stripped again when the text is prepared for drawing.
pub fn fmt_logical(s: &str, args: &[&dyn std::fmt::Display]) -> String {
    let t = logical(s);
    let iso = rtl();
    let mut out = String::with_capacity(t.len() + 16);
    let mut it = args.iter();
    let mut rest = t;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(a) = it.next() {
            let a = a.to_string();
            if iso && !crate::shaping::needs_shaping(&a) {
                out.push('\u{2066}');
                out.push_str(&a);
                out.push('\u{2069}');
            } else {
                out.push_str(&a);
            }
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// A translated template with `{}` placeholders filled in order, ready to draw.
pub fn trf(s: &str, args: &[&dyn std::fmt::Display]) -> String {
    disp(&fmt_logical(s, args))
}

/// Prepare arbitrary text (a game name, an error) for drawing: Arabic is
/// shaped and reordered; anything else comes back unchanged.
pub fn disp(s: &str) -> String {
    if !shaped(s) && crate::shaping::needs_shaping(s) {
        crate::shaping::visual(s, rtl())
    } else {
        crate::shaping::strip_isolates(s)
    }
}

/// Logical text for wrapping: a translation key or plain logical text, or
/// None if it was already shaped (and can't be re-wrapped).
pub fn for_wrap(s: &str) -> Option<String> {
    if shaped(s) { None } else { Some(logical(s).to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every literal handed to tr/trf/lg/lfmt or to a translating helper has an
    /// Arabic entry (brand names and symbols excepted).
    #[test]
    fn every_string_is_translated() {
        let sources = [
            include_str!("main.rs"),
            include_str!("proton.rs"),
            include_str!("saves.rs"),
            include_str!("saves_ui.rs"),
            include_str!("hltb.rs"),
            include_str!("optiscaler.rs"),
            include_str!("extras.rs"),
            include_str!("store.rs"),
            include_str!("couch.rs"),
            include_str!("library_list.rs"),
            include_str!("stores.rs"),
            include_str!("stores_ui.rs"),
            include_str!("cleanup.rs"),
            include_str!("home.rs"),
            include_str!("stats_ui.rs"),
            include_str!("sessions.rs"),
            include_str!("quick.rs"),
            include_str!("setup.rs"),
            include_str!("tray.rs"),
            include_str!("launchers.rs"),
            include_str!("emulation.rs"),
        ];
        let calls = [
            "tr(\"", "trf(\"", "lg(\"", "lfmt(\"", "note(ui, indent, \"", "note(ui, 0.0, \"", "note(ui, 128.0, \"",
            "section(ui, \"", "section_sub(ui, \"", "page_header(ui, \"", "modal_row(ui, \"", "para(ui, \"",
            "primary_button(ui, \"", "danger_button(ui, \"", "pill(ui, \"", "empty_hint(ui, \"", "item(ui, \"",
        ];
        let table: std::collections::HashSet<&str> = crate::i18n_ar::AR.iter().map(|(k, _)| *k).collect();
        let mut missing = Vec::new();
        for src in sources {
            let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
            for call in calls {
                for (i, _) in src.match_indices(call) {
                    // Skip identifiers that merely end in a helper's name.
                    let before = src[..i].chars().last().unwrap_or(' ');
                    if before.is_alphanumeric() || before == '_' {
                        continue;
                    }
                    let rest = &src[i + call.len()..];
                    let mut key = String::new();
                    let mut chars = rest.chars();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => {
                                match chars.next() {
                                    Some('n') => key.push('\n'),
                                    Some(o) => key.push(o),
                                    None => {}
                                }
                            }
                            '"' => break,
                            c => key.push(c),
                        }
                    }
                    let latin_only = !key.chars().any(|c| c.is_lowercase()) || key.contains("OptiScaler.ini") || key.starts_with("FSR");
                    if !table.contains(key.as_str()) && !latin_only && !missing.contains(&key) {
                        missing.push(key);
                    }
                }
            }
        }
        assert!(missing.is_empty(), "untranslated: {missing:#?}");
    }

    #[test]
    fn translating_twice_is_harmless() {
        OVERRIDE.with(|o| o.set(Some(Lang::Ar)));
        let once = tr("Library");
        assert_ne!(once, "Library");
        assert_eq!(tr(&once), once, "already-shaped text passes through");
        assert_eq!(tr("Not a known key"), "Not a known key");
        assert!(trf("{} games", &[&3]).contains('3'));
        OVERRIDE.with(|o| o.set(Some(Lang::En)));
        assert_eq!(tr("Library"), "Library");
        OVERRIDE.with(|o| o.set(None));
    }
}
