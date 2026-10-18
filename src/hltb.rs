//! How long to beat: completion times from howlongtobeat.com, shown next to
//! the playtime you've put in.

use crate::i18n::{disp, logical as lg, tr, trf};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

const SITE: &str = "https://howlongtobeat.com";
/// The site ties its search token to the browser it was issued to.
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Times {
    pub id: u64,
    pub name: String,
    /// Seconds; 0 = not enough data.
    pub main: u64,
    pub plus: u64,
    pub full: u64,
    #[serde(default)]
    pub year: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    /// None = looked up and not found.
    pub times: Option<Times>,
    pub fetched: u64,
    /// The user picked this match by hand; don't replace it automatically.
    #[serde(default)]
    pub pinned: bool,
}

fn cache_path() -> PathBuf {
    data_dir().join("hltb.json")
}

pub fn load_cache() -> HashMap<String, Entry> {
    fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_cache(c: &HashMap<String, Entry>) {
    let _ = fs::create_dir_all(data_dir());
    let tmp = cache_path().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(c).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, cache_path());
    }
}

/// curl with the browser user agent; `post` sends a JSON body.
fn curl(url: &str, headers: &[(&str, &str)], post: Option<&str>) -> Result<(u32, String), String> {
    let mut c = Command::new("curl");
    c.args(["-sSL", "--max-time", "20", "-A", UA, "-w", "\n%{http_code}"])
        .args(["-H", &format!("Referer: {SITE}/"), "-H", &format!("Origin: {SITE}")]);
    for (k, v) in headers {
        c.arg("-H").arg(format!("{k}: {v}"));
    }
    if let Some(body) = post {
        c.args(["-H", "Content-Type: application/json", "--data-binary", body]);
    }
    let out = c.arg(url).output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("HowLongToBeat: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n').unwrap_or((&text, "0"));
    Ok((code.trim().parse().unwrap_or(0), body.to_string()))
}

/// The search endpoint's name ("site" today). It has changed over time, so if
/// the known one stops answering, read the current one from the site's scripts.
fn discover_endpoint() -> Option<String> {
    let (_, home) = curl(SITE, &[], None).ok()?;
    for script in home.split("src=\"").skip(1).filter_map(|s| s.split('"').next()).filter(|s| s.ends_with(".js")) {
        let url = if script.starts_with("http") { script.to_string() } else { format!("{SITE}{script}") };
        let Ok((_, js)) = curl(&url, &[], None) else { continue };
        if let Some(i) = js.find("/api/search/") {
            let rest = &js[i + "/api/search/".len()..];
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if !name.is_empty() && rest[name.len()..].starts_with("/init") {
                return Some(name);
            }
        }
    }
    None
}

fn token(endpoint: &str) -> Result<String, String> {
    let (code, body) = curl(&format!("{SITE}/api/search/{endpoint}/init?t={}", unix_now() * 1000), &[], None)?;
    if code != 200 {
        return Err(format!("HTTP {code}"));
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    v.get("token").and_then(|t| t.as_str()).map(String::from).ok_or_else(|| "no token".into())
}

pub fn search(query: &str) -> Result<Vec<Times>, String> {
    let mut endpoint = "site".to_string();
    let tok = match token(&endpoint) {
        Ok(t) => t,
        Err(_) => {
            endpoint = discover_endpoint().ok_or_else(|| lg("HowLongToBeat changed how its search works").to_string())?;
            token(&endpoint)?
        }
    };
    let terms: Vec<&str> = query.split_whitespace().collect();
    let body = serde_json::json!({
        "searchType": "games",
        "searchTerms": terms,
        "searchPage": 1,
        "size": 20,
        "searchOptions": {
            "games": {
                "userId": 0, "platform": "", "sortCategory": "popular", "rangeCategory": "main",
                "rangeTime": {"min": null, "max": null},
                "gameplay": {"perspective": "", "flow": "", "genre": "", "difficulty": ""},
                "rangeYear": {"min": "", "max": ""}, "modifier": ""
            },
            "users": {"sortCategory": "postcount"},
            "lists": {"sortCategory": "follows"},
            "filter": "", "sort": 0, "randomizer": 0
        },
        "useCache": true
    })
    .to_string();
    let (code, text) = curl(&format!("{SITE}/api/search/{endpoint}"), &[("x-auth-token", &tok)], Some(&body))?;
    if code == 429 {
        return Err(lg("HowLongToBeat says too many searches — try again in a minute").to_string());
    }
    if code != 200 {
        return Err(format!("HowLongToBeat: HTTP {code}"));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let list = v.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|g| {
            let n = |k: &str| g.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            Some(Times {
                id: g.get("game_id")?.as_u64()?,
                name: g.get("game_name")?.as_str()?.to_string(),
                main: n("comp_main"),
                plus: n("comp_plus"),
                full: n("comp_100"),
                year: n("release_world"),
            })
        })
        .collect())
}

/// The result that fits `name`: same title ignoring punctuation, else the
/// most popular (first) one.
pub fn best<'a>(name: &str, results: &'a [Times]) -> Option<&'a Times> {
    let n = proton::norm_title(name);
    results.iter().find(|t| proton::norm_title(&t.name) == n).or_else(|| results.first())
}

