//! The Proton page: installed builds, downloadable releases (GE-Proton,
//! Proton-CachyOS, Proton-EM, UMU-Proton) and protonfixes / umu-database info.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::theme::pal;
use crate::*;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

/// A Proton build published as GitHub releases.
pub struct Source {
    pub key: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    pub blurb: &'static str,
}

pub const SOURCES: &[Source] = &[
    Source {
        key: "ge",
        name: "GE-Proton",
        repo: "GloriousEggroll/proton-ge-custom",
        blurb: "The most popular community build: extra media codecs, game patches and protonfixes.",
    },
    Source {
        key: "cachyos",
        name: "Proton-CachyOS",
        repo: "CachyOS/proton-cachyos",
        blurb: "CachyOS's build with performance patches and FSR4 / NTSync support, tuned per CPU level.",
    },
    Source {
        key: "em",
        name: "Proton-EM",
        repo: "BananaWorks07/Proton",
        blurb: "Experimental build with HDR and the newest Wayland work.",
    },
    Source {
        key: "umu",
        name: "UMU-Proton",
        repo: "Open-Wine-Components/umu-proton",
        blurb: "Valve's Proton with umu's protonfixes — closest to what Steam ships.",
    },
];

#[derive(Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub published: String,
    pub page: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

/// One row of umu-database.csv.
#[derive(Clone)]
pub struct UmuEntry {
    pub title: String,
    pub store: String,
    pub umu_id: String,
}

pub enum Msg {
    Releases(&'static str, Result<Vec<Release>, String>),
    UmuDb(Result<Vec<UmuEntry>, String>),
    Fixes(Result<HashSet<String>, String>),
}

pub struct ProtonState {
    pub source: usize,
    pub releases: HashMap<&'static str, Result<Vec<Release>, String>>,
    pub loading: HashSet<&'static str>,
    pub umu_db: Option<Result<Vec<UmuEntry>, String>>,
    pub fixes: Option<Result<HashSet<String>, String>>,
    pub db_loading: bool,
    pub confirm_delete: Option<PathBuf>,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    pub show_all: bool,
}

impl Default for ProtonState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        ProtonState {
            source: 0,
            releases: HashMap::new(),
            loading: HashSet::new(),
            umu_db: None,
            fixes: None,
            db_loading: false,
            confirm_delete: None,
            tx,
            rx,
            show_all: false,
        }
    }
}

/// Fetch `url`, reusing a copy in the cache dir younger than `max_age` secs.
/// A stale copy is still used when the network fails.
pub fn fetch_cached(url: &str, name: &str, max_age: u64, force: bool) -> Result<String, String> {
    let dir = data_dir().join("cache");
    let path = dir.join(name);
    let age = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs());
    if !force && age.is_some_and(|a| a < max_age)
        && let Ok(s) = fs::read_to_string(&path)
    {
        return Ok(s);
    }
    match http_fetch(url, None) {
        Ok(body) => {
            let _ = fs::create_dir_all(&dir);
            let tmp = path.with_extension("tmp");
            if fs::write(&tmp, &body).is_ok() {
                let _ = fs::rename(&tmp, &path);
            }
            Ok(body)
        }
        Err(e) => fs::read_to_string(&path).map_err(|_| e),
    }
}

