//! Store info for the game page: description, facts, screenshots, links and
//! the ProtonDB rating, from Steam's public store API (no key needed) and
//! ProtonDB. Text comes in the interface language when Steam has it (it falls
//! back to English by itself).

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Info {
    pub appid: u64,
    pub name: String,
    pub about: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    pub released: String,
    pub genres: Vec<String>,
    /// (thumbnail, full size) image URLs.
    pub shots: Vec<(String, String)>,
    /// ProtonDB tier ("platinum", "gold"…), empty when unknown.
    #[serde(default)]
    pub protondb: String,
    /// Steam's language name the text was fetched in.
    #[serde(default)]
    pub lang: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    /// None = looked up and not on the store.
    pub info: Option<Info>,
    pub fetched: u64,
    /// Picked by hand: don't replace it with an automatic match.
    #[serde(default)]
    pub pinned: bool,
}

/// A store search result.
#[derive(Clone, Debug)]
pub struct Hit {
    pub id: u64,
    pub name: String,
}

fn cache_path() -> PathBuf {
    data_dir().join("store.json")
}

fn load_cache() -> HashMap<String, Entry> {
    fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_cache(c: &HashMap<String, Entry>) {
    let _ = fs::create_dir_all(data_dir());
    let tmp = cache_path().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(c).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, cache_path());
    }
}

/// Steam's name for the interface language.
pub fn steam_lang() -> &'static str {
    if crate::i18n::rtl() { "arabic" } else { "english" }
}

fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let body = http_fetch(url, None)?;
    serde_json::from_str(&body).map_err(|e| format!("{}: {e}", lg("The store sent something unexpected")))
}

/// Search the Steam store for games called `query`.
pub fn search(query: &str) -> Result<Vec<Hit>, String> {
    let url = format!("https://store.steampowered.com/api/storesearch/?term={}&l=english&cc=US", url_encode(query));
    let v = get_json(&url)?;
    Ok(v["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|i| i["type"].as_str().is_none_or(|t| t == "app"))
                .filter_map(|i| Some(Hit { id: i["id"].as_u64()?, name: i["name"].as_str()?.to_string() }))
                .collect()
        })
        .unwrap_or_default())
}

/// The result named like `name` (ignoring punctuation), else the first one.
/// ("Hades" must not land on "Hades II", which the store lists first.)
pub fn best<'a>(name: &str, hits: &'a [Hit]) -> Option<&'a Hit> {
    let n = proton::norm_title(name);
    hits.iter().find(|h| proton::norm_title(&h.name) == n).or_else(|| hits.first())
}

/// Steam's app id from a umu id like "umu-274190".
pub fn appid_from_umu(umu: &str) -> Option<u64> {
    umu.strip_prefix("umu-")?.parse().ok()
}