/// "27h", "45m", "—".
pub fn hours(secs: u64) -> String {
    match secs {
        0 => "—".into(),
        s if s < 3600 => format!("{}m", (s / 60).max(1)),
        s if s < 36_000 => {
            let h = s as f64 / 3600.0;
            if (h - h.round()).abs() < 0.25 { format!("{}h", h.round()) } else { format!("{h:.1}h") }
        }
        s => format!("{}h", (s as f64 / 3600.0).round()),
    }
}

pub enum Msg {
    Fetched(String, Result<Option<Times>, String>),
    Results(Result<Vec<Times>, String>),
}

/// The picker dialog: game id, search text, results so far.
pub type Picker = (String, String, Option<Result<Vec<Times>, String>>);

pub struct HltbState {
    pub cache: HashMap<String, Entry>,
    pub pending: std::collections::HashSet<String>,
    pub errors: HashMap<String, String>,
    /// The "find on HowLongToBeat" dialog: (game id, query, results).
    pub picker: Option<Picker>,
    pub searching: bool,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for HltbState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        HltbState {
            cache: load_cache(),
            pending: Default::default(),
            errors: HashMap::new(),
            picker: None,
            searching: false,
            tx,
            rx,
        }
    }
}

impl App {
    /// Look game `id` up in the background unless it's cached (a miss is
    /// retried after a week).
    pub fn hltb_ensure(&mut self, ctx: &egui::Context, id: &str, name: &str) {
        if self.cfg.settings.no_hltb || self.hltb.pending.contains(id) || self.hltb.errors.contains_key(id) {
            return;
        }
        if let Some(e) = self.hltb.cache.get(id)
            && (e.times.is_some() || unix_now().saturating_sub(e.fetched) < 7 * 86_400)
        {
            return;
        }
        self.hltb.pending.insert(id.to_string());
        let (tx, ctx, id, name) = (self.hltb.tx.clone(), ctx.clone(), id.to_string(), name.to_string());
        std::thread::spawn(move || {
            let r = search(&name).map(|list| best(&name, &list).cloned());
            let _ = tx.send(Msg::Fetched(id, r));
            ctx.request_repaint();
        });
    }

    pub fn hltb_poll(&mut self) {
        while let Ok(msg) = self.hltb.rx.try_recv() {
            match msg {
                Msg::Fetched(id, r) => {
                    self.hltb.pending.remove(&id);
                    match r {
                        Ok(times) => {
                            self.hltb.cache.insert(id, Entry { times, fetched: unix_now(), pinned: false });
                            save_cache(&self.hltb.cache);
                        }
                        Err(e) => {
                            self.hltb.errors.insert(id, e);
                        }
                    }
                }
                Msg::Results(r) => {
                    self.hltb.searching = false;
                    if let Some(p) = self.hltb.picker.as_mut() {
                        p.2 = Some(r);
                    }
                }
            }
        }
    }