pub fn parse_releases(body: &str) -> Result<Vec<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad release list: {e}"))?;
    let Some(list) = v.as_array() else {
        let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("unexpected reply");
        return Err(format!("GitHub: {msg}"));
    };
    Ok(list
        .iter()
        .filter_map(|r| {
            Some(Release {
                tag: r.get("tag_name")?.as_str()?.to_string(),
                published: r.get("published_at").and_then(|p| p.as_str()).unwrap_or("").chars().take(10).collect(),
                page: r.get("html_url").and_then(|p| p.as_str()).unwrap_or("").to_string(),
                prerelease: r.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
                assets: r
                    .get("assets")?
                    .as_array()?
                    .iter()
                    .filter_map(|a| {
                        Some(Asset {
                            name: a.get("name")?.as_str()?.to_string(),
                            url: a.get("browser_download_url")?.as_str()?.to_string(),
                            size: a.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect(),
            })
        })
        .collect())
}

pub fn fetch_releases(repo: &str, force: bool) -> Result<Vec<Release>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=20");
    let name = format!("releases-{}.json", repo.replace('/', "_"));
    parse_releases(&fetch_cached(&url, &name, 3600, force)?)
}

/// The CPU supports x86-64-v3 (AVX2 etc.), for picking Proton-CachyOS builds.
fn cpu_has_v3() -> bool {
    let info = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let flags = info.lines().find(|l| l.starts_with("flags")).unwrap_or("");
    ["avx2", "bmi2", "fma", "movbe"]
        .iter()
        .all(|f| flags.split_whitespace().any(|w| w == *f))
}

/// The archive to download from a release, for this machine.
pub fn pick_asset<'a>(source: &str, assets: &'a [Asset]) -> Option<&'a Asset> {
    let archive = |n: &str| n.ends_with(".tar.gz") || n.ends_with(".tar.xz") || n.ends_with(".tar.zst");
    let arm = |n: &str| n.contains("aarch64") || n.contains("arm64");
    let pref: Option<&str> = match source {
        "cachyos" => Some(if cpu_has_v3() { "x86_64_v3." } else { "x86_64." }),
        "ge" => Some("x86_64."),
        _ => None,
    };
    pref.and_then(|p| assets.iter().find(|a| archive(&a.name) && a.name.contains(p)))
        .or_else(|| assets.iter().find(|a| archive(&a.name) && !arm(&a.name)))
}

/// The checksum file published next to `asset`, if any.
fn checksum_for<'a>(asset: &Asset, assets: &'a [Asset]) -> Option<&'a Asset> {
    let stem = asset
        .name
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".tar.xz")
        .trim_end_matches(".tar.zst");
    assets
        .iter()
        .find(|a| a.name.starts_with(stem) && (a.name.ends_with(".sha512sum") || a.name.ends_with(".sha256sum")))
}

/// Where downloaded Proton builds go: Steam's compatibilitytools.d when Steam
/// is installed (so Steam can use them too), else our own folder.
pub fn install_root(steam_root: Option<&Path>) -> PathBuf {
    match steam_root {
        Some(sr) => sr.join("compatibilitytools.d"),
        None => data_dir().join("proton"),
    }
}

/// Folders we may delete Proton builds from.
pub fn deletable(dir: &Path) -> bool {
    let h = home();
    [data_dir().join("proton"), h.join(".local/share/Steam/compatibilitytools.d"), h.join(".steam/steam/compatibilitytools.d")]
        .iter()
        .any(|root| dir.parent() == Some(root.as_path()) || dir.parent().and_then(|p| p.canonicalize().ok()) == root.canonicalize().ok())
}

/// Path a download is written to while in progress (watched for progress).
pub fn download_path(name: &str) -> PathBuf {
    data_dir().join("downloads").join(name)
}