/// Plain text from the store's HTML snippets.
pub fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    for (entity, c) in [("&quot;", "\""), ("&#39;", "'"), ("&apos;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&nbsp;", " "), ("&amp;", "&")] {
        out = out.replace(entity, c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Everything the store and ProtonDB know about app `id`, in `lang`.
pub fn details(id: u64, lang: &str) -> Result<Option<Info>, String> {
    let v = get_json(&format!("https://store.steampowered.com/api/appdetails?appids={id}&l={lang}"))?;
    let app = &v[id.to_string()];
    if !app["success"].as_bool().unwrap_or(false) {
        return Ok(None);
    }
    let d = &app["data"];
    let strings = |key: &str| -> Vec<String> {
        d[key].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let protondb = get_json(&format!("https://www.protondb.com/api/v1/reports/summaries/{id}.json"))
        .ok()
        .and_then(|p| p["tier"].as_str().map(String::from))
        .unwrap_or_default();
    Ok(Some(Info {
        appid: id,
        name: d["name"].as_str().unwrap_or_default().to_string(),
        about: plain(d["short_description"].as_str().unwrap_or_default()),
        developers: strings("developers"),
        publishers: strings("publishers"),
        released: d["release_date"]["date"].as_str().unwrap_or_default().to_string(),
        genres: d["genres"]
            .as_array()
            .map(|a| a.iter().filter_map(|g| g["description"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        shots: d["screenshots"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| Some((s["path_thumbnail"].as_str()?.to_string(), s["path_full"].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        protondb,
        lang: lang.to_string(),
    }))
}

/// Find a game on the store: by its Steam id when the umu id has one, else by name.
pub fn lookup(name: &str, umu_id: &str, lang: &str) -> Result<Option<Info>, String> {
    let id = match appid_from_umu(umu_id) {
        Some(id) => id,
        None => match best(name, &search(name)?) {
            Some(h) => h.id,
            None => return Ok(None),
        },
    };
    details(id, lang)
}

/// Where a store image is kept on disk.
pub(crate) fn image_path(url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    data_dir().join("cache/store").join(format!("{:016x}.jpg", h.finish()))
}

pub enum Msg {
    Fetched(String, Result<Option<Info>, String>, bool),
    Results(Result<Vec<Hit>, String>),
    Image(String, Option<image::RgbaImage>),
}

/// The "find it on Steam" dialog: game id, search text, results so far.
pub type Picker = (String, String, Option<Result<Vec<Hit>, String>>);

pub struct StoreState {
    pub cache: HashMap<String, Entry>,
    pub pending: HashSet<String>,
    pub errors: HashMap<String, String>,
    pub picker: Option<Picker>,
    pub searching: bool,
    /// Screenshot textures by URL, and the ones being fetched.
    pub images: HashMap<String, egui::TextureHandle>,
    pub loading: HashSet<String>,
    /// The full-size viewer: game id and screenshot index.
    pub viewer: Option<(String, usize)>,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for StoreState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        StoreState {
            cache: load_cache(),
            pending: HashSet::new(),
            errors: HashMap::new(),
            picker: None,
            searching: false,
            images: HashMap::new(),
            loading: HashSet::new(),
            viewer: None,
            tx,
            rx,
        }
    }
}

/// ProtonDB tier: label and colour.
pub(crate) fn tier_style(tier: &str) -> Option<(&'static str, egui::Color32)> {
    let c = egui::Color32::from_rgb;
    Some(match tier {
        "native" => ("Native", c(0x4c, 0xc2, 0x6b)),
        "platinum" => ("Platinum", c(0xb4, 0xc7, 0xdc)),
        "gold" => ("Gold", c(0xcf, 0xb5, 0x3b)),
        "silver" => ("Silver", c(0xa6, 0xa6, 0xa6)),
        "bronze" => ("Bronze", c(0xcd, 0x7f, 0x32)),
        "borked" => ("Borked", c(0xe0, 0x4f, 0x4f)),
        _ => return None,
    })
}

impl App {
    /// Look game `i` up in the background unless it's cached in the current
    /// language (a miss is retried after a week).
    pub(crate) fn store_ensure(&mut self, ctx: &egui::Context, i: usize) {
        let g = &self.cfg.custom_games[i];
        let id = g.id.clone();
        if self.cfg.settings.no_store || self.store.pending.contains(&id) || self.store.errors.contains_key(&id) {
            return;
        }
        let lang = steam_lang();
        let pinned_id = match self.store.cache.get(&id) {
            Some(Entry { info: Some(info), .. }) if info.lang == lang => return,
            Some(Entry { info: None, fetched, .. }) if unix_now().saturating_sub(*fetched) < 7 * 86_400 => return,
            // Fetched in another language: fetch the same game again.
            Some(Entry { info: Some(info), pinned, .. }) => Some((info.appid, *pinned)),
            _ => None,
        };
        self.store.pending.insert(id.clone());
        let (tx, ctx) = (self.store.tx.clone(), ctx.clone());
        let (name, umu) = (g.name.clone(), g.umu_id.clone());
        std::thread::spawn(move || {
            let (r, pinned) = match pinned_id {
                Some((appid, pinned)) => (details(appid, lang), pinned),
                None => (lookup(&name, &umu, lang), false),
            };
            let _ = tx.send(Msg::Fetched(id, r, pinned));
            ctx.request_repaint();
        });
    }

    pub fn store_poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.store.rx.try_recv() {
            match msg {
                Msg::Fetched(id, r, pinned) => {
                    self.store.pending.remove(&id);
                    match r {
                        Ok(info) => {
                            self.store.cache.insert(id, Entry { info, fetched: unix_now(), pinned });
                            save_cache(&self.store.cache);
                        }
                        Err(e) => {
                            self.store.errors.insert(id, e);
                        }
                    }
                }
                Msg::Results(r) => {
                    self.store.searching = false;
                    if let Some(p) = self.store.picker.as_mut() {
                        p.2 = Some(r);
                    }
                }
                Msg::Image(url, img) => {
                    self.store.loading.remove(&url);
                    if let Some(img) = img {
                        let tex = load_rgba(ctx, url.clone(), &img);
                        self.store.images.insert(url, tex);
                    }
                }
            }
        }
    }

    /// A screenshot's texture, fetching (and caching on disk) it when needed.
    pub(crate) fn store_image(&mut self, ctx: &egui::Context, url: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.store.images.get(url) {
            return Some(t.clone());
        }
        if self.store.loading.insert(url.to_string()) {
            let (tx, ctx, url) = (self.store.tx.clone(), ctx.clone(), url.to_string());
            std::thread::spawn(move || {
                let path = image_path(&url);
                if !path.is_file() {
                    let _ = fs::create_dir_all(path.parent().unwrap_or(&path));
                    let tmp = path.with_extension("part");
                    if http_fetch(&url, Some(&tmp)).is_ok() {
                        let _ = fs::rename(&tmp, &path);
                    }
                }
                let _ = tx.send(Msg::Image(url, decode_image(&path)));
                ctx.request_repaint();
            });
        }
        None
    }

    fn store_search(&mut self, ctx: &egui::Context, query: String) {
        self.store.searching = true;
        let (tx, ctx) = (self.store.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Results(search(&query)));
            ctx.request_repaint();
        });
    }

    /// The "About" card at the top of the game page's Overview, and the
    /// screenshots strip under it.
    pub fn about_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, acts: &mut Acts) {
        if self.cfg.settings.no_store {
            return;
        }
        self.store_ensure(ctx, i);
        let (id, name) = {
            let g = &self.cfg.custom_games[i];
            (g.id.clone(), g.name.clone())
        };
        let entry = self.store.cache.get(&id).cloned();
        let pending = self.store.pending.contains(&id);
        let error = self.store.errors.get(&id).cloned();
        let mut open_picker = false;
        let mut retry = false;
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr("About")).strong().color(pal().text));
                ui.with_layout(far_end(), |ui| {
                    if entry.is_some() && ui.small_button(tr("Not this game?")).clicked() {
                        open_picker = true;
                    }
                    if let Some((label, color)) = entry.as_ref().and_then(|e| e.info.as_ref()).and_then(|i| tier_style(&i.protondb)) {
                        pill(ui, &lfmt("ProtonDB: {}", &[&lg(label)]), color);
                    }
                });
            });
            ui.add_space(8.0);
            match (&entry, pending, &error) {
                (_, true, _) if entry.as_ref().is_none_or(|e| e.info.is_none()) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Looking it up on the Steam store…")).color(pal().subtext));
                    });
                }
                (_, _, Some(e)) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
                        if ui.small_button(tr("Retry")).clicked() {
                            retry = true;
                        }
                    });
                }
                (Some(Entry { info: Some(info), .. }), _, _) => {
                    if !info.about.is_empty() {
                        para(ui, &info.about, pal().text, true);
                        ui.add_space(10.0);
                    }
                    let facts = [
                        ("Developer", info.developers.join(", ")),
                        ("Publisher", info.publishers.join(", ")),
                        ("Released", info.released.clone()),
                    ];
                    for (label, value) in facts.iter().filter(|(_, v)| !v.is_empty()) {
                        modal_row(ui, label, |ui| {
                            ui.label(egui::RichText::new(disp(value)).color(pal().text));
                        });
                    }
                    if !info.genres.is_empty() {
                        modal_row(ui, "Genres", |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for genre in &info.genres {
                                    pill(ui, genre, pal().subtext);
                                }
                            });
                        });
                    }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let links = [
                            ("Steam store", format!("https://store.steampowered.com/app/{}", info.appid)),
                            ("PCGamingWiki", format!("https://www.pcgamingwiki.com/api/appid.php?appid={}", info.appid)),
                            ("ProtonDB", format!("https://www.protondb.com/app/{}", info.appid)),
                        ];
                        for (label, url) in links {
                            if ui.button(tr(label)).on_hover_text(&url).clicked() {
                                acts.open = Some(url);
                            }
                        }
                    });
                }
                (Some(Entry { info: None, .. }), _, _) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(tr("Not found on the Steam store.")).color(pal().subtext));
                        if ui.small_button(tr("Find it…")).clicked() {
                            open_picker = true;
                        }
                    });
                }
                _ => {}
            }
        });
        if retry {
            self.store.errors.remove(&id);
        }
        if open_picker {
            self.store_search(ctx, name.clone());
            self.store.picker = Some((id.clone(), name, None));
        }
        let shots = entry.and_then(|e| e.info).map(|i| i.shots).unwrap_or_default();
        if !shots.is_empty() {
            ui.add_space(12.0);
            self.screenshots_strip(ui, ctx, &id, &shots);
        }
    }

    /// A row of screenshot thumbnails; a click opens the viewer.
    fn screenshots_strip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, id: &str, shots: &[(String, String)]) {
        let mut open: Option<usize> = None;
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Screenshots")).strong().color(pal().text));
            ui.add_space(10.0);
            egui::ScrollArea::horizontal().id_salt(("shots", id)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (n, (thumb, _)) in shots.iter().enumerate() {
                        let size = egui::vec2(213.0, 120.0);
                        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let h = ctx.animate_bool_with_time(egui::Id::unique(("shot-hover", id, n)), resp.hovered(), self.secs(0.14));
                        let r = rect.shrink(4.0 * (1.0 - h)).translate(egui::vec2(0.0, -2.0 * h));
                        match self.store_image(ctx, thumb) {
                            Some(t) => {
                                ui.painter().add(cover_fill_shape(&t, r, 8, 1.0));
                            }
                            None => {
                                ui.painter().rect_filled(r, rad(8), pal().card2);
                            }
                        }
                        let stroke = egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h));
                        ui.painter().rect_stroke(r, rad(8), stroke, egui::StrokeKind::Inside);
                        if resp.on_hover_cursor(egui::CursorIcon::ZoomIn).clicked() {
                            open = Some(n);
                        }
                    }
                });
            });
        });
        if let Some(n) = open {
            self.store.viewer = Some((id.to_string(), n));
        }
    }

    /// Full-size screenshots: arrows (or the controller) flip, Escape closes.
    pub fn screenshot_viewer(&mut self, ctx: &egui::Context) {
        let Some((id, mut n)) = self.store.viewer.clone() else {
            return;
        };
        let shots = self.store.cache.get(&id).and_then(|e| e.info.clone()).map(|i| i.shots).unwrap_or_default();
        if shots.is_empty() {
            self.store.viewer = None;
            return;
        }
        n = n.min(shots.len() - 1);
        let (left, right) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        let step = right as isize - left as isize;
        n = (n as isize + step).rem_euclid(shots.len() as isize) as usize;
        let (thumb, full) = shots[n].clone();
        let tex = self.store_image(ctx, &full).or_else(|| self.store_image(ctx, &thumb));
        // Fetch the neighbours ahead of time.
        for k in [n + 1, n + shots.len() - 1] {
            let (_, f) = &shots[k % shots.len()];
            let _ = self.store_image(ctx, f);
        }
        let screen = ctx.content_rect();
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::unique("shot-viewer"))
            .frame(egui::Frame::NONE)
            .backdrop_color(egui::Color32::from_black_alpha(225))
            .show(ctx, |ui| {
                let area = egui::vec2(screen.width() - 120.0, screen.height() - 120.0);
                let size = match &tex {
                    Some(t) => {
                        let [w, h] = t.size();
                        let s = (area.x / w as f32).min(area.y / h as f32);
                        egui::vec2(w as f32 * s, h as f32 * s)
                    }
                    None => egui::vec2(area.x, area.x * 9.0 / 16.0).min(area),
                };
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                match &tex {
                    Some(t) => {
                        ui.painter().add(cover_fill_shape(t, rect, 10, 1.0));
                    }
                    None => {
                        ui.painter().rect_filled(rect, rad(10), pal().card);
                        ui.put(rect, egui::Spinner::new());
                    }
                }
                // Click the left or right third to flip, the middle to close.
                if let Some(p) = resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand).interact_pointer_pos().filter(|_| resp.clicked()) {
                    let third = (p.x - rect.left()) / rect.width();
                    if third < 0.33 {
                        n = (n + shots.len() - 1) % shots.len();
                    } else if third > 0.66 {
                        n = (n + 1) % shots.len();
                    } else {
                        close = true;
                    }
                }
                let counter = format!("{} / {}", n + 1, shots.len());
                ui.painter().text(
                    rect.center_bottom() + egui::vec2(0.0, 14.0),
                    egui::Align2::CENTER_TOP,
                    counter,
                    egui::FontId::proportional(14.0),
                    egui::Color32::from_white_alpha(200),
                );
            });
        if close || resp.should_close() {
            self.store.viewer = None;
        } else {
            self.store.viewer = Some((id, n));
        }
    }

    /// The "find it on Steam" dialog.
    pub fn store_picker(&mut self, ctx: &egui::Context) {
        let Some((id, mut query, results)) = self.store.picker.take() else { return };
        let mut search_again = false;
        let mut chosen: Option<Option<u64>> = None;
        let mut close = false;
        let searching = self.store.searching;
        let resp = egui::Modal::new(egui::Id::unique("store-picker")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(480.0);
            ui.label(egui::RichText::new(tr("Find it on the Steam store")).size(18.0).strong().color(pal().text));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let w = ui.available_width() - button_width(ui, "Search") - 8.0;
                let (r, _) = text_field(ui, &mut query, "Game name…", w);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if primary_button(ui, "Search").clicked() || enter {
                    search_again = true;
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| match &results {
                _ if searching => {
                    ui.spinner();
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(egui::RichText::new(tr("No results")).color(pal().subtext));
                }
                Some(Ok(list)) => {
                    for h in list {
                        if ui.selectable_label(false, disp(&h.name)).clicked() {
                            chosen = Some(Some(h.id));
                        }
                    }
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(disp(e)).color(pal().warn));
                }
                None => {}
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
                if ui.button(tr("It's not on Steam")).clicked() {
                    chosen = Some(None);
                }
            });
        }));
        match chosen {
            Some(Some(appid)) => {
                self.store.pending.insert(id.clone());
                self.store.errors.remove(&id);
                let (tx, ctx) = (self.store.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Fetched(id, details(appid, steam_lang()), true));
                    ctx.request_repaint();
                });
                return;
            }
            Some(None) => {
                self.store.cache.insert(id, Entry { info: None, fetched: unix_now(), pinned: true });
                save_cache(&self.store.cache);
                return;
            }
            None => {}
        }
        if close || resp.should_close() {
            return;
        }
        if search_again {
            self.store_search(ctx, query.clone());
            self.store.picker = Some((id, query, None));
        } else {
            self.store.picker = Some((id, query, results));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_text_and_matching() {
        assert_eq!(plain("<b>Fast</b> &amp; <i>loud</i>&quot;!&quot;"), "Fast & loud\"!\"");
        assert_eq!(appid_from_umu("umu-274190"), Some(274190));
        assert_eq!(appid_from_umu("umu-hollow-knight"), None);
        let hits = vec![Hit { id: 1, name: "Hades II".into() }, Hit { id: 2, name: "Hades".into() }];
        assert_eq!(best("Hades", &hits).unwrap().id, 2);
        assert_eq!(best("Something", &hits).unwrap().id, 1);
    }

    /// cargo test -- --ignored store_live --nocapture
    #[test]
    #[ignore]
    fn store_live() {
        let info = lookup("Broforce", "", "arabic").unwrap().unwrap();
        eprintln!("{info:#?}");
        assert_eq!(info.appid, 274190);
        assert!(!info.shots.is_empty() && !info.genres.is_empty());
        assert_eq!(info.protondb, "platinum");
    }
}