    fn hltb_search(&mut self, ctx: &egui::Context, query: String) {
        self.hltb.searching = true;
        let (tx, ctx) = (self.hltb.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Results(search(&query)));
            ctx.request_repaint();
        });
    }

    /// The "How long to beat" card on the game page.
    pub fn hltb_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, played: u64) {
        if self.cfg.settings.no_hltb {
            return;
        }
        let (id, name) = {
            let g = &self.cfg.custom_games[i];
            (g.id.clone(), g.name.clone())
        };
        self.hltb_ensure(ctx, &id, &name);
        let entry = self.hltb.cache.get(&id).cloned();
        let pending = self.hltb.pending.contains(&id);
        let error = self.hltb.errors.get(&id).cloned();
        let mut open_picker = false;
        let mut retry = false;
        let mut open_page: Option<u64> = None;
        let progress_secs = self.secs(0.9);
        let since = self.motion.page_since;
        let grow = self.progress(since, progress_secs.max(0.001));
        section(ui, "How long to beat", |ui| {
            match (&entry, pending, &error) {
                (_, true, _) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Asking HowLongToBeat…")).color(pal().subtext));
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
                (Some(Entry { times: Some(t), .. }), _, _) => {
                    ui.horizontal(|ui| {
                        for (label, secs) in [(tr("Main story"), t.main), (tr("Main + extras"), t.plus), (tr("Completionist"), t.full)] {
                            stat_block(ui, &label, &hours(secs));
                            ui.add_space(18.0);
                        }
                        ui.with_layout(if rtl() { egui::Layout::left_to_right(egui::Align::Min) } else { egui::Layout::right_to_left(egui::Align::Min) }, |ui| {
                            if ui.small_button(tr("Not this game?")).clicked() {
                                open_picker = true;
                            }
                            if ui.small_button(tr("Open")).on_hover_text(disp(&t.name)).clicked() {
                                open_page = Some(t.id);
                            }
                        });
                    });
                    // Your playtime against the main story, with marks for the longer runs.
                    let target = [t.main, t.plus, t.full].into_iter().filter(|&s| s > 0).max().unwrap_or(0);
                    if target > 0 {
                        ui.add_space(10.0);
                        let w = ui.available_width();
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 8.0), egui::Sense::hover());
                        let p = ui.painter();
                        p.rect_filled(rect, rad(4), pal().card2);
                        let frac = (played as f32 / target as f32).clamp(0.0, 1.0) * grow;
                        if frac > 0.0 {
                            let fw = (w * frac).max(8.0);
                            let fill = if rtl() {
                                egui::Rect::from_min_size(egui::pos2(rect.right() - fw, rect.top()), egui::vec2(fw, rect.height()))
                            } else {
                                egui::Rect::from_min_size(rect.min, egui::vec2(fw, rect.height()))
                            };
                            p.rect_filled(fill, rad(4), pal().accent);
                        }
                        for s in [t.main, t.plus] {
                            if s > 0 && s < target {
                                let off = w * (s as f32 / target as f32);
                                let x = if rtl() { rect.right() - off } else { rect.left() + off };
                                p.line_segment([egui::pos2(x, rect.top() - 3.0), egui::pos2(x, rect.bottom() + 3.0)], egui::Stroke::new(1.5, pal().subtext));
                            }
                        }
                        ui.add_space(4.0);
                        let line = if played == 0 {
                            tr("You haven't played it yet")
                        } else if t.main > 0 && played < t.main {
                            trf("{} played — about {}% of the main story", &[&format_duration(played), &(played * 100 / t.main)])
                        } else if t.plus > 0 && played < t.plus {
                            trf("{} played — past the main story's usual length", &[&format_duration(played)])
                        } else {
                            trf("{} played — longer than most completionists", &[&format_duration(played)])
                        };
                        ui.label(egui::RichText::new(line).color(pal().subtext).small());
                    }
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr("Not found on HowLongToBeat")).color(pal().subtext));
                        if ui.small_button(tr("Search…")).clicked() {
                            open_picker = true;
                        }
                    });
                }
            }
        });
        if retry {
            self.hltb.errors.remove(&id);
            self.hltb.cache.remove(&id);
        }
        if let Some(gid) = open_page {
            self.children.extend(xdg_open(format!("{SITE}/game/{gid}")));
        }
        if open_picker {
            self.hltb.picker = Some((id, name.clone(), None));
            self.hltb_search(ctx, name);
        }
    }

    /// Dialog to pick the right HowLongToBeat entry.
    pub fn hltb_modal(&mut self, ctx: &egui::Context) {
        let Some((id, mut query, results)) = self.hltb.picker.take() else { return };
        let mut search_again = false;
        let mut chosen: Option<Times> = None;
        let mut close = false;
        let searching = self.hltb.searching;
        let resp = egui::Modal::new(egui::Id::unique("hltb-picker")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(480.0);
            ui.label(egui::RichText::new(tr("Find it on HowLongToBeat")).size(18.0).strong().color(pal().text));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let w = ui.available_width() - 90.0;
                let (r, _) = text_field(ui, &mut query, &tr("Game name…"), w);
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
                    for t in list {
                        let year = if t.year > 0 { format!(" ({})", t.year) } else { String::new() };
                        let label = disp(&format!("{}{year}  ·  {} / {} / {}", t.name, hours(t.main), hours(t.plus), hours(t.full)));
                        if ui.selectable_label(false, label).clicked() {
                            chosen = Some(t.clone());
                        }
                    }
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(disp(e)).color(pal().warn));
                }
                None => {}
            });
            ui.add_space(8.0);
            if ui.button(tr("Cancel")).clicked() {
                close = true;
            }
        }));
        if let Some(t) = chosen {
            self.hltb.cache.insert(id.clone(), Entry { times: Some(t), fetched: unix_now(), pinned: true });
            self.hltb.errors.remove(&id);
            save_cache(&self.hltb.cache);
            return;
        }
        if close || resp.should_close() {
            return;
        }
        if search_again {
            self.hltb_search(ctx, query.clone());
            self.hltb.picker = Some((id, query, None));
        } else {
            self.hltb.picker = Some((id, query, results));
        }
    }
}

/// A big number with a small caption under it.
pub fn stat_block(ui: &mut egui::Ui, label: &str, value: &str) {
    vcol(ui, |ui| {
        ui.label(egui::RichText::new(value).size(22.0).strong().color(pal().text));
        ui.label(egui::RichText::new(label).color(pal().subtext).small());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_read_naturally() {
        assert_eq!(hours(0), "—");
        assert_eq!(hours(45 * 60), "45m");
        assert_eq!(hours(97_100), "27h");
        assert_eq!(hours(5 * 3600 + 30 * 60), "5.5h");
        assert_eq!(hours(3 * 3600 + 5 * 60), "3h");
    }

    /// cargo test -- --ignored hltb_live --nocapture
    #[test]
    #[ignore]
    fn hltb_live() {
        let r = search("Hollow Knight").unwrap();
        let hk = best("Hollow Knight", &r).unwrap();
        eprintln!("{hk:?}");
        assert_eq!(hk.name, "Hollow Knight");
        assert!(hk.main > 10 * 3600);
    }
}