/// Download, verify and unpack a release archive into `root`.
pub fn install_release(root: PathBuf, asset: Asset, checksum: Option<Asset>) -> Result<String, String> {
    let dl = data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let archive = download_path(&asset.name);
    let _ = fs::remove_file(&archive);
    http_fetch(&asset.url, Some(&archive))?;
    if let Some(sum) = checksum {
        let expected = http_fetch(&sum.url, None)?;
        let expected = expected.split_whitespace().next().unwrap_or("").to_lowercase();
        let tool = if sum.name.ends_with(".sha512sum") { "sha512sum" } else { "sha256sum" };
        if let Ok(out) = Command::new(tool).arg(&archive).output() {
            let got = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or("").to_lowercase();
            if !expected.is_empty() && !got.is_empty() && got != expected {
                let _ = fs::remove_file(&archive);
                return Err(lg("The download is corrupted (checksum mismatch) — try again").to_string());
            }
        }
    }
    let staging = root.join(format!(".ptmp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = fs::remove_file(&archive);
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(lg("Unpacking the archive failed").to_string());
    }
    let proton = find_file_recursive(&staging, "proton").ok_or_else(|| lg("The archive did not contain a Proton build").to_string())?;
    let tool_dir = proton.parent().ok_or("unexpected archive layout")?.to_path_buf();
    let name = tool_dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| asset.name.clone());
    let final_dir = root.join(&name);
    if final_dir.exists() {
        let _ = fs::remove_dir_all(&final_dir);
    }
    fs::rename(&tool_dir, &final_dir).map_err(|e| format!("moving Proton build: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    let _ = make_executable(&final_dir.join("proton"));
    Ok(lfmt("Installed {}", &[&name]))
}

pub fn parse_umu_db(csv: &str) -> Vec<UmuEntry> {
    let mut out = Vec::new();
    for line in csv.lines().skip(1) {
        let cols = split_csv(line);
        if cols.len() >= 4 && !cols[3].is_empty() {
            out.push(UmuEntry { title: cols[0].clone(), store: cols[1].clone(), umu_id: cols[3].clone() });
        }
    }
    out
}

/// Split one CSV line, honouring double quotes.
fn split_csv(line: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => q = !q,
            ',' if !q => cols.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    cols.push(cur);
    cols
}

/// Lower-case letters and digits only, for loose title matching.
pub fn norm_title(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The database entry for a game name: an exact (normalised) title match,
/// preferring standalone ("none") then Steam entries.
pub fn lookup_umu<'a>(db: &'a [UmuEntry], name: &str) -> Option<&'a UmuEntry> {
    let n = norm_title(name);
    if n.is_empty() {
        return None;
    }
    let rank = |e: &UmuEntry| match e.store.as_str() {
        "none" => 0,
        "steam" => 1,
        _ => 2,
    };
    db.iter().filter(|e| norm_title(&e.title) == n).min_by_key(|e| rank(e))
}

/// File names (without .py) of every protonfix, as "store/id".
pub fn parse_fix_tree(body: &str) -> Result<HashSet<String>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("bad fix list: {e}"))?;
    let tree = v.get("tree").and_then(|t| t.as_array()).ok_or("bad fix list")?;
    Ok(tree
        .iter()
        .filter_map(|t| t.get("path")?.as_str())
        .filter_map(|p| {
            let (dir, file) = p.split_once('/')?;
            let store = dir.strip_prefix("gamefixes-")?;
            let id = file.strip_suffix(".py")?;
            (id != "__init__" && id != "default").then(|| format!("{store}/{id}"))
        })
        .collect())
}

/// The protonfix umu would run for this GAMEID (with no STORE set), as a
/// path in the umu-protonfixes repo.
pub fn fix_for(fixes: &HashSet<String>, gameid: &str) -> Option<String> {
    let key = if gameid.chars().all(|c| c.is_ascii_digit()) {
        format!("steam/{gameid}")
    } else {
        format!("umu/{gameid}")
    };
    fixes.contains(&key).then(|| {
        let (store, id) = key.split_once('/').unwrap_or(("umu", gameid));
        format!("gamefixes-{store}/{id}.py")
    })
}

pub const FIXES_REPO_URL: &str = "https://github.com/Open-Wine-Components/umu-protonfixes/blob/master/";

impl App {
    /// Load the umu database and the protonfix list in the background (cached for a week).
    pub fn load_fix_data(&mut self, ctx: &egui::Context, force: bool) {
        if self.proton.db_loading {
            return;
        }
        self.proton.db_loading = true;
        let (tx, ctx) = (self.proton.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let week = 7 * 86_400;
            let db = fetch_cached(
                "https://raw.githubusercontent.com/Open-Wine-Components/umu-database/main/umu-database.csv",
                "umu-database.csv",
                week,
                force,
            )
            .map(|s| parse_umu_db(&s));
            let _ = tx.send(Msg::UmuDb(db));
            let fixes = fetch_cached(
                "https://api.github.com/repos/Open-Wine-Components/umu-protonfixes/git/trees/master?recursive=1",
                "protonfixes-tree.json",
                week,
                force,
            )
            .and_then(|s| parse_fix_tree(&s));
            let _ = tx.send(Msg::Fixes(fixes));
            ctx.request_repaint();
        });
    }

    pub fn load_releases(&mut self, ctx: &egui::Context, source: usize, force: bool) {
        let Some(src) = SOURCES.get(source) else { return };
        if self.proton.loading.contains(src.key) {
            return;
        }
        self.proton.loading.insert(src.key);
        let (tx, ctx, repo, key) = (self.proton.tx.clone(), ctx.clone(), src.repo, src.key);
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Releases(key, fetch_releases(repo, force)));
            ctx.request_repaint();
        });
    }

    pub fn proton_poll(&mut self) {
        while let Ok(msg) = self.proton.rx.try_recv() {
            match msg {
                Msg::Releases(key, r) => {
                    self.proton.loading.remove(key);
                    self.proton.releases.insert(key, r);
                }
                Msg::UmuDb(r) => self.proton.umu_db = Some(r),
                Msg::Fixes(r) => {
                    self.proton.db_loading = false;
                    self.proton.fixes = Some(r);
                }
            }
        }
    }

    /// The umu-database entry for game `i`, when the database is loaded.
    pub fn umu_match(&self, name: &str) -> Option<UmuEntry> {
        let Some(Ok(db)) = &self.proton.umu_db else { return None };
        lookup_umu(db, name).cloned()
    }

    fn install_proton(&mut self, src: &'static Source, rel: &Release) {
        let Some(asset) = pick_asset(src.key, &rel.assets).cloned() else {
            self.set_status(tr("No download for this machine in that release"), true);
            return;
        };
        let sum = checksum_for(&asset, &rel.assets).cloned();
        let root = install_root(self.steam_root.as_deref());
        let progress = Some((download_path(&asset.name), asset.size));
        let label = lfmt("Installing {}", &[&rel.tag]);
        self.start_task(TaskKind::Proton, &format!("proton:{}", rel.tag), &label, progress, move || {
            install_release(root, asset, sum)
        });
    }

    pub fn page_proton(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        page_header(ui, "Proton", "Download Proton builds, manage the ones you have, and see which games have fixes");
        ui.set_max_width(900.0);

        // ---- Installed
        let usage = |dir: &Path| -> Vec<String> {
            self.cfg
                .custom_games
                .iter()
                .filter(|g| g.kind == GameKind::Windows && Path::new(&g.proton_dir) == dir)
                .map(|g| g.name.clone())
                .collect()
        };
        let tools: Vec<(String, PathBuf, Vec<String>)> =
            self.proton_tools.iter().map(|t| (t.label.clone(), t.dir.clone(), usage(&t.dir))).collect();
        let default = self.cfg.settings.default_proton.clone();
        let mut set_default: Option<String> = None;
        section_sub(ui, "Installed", "Proton builds found on this computer", |ui| {
            if tools.is_empty() {
                ui.label(egui::RichText::new(tr("None yet — install one below.")).color(pal().warn));
            }
            for (label, dir, users) in &tools {
                let is_default = dir.to_string_lossy() == default;
                ui.horizontal(|ui| {
                    status_dot(ui, if users.is_empty() { pal().faint } else { pal().ok }, !users.is_empty());
                    vcol(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(label).strong().color(pal().text));
                            if is_default {
                                pill(ui, "default", pal().accent);
                            }
                            if !deletable(dir) && dir.starts_with("/usr") {
                                let r = ui.scope(|ui| pill(ui, "system", pal().faint)).response;
                                r.on_hover_text(tr("Installed by your system's package manager — remove it from there"));
                            }
                        });
                        let used = match users.len() {
                            0 => tr("not used by any game"),
                            1 => trf("used by {}", &[&users[0]]),
                            n => trf("used by {} games", &[&n]),
                        };
                        ui.label(egui::RichText::new(used).color(pal().subtext).small())
                            .on_hover_text(dir.display().to_string());
                    });
                    ui.with_layout(far_end(), |ui| {
                        if deletable(dir) && ui.button(tr("Delete…")).clicked() {
                            self.proton.confirm_delete = Some(dir.clone());
                        }
                        if ui.button(tr("Open")).clicked() {
                            acts.open = Some(dir.to_string_lossy().into());
                        }
                        if !is_default && ui.button(tr("Make default")).on_hover_text(tr("New games will use it")).clicked() {
                            set_default = Some(dir.to_string_lossy().into());
                        }
                    });
                });
                ui.add_space(6.0);
            }
        });
        if let Some(d) = set_default {
            self.cfg.settings.default_proton = d;
            acts.save_settings = true;
        }
        ui.add_space(12.0);

        // ---- Download
        let src_idx = self.proton.source.min(SOURCES.len() - 1);
        let src = &SOURCES[src_idx];
        if !self.proton.releases.contains_key(src.key) {
            self.load_releases(ctx, src_idx, false);
        }
        let installed: Vec<String> = self.proton_tools.iter().map(|t| t.label.to_lowercase()).collect();
        let active: HashSet<String> = self.tasks.iter().map(|t| t.key.clone()).collect();
        let mut install: Option<Release> = None;
        let mut refresh = false;
        let loading = self.proton.loading.contains(src.key);
        let show_all = self.proton.show_all;
        let mut toggle_all = false;
        let releases = self.proton.releases.get(src.key);
        let task_progress: HashMap<String, f32> = self.tasks.iter().map(|t| (t.key.clone(), t.fraction())).collect();
        section_sub(ui, "Get Proton", "Pick a build and install it with one click — no Steam needed", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (n, s) in SOURCES.iter().enumerate() {
                    if choice_chip(ui, n == src_idx, s.name).clicked() {
                        self.proton.source = n;
                    }
                }
                ui.with_layout(far_end(), |ui| {
                    if ui.add_enabled(!loading, egui::Button::new(tr("Refresh"))).clicked() {
                        refresh = true;
                    }
                    if loading {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(tr(src.blurb)).color(pal().subtext).small());
            ui.add_space(8.0);
            match releases {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Loading releases…")).color(pal().subtext));
                    });
                }
                Some(Err(e)) => {
                    ui.label(egui::RichText::new(disp(e)).color(pal().warn));
                }
                Some(Ok(list)) => {
                    let shown = if show_all { list.len() } else { list.len().min(6) };
                    for (n, rel) in list.iter().take(shown).enumerate() {
                        let asset = pick_asset(src.key, &rel.assets);
                        let tag_l = rel.tag.to_lowercase();
                        let is_installed = installed.iter().any(|l| l.contains(&tag_l) || tag_l.contains(l.as_str()));
                        let key = format!("proton:{}", rel.tag);
                        ui.horizontal(|ui| {
                            vcol(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&rel.tag).strong().color(pal().text));
                                    if n == 0 {
                                        pill(ui, "latest", pal().ok);
                                    }
                                    if rel.prerelease {
                                        pill(ui, "pre-release", pal().warn);
                                    }
                                });
                                let size = asset.map(|a| format_bytes(a.size)).unwrap_or_else(|| "—".into());
                                ui.label(egui::RichText::new(format!("{} · {size}", rel.published)).color(pal().subtext).small());
                            });
                            ui.with_layout(far_end(), |ui| {
                                if let Some(f) = task_progress.get(&key) {
                                    ui.add(egui::ProgressBar::new(*f).desired_width(160.0).show_percentage());
                                } else if is_installed {
                                    pill(ui, "installed", pal().ok);
                                } else if asset.is_some()
                                    && ui.add_enabled(!active.contains(&key), egui::Button::new(tr("Install"))).clicked()
                                {
                                    install = Some(rel.clone());
                                }
                                if !rel.page.is_empty() && ui.small_button(tr("Notes")).clicked() {
                                    acts.open = Some(rel.page.clone());
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                    if list.len() > 6 && ui.button(if show_all { tr("Show fewer") } else { tr("Show older releases") }).clicked() {
                        toggle_all = true;
                    }
                }
            }
            ui.add_space(4.0);
            let root = install_root(self.steam_root.as_deref());
            note(ui, 0.0, &lfmt("Builds are installed to {}", &[&root.display()]));
        });
        if toggle_all {
            self.proton.show_all = !self.proton.show_all;
        }
        if refresh {
            self.proton.releases.remove(src.key);
            self.load_releases(ctx, src_idx, true);
        }
        if let Some(rel) = install {
            self.install_proton(src, &rel);
        }
        ui.add_space(12.0);

        // ---- Fixes
        if self.proton.umu_db.is_none() && !self.proton.db_loading {
            self.load_fix_data(ctx, false);
        }
        self.fixes_section(ui, ctx, acts);
    }

    fn fixes_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let db_state = match &self.proton.umu_db {
            None => None,
            Some(Ok(db)) => Some(Ok(db.len())),
            Some(Err(e)) => Some(Err(e.clone())),
        };
        let fixes = match &self.proton.fixes {
            Some(Ok(f)) => Some(f.clone()),
            _ => None,
        };
        let rows: Vec<(String, String, String, Option<UmuEntry>, bool)> = self
            .cfg
            .custom_games
            .iter()
            .filter(|g| g.kind == GameKind::Windows)
            .map(|g| (g.id.clone(), g.name.clone(), game_umu_id(g), self.umu_match(&g.name), g.flags.no_protonfixes))
            .collect();
        let loading = self.proton.db_loading;
        let mut reload = false;
        section_sub(ui, "Fixes", "protonfixes: automatic per-game tweaks umu applies when a game's umu ID is known", |ui| {
            ui.horizontal(|ui| {
                match &db_state {
                    None => {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Loading the umu database…")).color(pal().subtext).small());
                    }
                    Some(Ok(n)) => {
                        status_dot(ui, pal().ok, true);
                        ui.label(egui::RichText::new(trf("umu database: {} entries", &[n])).color(pal().subtext).small());
                        if let Some(f) = &fixes {
                            ui.label(egui::RichText::new(trf("· {} game fixes", &[&f.len()])).color(pal().subtext).small());
                        }
                    }
                    Some(Err(e)) => {
                        status_dot(ui, pal().warn, true);
                        ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
                    }
                }
                ui.with_layout(far_end(), |ui| {
                    if ui.add_enabled(!loading, egui::Button::new(tr("Update"))).clicked() {
                        reload = true;
                    }
                });
            });
            ui.add_space(8.0);
            if rows.is_empty() {
                ui.label(egui::RichText::new(tr("No Windows games in your library yet.")).color(pal().subtext));
            }
            for (id, name, gameid, entry, disabled) in &rows {
                card_inner().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        vcol(ui, |ui| {
                            ui.label(egui::RichText::new(disp(name)).strong().color(pal().text));
                            ui.label(egui::RichText::new(format!("GAMEID {gameid}")).color(pal().subtext).small().monospace());
                        });
                        ui.with_layout(far_end(), |ui| {
                            let mut off = *disabled;
                            if check(ui, &mut off, tr("Disable fixes")).on_hover_text(tr("Sets PROTONFIXES_DISABLE=1 for this game")).changed() {
                                acts.game.push((id.clone(), GameAct::SetNoFixes(off)));
                            }
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        match fixes.as_ref().map(|f| fix_for(f, gameid)) {
                            Some(Some(path)) => {
                                status_dot(ui, pal().ok, true);
                                ui.label(egui::RichText::new(tr("A protonfix exists for this ID")).color(pal().ok).small());
                                if ui.small_button(tr("View")).clicked() {
                                    acts.open = Some(format!("{FIXES_REPO_URL}{path}"));
                                }
                            }
                            Some(None) => {
                                status_dot(ui, pal().faint, false);
                                ui.label(egui::RichText::new(tr("No protonfix for this ID")).color(pal().subtext).small());
                            }
                            None => {}
                        }
                        match entry {
                            Some(e) if &e.umu_id != gameid => {
                                ui.label(
                                    egui::RichText::new(trf("· the database lists it as {}", &[&e.umu_id]))
                                        .color(pal().accent)
                                        .small(),
                                );
                                if ui.small_button(tr("Use this ID")).clicked() {
                                    acts.game.push((id.clone(), GameAct::SetUmuId(e.umu_id.clone())));
                                }
                            }
                            Some(_) => {
                                ui.label(egui::RichText::new(tr("· matches the umu database")).color(pal().subtext).small());
                            }
                            None if db_state.as_ref().is_some_and(|d| d.is_ok()) => {
                                ui.label(egui::RichText::new(tr("· not in the umu database")).color(pal().faint).small());
                            }
                            None => {}
                        }
                    });
                });
                ui.add_space(6.0);
            }
            note(ui, 0.0, "Fixes only run with Proton builds that include protonfixes (GE-Proton, UMU-Proton, CachyOS).");
        });
        if reload {
            self.load_fix_data(ctx, true);
        }
    }

    /// Delete-confirmation dialog for an installed Proton build.
    pub fn proton_delete_modal(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.proton.confirm_delete.clone() else { return };
        // Only builds in our own folders: system ones belong to the package manager.
        if !deletable(&dir) {
            self.proton.confirm_delete = None;
            self.set_status(lg("That Proton build was installed by your system's package manager — remove it from there").into(), true);
            return;
        }
        let users: Vec<String> = self
            .cfg
            .custom_games
            .iter()
            .filter(|g| Path::new(&g.proton_dir) == dir)
            .map(|g| disp(&g.name))
            .collect();
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (mut yes, mut no) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("proton-delete")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(440.0);
            ui.label(egui::RichText::new(trf("Delete {}?", &[&name])).size(18.0).strong().color(pal().text));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tr("The folder is removed from disk. Prefixes and saves are not touched.")).color(pal().subtext));
            if !users.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(trf("Used by: {} — they will need another Proton build.", &[&users.join(", ")]))
                        .color(pal().warn),
                );
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if danger_button(ui, "Delete").clicked() {
                    yes = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    no = true;
                }
            });
        }));
        if yes {
            self.proton.confirm_delete = None;
            match fs::remove_dir_all(&dir) {
                Ok(()) => {
                    self.refresh();
                    self.set_status(trf("Deleted {}", &[&name]), false);
                }
                Err(e) => self.set_status(format!("{name}: {e}"), true),
            }
        } else if no || resp.should_close() {
            self.proton.confirm_delete = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_right_archive() {
        let a = |n: &str| Asset { name: n.into(), url: String::new(), size: 1 };
        let ge = [a("GE-Proton11-9-aarch64.tar.gz"), a("GE-Proton11-9-x86_64.sha512sum"), a("GE-Proton11-9-x86_64.tar.gz")];
        assert_eq!(pick_asset("ge", &ge).unwrap().name, "GE-Proton11-9-x86_64.tar.gz");
        assert_eq!(checksum_for(&ge[2], &ge).unwrap().name, "GE-Proton11-9-x86_64.sha512sum");
        let em = [a("proton-EM-10.0-37-HDR.sha256sum"), a("proton-EM-10.0-37-HDR.tar.xz")];
        assert_eq!(pick_asset("em", &em).unwrap().name, "proton-EM-10.0-37-HDR.tar.xz");
        let cachy = [
            a("proton-cachyos-11.0-slr-arm64.tar.xz"),
            a("proton-cachyos-11.0-slr-x86_64.tar.xz"),
            a("proton-cachyos-11.0-slr-x86_64_v3.tar.xz"),
        ];
        assert!(pick_asset("cachyos", &cachy).unwrap().name.contains("x86_64"));
    }

    /// Downloads, verifies and unpacks a real (smallest) build into a temp dir.
    /// cargo test -- --ignored install_live
    #[test]
    #[ignore]
    fn install_live() {
        let rels = fetch_releases("Open-Wine-Components/umu-proton", true).unwrap();
        let rel = &rels[0];
        let asset = pick_asset("umu", &rel.assets).unwrap().clone();
        let sum = checksum_for(&asset, &rel.assets).cloned();
        assert!(sum.is_some(), "no checksum published");
        let root = std::env::temp_dir().join(format!("plaunch-proton-{}", std::process::id()));
        let msg = install_release(root.clone(), asset, sum).unwrap();
        eprintln!("{msg}");
        let tools: Vec<_> = fs::read_dir(&root).unwrap().flatten().map(|e| e.path()).collect();
        assert!(tools.iter().any(|t| t.join("proton").is_file()), "{tools:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn umu_database_lookup() {
        let csv = "TITLE,STORE,CODENAME,UMU_ID,A,N,E\nBroforce,amazon,x,umu-274190,,,\n\"Hades, II\",none,none,umu-1145350,,,\nBroforce,none,none,umu-274190,,,\n";
        let db = parse_umu_db(csv);
        assert_eq!(db.len(), 3);
        assert_eq!(db[1].title, "Hades, II");
        let e = lookup_umu(&db, "BROFORCE").unwrap();
        assert_eq!((e.store.as_str(), e.umu_id.as_str()), ("none", "umu-274190"));
        assert!(lookup_umu(&db, "Valheim").is_none());
        let fixes: HashSet<String> = ["steam/367520".to_string(), "umu/umu-274190".to_string()].into();
        assert_eq!(fix_for(&fixes, "umu-274190").as_deref(), Some("gamefixes-umu/umu-274190.py"));
        assert_eq!(fix_for(&fixes, "367520").as_deref(), Some("gamefixes-steam/367520.py"));
        assert!(fix_for(&fixes, "umu-1").is_none());
        let tree = r#"{"tree":[{"path":"gamefixes-steam/10220.py"},{"path":"gamefixes-umu/__init__.py"},{"path":"verbs/x.py"}]}"#;
        assert_eq!(parse_fix_tree(tree).unwrap(), ["steam/10220".to_string()].into());
    }
}
