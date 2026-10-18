//! Game page extras: the tab bar, the last-session card with a log viewer
//! that points at likely causes, and Windows components via winetricks.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A log line, and what it might mean.
pub struct LogLine {
    pub text: String,
    pub level: u8, // 0 plain, 1 warning, 2 error
}

pub struct LogView {
    pub title: String,
    pub path: PathBuf,
    pub lines: Vec<LogLine>,
    pub hints: Vec<String>,
    pub errors_only: bool,
}

#[derive(Default)]
pub struct ExtrasState {
    pub log: Option<LogView>,
    pub verbs: HashSet<String>,
    pub custom_verb: String,
    pub disk: DiskUse,
}

/// Disk use per game: (game folder, prefix) bytes, None while measuring.
pub type DiskUse = Arc<Mutex<HashMap<String, Option<(u64, u64)>>>>;

/// Known problems, recognised from log lines: (needle, logical hint).
const HINTS: &[(&str, &str)] = &[
    ("err:module:import_dll", "A Windows library the game needs is missing — try Visual C++ or DirectX under Mods & fixes › Windows components."),
    ("vcruntime140", "The Visual C++ runtime seems to be missing — install vcrun2022 under Mods & fixes › Windows components."),
    ("d3dx9_", "Old DirectX 9 files are missing — install d3dx9 under Mods & fixes › Windows components."),
    ("mscoree", ".NET is missing — install dotnet48 under Mods & fixes › Windows components."),
    ("VK_ERROR_DEVICE_LOST", "The GPU driver reset (Vulkan device lost) — try another Proton build or update your graphics driver."),
    ("vkCreateInstance failed", "Vulkan didn't start — check that your graphics driver's Vulkan support is installed."),
    ("err:openxr:xrCreateInstance Exception", "Proton's VR (OpenXR) support crashed as the game started — turn off Settings › Appearance › VR."),
    ("Unhandled page fault", "The game crashed inside Wine — try another Proton build, or remove OptiScaler if it's installed."),
    ("Unhandled exception", "The game crashed inside Wine — try another Proton build, or remove OptiScaler if it's installed."),
    ("EasyAntiCheat", "The game uses Easy Anti-Cheat, which only works on Linux if the developer allows it."),
    ("BattlEye", "The game uses BattlEye, which only works on Linux if the developer allows it."),
    ("No such file or directory", "A file the launch needs wasn't found — check the executable and Proton paths."),
];

pub fn read_log(path: &Path) -> (Vec<LogLine>, Vec<String>) {
    let text = fs::read(path).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    let tail = &all[all.len().saturating_sub(2000)..];
    let mut hints: Vec<String> = Vec::new();
    let lines = tail
        .iter()
        .map(|l| {
            let low = l.to_lowercase();
            let level = if low.contains("err:") || low.contains("error") || low.contains("unhandled") || low.contains("fatal") {
                2
            } else if low.contains("warn") {
                1
            } else {
                0
            };
            for (needle, hint) in HINTS {
                if low.contains(&needle.to_lowercase()) && !hints.iter().any(|h| h == hint) {
                    hints.push((*hint).to_string());
                }
            }
            LogLine { text: l.chars().take(400).collect(), level }
        })
        .collect();
    (lines, hints)
}

/// Common winetricks verbs: (verb, what it is).
pub const VERBS: &[(&str, &str)] = &[
    ("vcrun2022", "Visual C++ 2015–2022"),
    ("vcrun2013", "Visual C++ 2013"),
    ("vcrun2010", "Visual C++ 2010"),
    ("d3dx9", "DirectX 9 (d3dx9)"),
    ("d3dcompiler_47", "D3D shader compiler"),
    ("dotnet48", ".NET Framework 4.8"),
    ("xact", "XACT audio"),
    ("physx", "PhysX"),
    ("corefonts", "Microsoft core fonts"),
];

/// Verbs already installed in a prefix (from its winetricks.log).
pub fn installed_verbs(prefix: &Path) -> HashSet<String> {
    fs::read_to_string(prefix.join("winetricks.log"))
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// Run `umu-run winetricks -q <verbs>` in the game's prefix and wait for it.
pub fn run_winetricks(g: &CustomGame, settings: &Settings, verbs: &[String]) -> Result<String, String> {
    let umu = settings.umu_bin().ok_or_else(|| lg("umu-run not found (install umu-launcher)").to_string())?;
    let mut envs = Vec::new();
    umu_env(g, settings, &mut envs)?;
    ensure_prefix(g, settings)?;
    let log_dir = data_dir().join("logs");
    fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let log_path = log_dir.join(format!("{}-winetricks.log", slugify(&g.name)));
    let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(umu);
    cmd.arg("winetricks").arg("-q").args(verbs).stdin(Stdio::null()).stdout(log).stderr(err);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(lfmt("Installed {} into {}'s prefix", &[&verbs.join(", "), &g.name]))
    } else {
        Err(lfmt("winetricks failed — see {}", &[&log_path.display()]))
    }
}

impl App {
    /// The tab strip under the game banner, with an underline that slides
    /// between tabs. Returns the tab to show.
    pub fn game_tab_bar(&mut self, ui: &mut egui::Ui, windows: bool) -> GameTab {
        let mut tabs = vec![(GameTab::Overview, "Overview"), (GameTab::Settings, "Settings")];
        if windows {
            tabs.push((GameTab::Mods, "Mods & fixes"));
        } else if self.game_tab == GameTab::Mods {
            self.game_tab = GameTab::Overview;
        }
        // [ and ] (a controller's LB/RB) step through the tabs.
        let ctx = ui.ctx().clone();
        if ctx.memory(|m| m.focused().is_none()) && !overlay_open(&ctx) {
            let (prev, next) = ctx.input(|i| (i.key_pressed(egui::Key::OpenBracket), i.key_pressed(egui::Key::CloseBracket)));
            if prev != next {
                let n = tabs.len();
                let cur = tabs.iter().position(|(t, _)| *t == self.game_tab).unwrap_or(0);
                // The right bumper moves right on screen: backwards in Arabic.
                let step = if next != rtl() { 1 } else { n - 1 };
                self.game_tab = tabs[(cur + step) % n].0;
            }
        }
        let mut current: Option<egui::Rect> = None;
        let line_y = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 22.0;
                for (tab, label) in &tabs {
                    let on = self.game_tab == *tab;
                    let text = egui::RichText::new(tr(label)).size(15.5).color(if on { pal().text } else { pal().subtext });
                    let r = ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if r.clicked() {
                        self.game_tab = *tab;
                    }
                    if self.game_tab == *tab {
                        current = Some(r.rect);
                    }
                }
            })
            .response
            .rect
            .bottom()
            + 6.0;
        let full = ui.max_rect();
        ui.painter().line_segment(
            [egui::pos2(full.left(), line_y), egui::pos2(full.right(), line_y)],
            egui::Stroke::new(1.0, pal().border),
        );
        if let Some(r) = current {
            let (now, secs) = (self.now, self.secs(0.22));
            let tw = self.motion.tab_x.get_or_insert((Tween::at(r.left(), now), Tween::at(r.width(), now)));
            let x = tw.0.follow(r.left(), now, secs);
            let w = tw.1.follow(r.width(), now, secs);
            self.motion.busy |= !(tw.0.settled(now, secs) && tw.1.settled(now, secs));
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, line_y - 1.5), egui::vec2(w, 3.0)),
                rad(2),
                pal().accent,
            );
        }
        ui.add_space(4.0);
        self.game_tab
    }

    /// The most recent session: how long, how it ended, and its log.
    pub fn last_session_card(&mut self, ui: &mut egui::Ui, i: usize, stat: &GameStats, acts: &mut Acts) {
        let g = self.cfg.custom_games[i].clone();
        let log = data_dir().join("logs").join(format!("{}.log", slugify(&g.name)));
        let has_log = log.is_file();
        let mut open_viewer = false;
        section(ui, "Last session", |ui| {
            if stat.last_played == 0 {
                ui.label(egui::RichText::new(tr("Not played yet.")).color(pal().subtext));
                return;
            }
            let crashed = stat.last_exit != Some(0) && stat.last_session_secs < 30;
            ui.horizontal(|ui| {
                status_dot(ui, if crashed { pal().warn } else { pal().ok }, true);
                let when = format_ago(stat.last_played, unix_now());
                let ended = match stat.last_exit {
                    Some(0) => lg("closed normally").to_string(),
                    Some(c) => lfmt("exit code {}", &[&c]),
                    None => lg("stopped").to_string(),
                };
                ui.label(
                    egui::RichText::new(disp(&format!("{when} · {} · {ended}", format_duration(stat.last_session_secs))))
                        .color(if crashed { pal().warn } else { pal().subtext }),
                );
                ui.with_layout(far_end(), |ui| {
                    if has_log && ui.button(tr("View log")).clicked() {
                        open_viewer = true;
                    }
                });
            });
            if crashed {
                ui.add_space(4.0);
                note(ui, 0.0, "It closed within seconds — the log usually says why.");
            }
        });
        if open_viewer {
            let (lines, hints) = read_log(&log);
            self.extras.log = Some(LogView { title: g.name.clone(), path: log, lines, hints, errors_only: false });
        }
        let _ = acts;
    }

    /// Windows components (winetricks verbs) for a game's prefix.
    pub fn components_card(&mut self, ui: &mut egui::Ui, i: usize) {
        let g = self.cfg.custom_games[i].clone();
        if g.kind != GameKind::Windows {
            return;
        }
        let prefix = game_prefix(&g, &self.cfg.settings);
        let installed = installed_verbs(&prefix);
        let key = format!("winetricks:{}", g.id);
        let busy = self.tasks.iter().any(|t| t.key == key);
        let mut run: Option<Vec<String>> = None;
        section_sub(ui, "Windows components", "Runtimes some games need (installed with winetricks into this game's prefix)", |ui| {
            ui.horizontal_wrapped(|ui| {
                for (verb, label) in VERBS {
                    let done = installed.contains(*verb);
                    let on = self.extras.verbs.contains(*verb);
                    let text = if done { format!("{label}  ✓") } else { label.to_string() };
                    let r = choice_chip(ui, on || done, &text).on_hover_text(*verb);
                    if r.clicked() && !done {
                        if on {
                            self.extras.verbs.remove(*verb);
                        } else {
                            self.extras.verbs.insert(verb.to_string());
                        }
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let w = 220.0;
                text_field(ui, &mut self.extras.custom_verb, "other verb, e.g. mfc140", w);
                let mut chosen: Vec<String> = self.extras.verbs.iter().cloned().collect();
                chosen.extend(self.extras.custom_verb.split_whitespace().map(String::from));
                chosen.sort();
                ui.add_enabled_ui(!busy && !chosen.is_empty(), |ui| {
                    if primary_button(ui, "Install selected").clicked() {
                        run = Some(chosen);
                    }
                });
                if busy {
                    ui.spinner();
                    ui.label(egui::RichText::new(tr("Installing — this can take a few minutes…")).color(pal().subtext).small());
                }
            });
            note(ui, 0.0, "Only add what a game asks for: most games run without any of these.");
        });
        if let Some(verbs) = run {
            self.extras.verbs.clear();
            self.extras.custom_verb.clear();
            let settings = self.cfg.settings.clone();
            let label = lfmt("Installing {}", &[&verbs.join(", ")]);
            self.start_task(TaskKind::Components, &key, &label, None, move || run_winetricks(&g, &settings, &verbs));
        }
    }

    /// The in-app log viewer.
    pub fn log_viewer(&mut self, ctx: &egui::Context) {
        let Some(mut view) = self.extras.log.take() else { return };
        let mut close = false;
        let mut open_file = false;
        let resp = egui::Modal::new(egui::Id::unique("log-viewer")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 120.0).clamp(420.0, 1000.0));
            ui.label(egui::RichText::new(trf("Log — {}", &[&view.title])).size(18.0).strong().color(pal().text));
            ui.label(egui::RichText::new(view.path.display().to_string()).color(pal().subtext).small().monospace());
            ui.add_space(8.0);
            if !view.hints.is_empty() {
                egui::Frame::new()
                    .fill(pal().warn.gamma_multiply(0.12))
                    .stroke(egui::Stroke::new(1.0, pal().warn.gamma_multiply(0.5)))
                    .corner_radius(rad(10))
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(egui::RichText::new(tr("What this might mean")).strong().color(pal().warn));
                        for h in &view.hints {
                            para(ui, &format!("• {}", lg(h)), pal().text, false);
                        }
                    });
                ui.add_space(8.0);
            }
            ui.horizontal(|ui| {
                check(ui, &mut view.errors_only, tr("Errors only"));
                let errors = view.lines.iter().filter(|l| l.level == 2).count();
                ui.label(egui::RichText::new(trf("{} error lines", &[&errors])).color(pal().subtext).small());
            });
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(pal().field)
                .stroke(egui::Stroke::new(1.0, pal().border))
                .corner_radius(rad(8))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    egui::ScrollArea::both().auto_shrink([false, true]).max_height((ctx.content_rect().height() - 360.0).max(200.0)).stick_to_bottom(true).show(ui, |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            for l in view.lines.iter().filter(|l| !view.errors_only || l.level == 2) {
                                let color = match l.level {
                                    2 => pal().danger,
                                    1 => pal().warn,
                                    _ => pal().subtext,
                                };
                                ui.add(egui::Label::new(egui::RichText::new(&l.text).monospace().small().color(color)).extend());
                            }
                        });
                    });
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Open in editor")).clicked() {
                    open_file = true;
                }
                if ui.button(tr("Close")).clicked() {
                    close = true;
                }
            });
        });
        if open_file {
            self.children.extend(xdg_open(&view.path));
        }
        if !(close || resp.should_close()) {
            self.extras.log = Some(view);
        }
    }
}


// ---------------------------------------------------------------------------
// Scanning a folder for games
// ---------------------------------------------------------------------------

/// Executables that are never the game itself.
const NOT_GAMES: &[&str] = &[
    "unins", "setup", "install", "redist", "vcredist", "directx", "dxsetup", "dxwebsetup", "crash", "report",
    "helper", "prereq", "dotnet", "easyanticheat", "eac_", "battleye", "cefprocess", "quicksfv", "update",
    "patch", "register", "touchup", "notification", "overlay", "uploader", "language selector", "config",
    "settings", "benchmark", "7z", "python", "unitycrashhandler", "dxdiag", "vc_redist", "oalinst", "physx",
    "ue4prereq", "uninstall", "server", "editor", "sdk",
];

/// A game folder found by a scan.
pub struct Found {
    pub include: bool,
    pub name: String,
    /// Possible executables, best first.
    pub exes: Vec<PathBuf>,
    pub pick: usize,
}

fn candidates(dir: &Path, max_depth: usize) -> Vec<(PathBuf, usize, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_lowercase();
            if ft.is_dir() {
                let skip = ["_redist", "redist", "directx", "uninstall", "_commonredist", "support", "engine", "__installer", "md5"];
                if depth < max_depth && !skip.contains(&name.as_str()) {
                    stack.push((p, depth + 1));
                }
            } else if name.ends_with(".exe") && !NOT_GAMES.iter().any(|n| name.contains(n)) {
                let size = e.metadata().map_or(0, |m| m.len());
                out.push((p, depth, size));
            }
        }
    }
    out
}

/// The likely main executable of a game folder, best first: a name like the
/// folder's wins, then being near the top, then size.
pub fn rank_exes(folder: &Path, found: Vec<(PathBuf, usize, u64)>) -> Vec<PathBuf> {
    let folder_words: Vec<String> = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(String::from)
        .collect();
    let mut scored: Vec<(f64, PathBuf)> = found
        .into_iter()
        .map(|(p, depth, size)| {
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let matches = folder_words.iter().filter(|w| stem.contains(w.as_str())).count() as f64;
            let score = matches * 100.0 + (5.0 - depth as f64) * 2.0 + ((size as f64) / 1e6 + 1.0).ln();
            (score, p)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, p)| p).collect()
}

/// Every game folder under `root` (and `root` itself if it holds a game),
/// leaving out games already in the library.
pub fn scan_folder(root: &Path, known: &[PathBuf]) -> Vec<Found> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    let mut out = Vec::new();
    let mut consider = |dir: &Path, depth: usize| {
        if known.iter().any(|k| k.starts_with(dir)) {
            return;
        }
        let exes = rank_exes(dir, candidates(dir, depth));
        if let Some(best) = exes.first() {
            out.push(Found { include: true, name: guess_game_name(best), exes, pick: 0 });
        }
    };
    if fs::read_dir(root).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".exe"))).unwrap_or(false) {
        consider(root, 1);
    } else {
        for d in &dirs {
            consider(d, 4);
        }
    }
    out
}

pub struct ScanState {
    pub results: Option<Vec<Found>>,
    pub scanning: bool,
    pub root: PathBuf,
    pub tx: std::sync::mpsc::Sender<Vec<Found>>,
    pub rx: std::sync::mpsc::Receiver<Vec<Found>>,
}

impl Default for ScanState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        ScanState { results: None, scanning: false, root: PathBuf::new(), tx, rx }
    }
}

impl App {
    pub fn start_scan(&mut self, ctx: &egui::Context, root: PathBuf) {
        let known: Vec<PathBuf> = self.cfg.custom_games.iter().filter_map(|g| Path::new(&g.exe).parent().map(Path::to_path_buf)).collect();
        self.scan.scanning = true;
        self.scan.root = root.clone();
        self.set_status(lfmt("Looking for games in {}…", &[&root.display()]), false);
        let (tx, ctx) = (self.scan.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(scan_folder(&root, &known));
            ctx.request_repaint();
        });
    }

    pub fn scan_poll(&mut self) {
        while let Ok(r) = self.scan.rx.try_recv() {
            self.scan.scanning = false;
            if r.is_empty() {
                self.set_status(lg("No new games found there").into(), false);
            } else {
                self.set_status(lfmt("Found {} games", &[&r.len()]), false);
                self.scan.results = Some(r);
            }
        }
    }

    /// The checklist of found games. Returns what to add once confirmed.
    pub fn scan_modal(&mut self, ctx: &egui::Context) -> Option<Vec<AddModal>> {
        let mut results = self.scan.results.take()?;
        let (mut add, mut cancel) = (false, false);
        let root = self.scan.root.clone();
        let resp = egui::Modal::new(egui::Id::unique("scan-results")).show(ctx, |ui| {
            dir_ui(ui, |ui| {
                ui.set_width(640.0);
                ui.label(egui::RichText::new(tr("Games found")).size(18.0).strong().color(pal().text));
                ui.label(egui::RichText::new(root.display().to_string()).color(pal().subtext).small().monospace());
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 280.0).max(200.0)).show(ui, |ui| {
                    for (n, f) in results.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::Checkbox::without_text(&mut f.include));
                            ui.add_enabled_ui(f.include, |ui| {
                                ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(220.0));
                                let rel = |p: &PathBuf| p.strip_prefix(&root).unwrap_or(p).display().to_string();
                                let picked = rel(&f.exes[f.pick]);
                                egui::ComboBox::from_id_salt(("scan-exe", n))
                                    .selected_text(short_path(&picked))
                                    .width(330.0)
                                    .truncate()
                                    .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                        for (k, p) in f.exes.iter().enumerate() {
                                            if ui.selectable_label(f.pick == k, rel(p)).clicked() {
                                                f.pick = k;
                                            }
                                        }
                                    }))
                                    .response
                                    .on_hover_text(picked);
                            });
                        });
                    }
                });
                ui.add_space(10.0);
                let n = results.iter().filter(|f| f.include && !f.name.trim().is_empty()).count();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(n > 0, |ui| {
                        let label = if n == 1 { tr("Add 1 game") } else { trf("Add {} games", &[&n]) };
                        if primary_button(ui, &label).clicked() {
                            add = true;
                        }
                    });
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
                note(ui, 0.0, "Pick a different executable from the list if the guess is wrong. Each game gets your default Proton and settings.");
            })
        });
        if add {
            return Some(
                results
                    .into_iter()
                    .filter(|f| f.include && !f.name.trim().is_empty())
                    .map(|f| {
                        let mut m = self.new_add_modal(f.exes[f.pick].clone());
                        m.name = f.name.trim().to_string();
                        m
                    })
                    .collect(),
            );
        }
        if !(cancel || resp.should_close()) {
            self.scan.results = Some(results);
        }
        None
    }
}


// ---------------------------------------------------------------------------
// Importing non-Steam games from Steam (with their prefixes and artwork)
// ---------------------------------------------------------------------------

/// A non-Steam game added to Steam, as found in its shortcuts.vdf.
#[derive(Clone, Debug, Default)]
pub struct SteamShortcut {
    pub include: bool,
    pub name: String,
    pub exe: PathBuf,
    pub start_dir: String,
    pub args: String,
    pub env: String,
    pub mangohud: bool,
    pub gamemode: bool,
    /// Steam's Proton prefix for it (compatdata/<id>/pfx), where its saves are.
    pub prefix: Option<PathBuf>,
    pub keep_prefix: bool,
    pub cover: Option<PathBuf>,
    pub hero: Option<PathBuf>,
    pub icon: Option<PathBuf>,
    /// The Proton build Steam used (its folder name).
    pub proton: Option<String>,
}

/// Split Steam launch options ("mangohud VAR=1 %command% -dx11") into
/// (args after %command%, env lines, mangohud, gamemode).
pub fn split_launch_options(opts: &str) -> (String, String, bool, bool) {
    let (before, after) = match opts.split_once("%command%") {
        Some((b, a)) => (b, a),
        None => ("", opts),
    };
    let (mut env, mut mh, mut gm) = (Vec::new(), false, false);
    for tok in tokenize(before) {
        match tok.as_str() {
            "mangohud" => mh = true,
            "gamemoderun" => gm = true,
            t if t.contains('=') => env.push(t.to_string()),
            _ => {}
        }
    }
    (after.trim().to_string(), env.join("\n"), mh, gm)
}

/// Strings and ints of a binary shortcuts.vdf, entry by entry.
pub fn parse_shortcuts(data: &[u8]) -> Vec<Vec<(String, String)>> {
    let mut entries: Vec<Vec<(String, String)>> = Vec::new();
    let mut i = 0;
    let read_cstr = |from: usize| -> (String, usize) {
        let end = data[from..].iter().position(|&c| c == 0).map_or(data.len(), |e| from + e);
        (String::from_utf8_lossy(&data[from..end]).to_string(), end + 1)
    };
    while i < data.len() {
        let t = data[i];
        if t == 0x01 || t == 0x02 {
            let (key, after_key) = read_cstr(i + 1);
            let valid = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                i += 1;
                continue;
            }
            if t == 0x01 {
                let (val, next) = read_cstr(after_key);
                if let Some(e) = entries.last_mut() {
                    e.push((key.to_lowercase(), val));
                }
                i = next;
            } else {
                if after_key + 4 > data.len() {
                    break;
                }
                let b = &data[after_key..after_key + 4];
                let v = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if key.eq_ignore_ascii_case("appid") {
                    entries.push(Vec::new());
                }
                if let Some(e) = entries.last_mut() {
                    e.push((key.to_lowercase(), v.to_string()));
                }
                i = after_key + 4;
            }
        } else {
            i += 1;
        }
    }
    entries
}

/// Steam's per-game Proton choices (CompatToolMapping in config.vdf).
fn steam_compat_tools(steam: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(text) = fs::read_to_string(steam.join("config/config.vdf")) else { return out };
    let Some(start) = text.find("\"CompatToolMapping\"") else { return out };
    let mut current: Option<String> = None;
    let mut depth = 0;
    for line in text[start..].lines().skip(1) {
        let t = line.trim();
        match t {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                current = None;
            }
            _ => {
                let parts: Vec<&str> = t.split('"').filter(|p| !p.trim().is_empty()).collect();
                if depth == 1 && parts.len() == 1 {
                    current = Some(parts[0].to_string());
                } else if depth == 2 && parts.len() == 2 && parts[0] == "name"
                    && let Some(id) = &current
                {
                    out.insert(id.clone(), parts[1].to_string());
                }
            }
        }
    }
    out
}

/// Every non-Steam game in Steam whose executable still exists.
pub fn steam_shortcuts(known: &[PathBuf]) -> Vec<SteamShortcut> {
    let Some(steam) = find_steam_root() else { return Vec::new() };
    let tools = steam_compat_tools(&steam);
    let Ok(users) = fs::read_dir(steam.join("userdata")) else { return Vec::new() };
    let mut out = Vec::new();
    for u in users.flatten() {
        let cfg = u.path().join("config");
        let Ok(data) = fs::read(cfg.join("shortcuts.vdf")) else { continue };
        for e in parse_shortcuts(&data) {
            let get = |k: &str| e.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
            let exe = PathBuf::from(get("exe").trim_matches('"'));
            if !exe.is_file() || known.contains(&exe) {
                continue;
            }
            let appid = get("appid");
            let prefix = steam.join("steamapps/compatdata").join(&appid).join("pfx");
            let art = |suffix: &str| {
                ["png", "jpg"].iter().map(|x| cfg.join("grid").join(format!("{appid}{suffix}.{x}"))).find(|p| p.is_file())
            };
            let (args, env, mangohud, gamemode) = split_launch_options(&get("launchoptions"));
            let name = get("appname");
            let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
            let has_prefix = prefix.join("drive_c").is_dir();
            out.push(SteamShortcut {
                include: true,
                name,
                start_dir: get("startdir").trim_matches('"').to_string(),
                args,
                env,
                mangohud,
                gamemode,
                prefix: has_prefix.then_some(prefix),
                keep_prefix: has_prefix,
                cover: art("p"),
                hero: art("_hero"),
                icon: art("_icon"),
                proton: tools.get(&appid).cloned(),
                exe,
            });
        }
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

/// The `game:` section of a Lutris game config: (exe, prefix, args, working dir).
pub fn lutris_game_section(yml: &str) -> (String, String, String, String) {
    let (mut exe, mut prefix, mut args, mut dir) = (String::new(), String::new(), String::new(), String::new());
    let mut in_game = false;
    for line in yml.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            in_game = line.trim_end() == "game:";
            continue;
        }
        if !in_game {
            continue;
        }
        let Some((k, v)) = line.trim().split_once(':') else { continue };
        let v = v.trim().trim_matches(|c| c == '\'' || c == '"').to_string();
        match k {
            "exe" => exe = v,
            "prefix" => prefix = v,
            "args" => args = v,
            "working_dir" => dir = v,
            _ => {}
        }
    }
    (exe, prefix, args, dir)
}

/// Installed Wine and native games from Lutris, as an import checklist.
pub fn lutris_games(known: &[PathBuf]) -> Vec<SteamShortcut> {
    let h = home();
    let share = h.join(".local/share/lutris");
    // Names and config files from Lutris's database (via sqlite3 when present).
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let db = share.join("pga.db");
    if db.is_file()
        && let Ok(out) = Command::new("sqlite3")
            .args(["-separator", "\t"])
            .arg(&db)
            .arg("select name, slug, configpath from games where installed = 1 and runner in ('wine', 'linux')")
            .output()
    {
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            let mut f = l.split('\t');
            if let (Some(n), Some(s), Some(c)) = (f.next(), f.next(), f.next()) {
                rows.push((n.to_string(), s.to_string(), c.to_string()));
            }
        }
    }
    let config_dirs = [h.join(".config/lutris/games"), share.join("games")];
    if rows.is_empty() {
        // No database to read: name each config after its file ("hades-1690000000").
        for d in &config_dirs {
            for e in fs::read_dir(d).into_iter().flatten().flatten() {
                let file = e.file_name().to_string_lossy().trim_end_matches(".yml").to_string();
                let slug = file.rsplit_once('-').filter(|(_, n)| n.chars().all(|c| c.is_ascii_digit())).map_or(file.clone(), |(s, _)| s.to_string());
                let name = slug.split('-').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join(" ");
                rows.push((name, slug, file));
            }
        }
    }
    let art = |dirs: &[&str], slug: &str| {
        dirs.iter().flat_map(|d| ["jpg", "png"].map(|x| share.join(d).join(format!("{slug}.{x}")))).find(|p| p.is_file())
    };
    let mut out = Vec::new();
    for (name, slug, config) in rows {
        let Some(yml) = config_dirs.iter().map(|d| d.join(format!("{config}.yml"))).find_map(|p| fs::read_to_string(p).ok()) else { continue };
        let (exe, prefix, args, dir) = lutris_game_section(&yml);
        let mut exe = PathBuf::from(&exe);
        if exe.is_relative() && !prefix.is_empty() {
            exe = Path::new(&prefix).join(&exe);
        }
        if !exe.is_file() || known.contains(&exe) {
            continue;
        }
        let windows = exe.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"));
        out.push(SteamShortcut {
            include: true,
            name,
            start_dir: dir,
            args,
            env: String::new(),
            mangohud: false,
            gamemode: false,
            prefix: (windows && Path::new(&prefix).is_dir()).then(|| PathBuf::from(&prefix)),
            keep_prefix: true,
            cover: art(&["coverart"], &slug),
            hero: art(&["banners"], &slug),
            icon: None,
            proton: None,
            exe,
        });
    }
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

impl App {
    pub fn open_lutris_import(&mut self) {
        let known: Vec<PathBuf> = self.cfg.custom_games.iter().map(|g| PathBuf::from(&g.exe)).collect();
        let found = lutris_games(&known);
        if found.is_empty() {
            self.set_status(lg("No new games found in Lutris").into(), false);
        } else {
            self.import_lutris = true;
            self.steam_import = Some(found);
        }
    }

    pub fn open_steam_import(&mut self) {
        let known: Vec<PathBuf> = self.cfg.custom_games.iter().map(|g| PathBuf::from(&g.exe)).collect();
        let found = steam_shortcuts(&known);
        if found.is_empty() {
            self.set_status(lg("No new non-Steam games found in Steam").into(), false);
        } else {
            self.import_lutris = false;
            self.steam_import = Some(found);
        }
    }

    /// The Steam import checklist. Returns true when games were added.
    pub fn steam_import_modal(&mut self, ctx: &egui::Context) {
        let Some(mut list) = self.steam_import.take() else { return };
        let (mut add, mut cancel) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("steam-import")).show(ctx, |ui| {
            dir_ui(ui, |ui| {
                ui.set_width(640.0);
                let lutris = self.import_lutris;
                let title = if lutris { "Import from Lutris" } else { "Import from Steam" };
                ui.label(egui::RichText::new(tr(title)).size(18.0).strong().color(pal().text));
                if lutris {
                    para(ui, "Games installed with Lutris. Keeping their prefix means saves and settings come along untouched.", pal().subtext, true);
                } else {
                    para(ui, "Non-Steam games you added to Steam. Keeping Steam's prefix means their saves and settings come along untouched.", pal().subtext, true);
                }
                ui.add_space(8.0);
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 300.0).max(200.0)).show(ui, |ui| {
                    for s in list.iter_mut() {
                        card_inner().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add(egui::Checkbox::without_text(&mut s.include));
                                vcol(ui, |ui| {
                                    ui.label(egui::RichText::new(disp(&s.name)).strong().color(pal().text));
                                    ui.add(egui::Label::new(egui::RichText::new(s.exe.display().to_string()).color(pal().subtext).small().monospace()).truncate());
                                    ui.horizontal_wrapped(|ui| {
                                        if s.prefix.is_some() {
                                            check(ui, &mut s.keep_prefix, tr(if lutris { "Keep its Lutris prefix (saves stay put)" } else { "Keep Steam's prefix (saves stay put)" }));
                                        }
                                        if s.cover.is_some() {
                                            pill(ui, "artwork", pal().accent);
                                        }
                                        if let Some(p) = &s.proton {
                                            pill(ui, p, pal().subtext);
                                        }
                                    });
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
                ui.add_space(10.0);
                let n = list.iter().filter(|s| s.include).count();
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(n > 0, |ui| {
                        let label = if n == 1 { tr("Add 1 game") } else { trf("Add {} games", &[&n]) };
                        if primary_button(ui, &label).clicked() {
                            add = true;
                        }
                    });
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            })
        });
        if add {
            let chosen: Vec<SteamShortcut> = list.into_iter().filter(|s| s.include).collect();
            let n = chosen.len();
            let mut ids = Vec::new();
            for s in chosen {
                let windows = s.exe.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"));
                let m = self.new_add_modal(s.exe.clone());
                // Steam's Proton, if we have the same build; else the usual default.
                let steam_tool = s.proton.as_ref().and_then(|name| {
                    let base = name.trim_end_matches("-x86_64");
                    self.proton_tools.iter().find(|t| t.label == *name || t.label == base)
                });
                let proton_dir = match (windows, steam_tool) {
                    (false, _) => String::new(),
                    (true, Some(t)) => t.dir.to_string_lossy().into(),
                    (true, None) => self.proton_tools.get(m.proton_idx).map(|t| t.dir.to_string_lossy().to_string()).unwrap_or_default(),
                };
                let mut flags = self.cfg.settings.new_game_flags();
                flags.mangohud |= s.mangohud;
                flags.gamemode |= s.gamemode;
                let path = |p: &Option<PathBuf>| p.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let g = CustomGame {
                    id: new_game_id(),
                    umu_id: self.umu_match(&s.name).map(|e| e.umu_id).unwrap_or_default(),
                    name: s.name.clone(),
                    kind: if windows { GameKind::Windows } else { GameKind::Linux },
                    exe: s.exe.to_string_lossy().into(),
                    args: s.args.clone(),
                    workdir: s.start_dir.trim_end_matches('/').to_string(),
                    proton_dir,
                    prefix: if s.keep_prefix { path(&s.prefix) } else { String::new() },
                    flags,
                    icon: path(&s.icon),
                    cover: path(&s.cover),
                    hero: path(&s.hero),
                    art_checked: s.cover.is_some() && s.hero.is_some(),
                    env: s.env.clone(),
                    added: unix_now(),
                    ..Default::default()
                };
                self.motion.fresh.insert(g.id.clone(), None);
                ids.push(g.id.clone());
                self.cfg.custom_games.push(g);
            }
            save_config(&self.cfg);
            self.page = Page::Library;
            if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
                let missing: Vec<String> = ids.into_iter().filter(|id| self.needs_art().contains(id)).collect();
                self.auto_fetch(ctx, missing);
            }
            self.set_status(lfmt("Imported {} games from Steam", &[&n]), false);
        } else if !(cancel || resp.should_close()) {
            self.steam_import = Some(list);
        }
    }
}

/// A long relative path cut down to its last two parts ("…/Bin32/bio4.exe"),
/// so the executable's name stays visible in a narrow box.
pub fn short_path(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    if rel.chars().count() <= 36 || parts.len() <= 2 {
        rel.to_string()
    } else {
        format!("…/{}", parts[parts.len() - 2..].join("/"))
    }
}

/// The folder a game lives in: the exe's folder, stepping out of the usual
/// binary subfolders (Unreal's `Binaries/Win64`, `bin/x64`…). None when that
/// would be the home folder or the root, which aren't the game's own.
pub fn game_root(exe: &str) -> Option<PathBuf> {
    let mut dir = Path::new(exe).parent()?.to_path_buf();
    while let (Some(name), Some(parent)) = (dir.file_name(), dir.parent()) {
        let name = name.to_string_lossy().to_lowercase();
        if !["win64", "win32", "binaries", "bin", "bin64", "bin32", "x64", "x86", "x86_64"].contains(&name.as_str()) {
            break;
        }
        dir = parent.to_path_buf();
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    (dir.parent().is_some() && Some(&dir) != home.as_ref()).then_some(dir)
}

impl App {
    /// "Jump back in": the most recently played game as a wide banner with
    /// its Play button, above the library grid.
    pub fn jump_back_in(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let recent = self
            .cfg
            .custom_games
            .iter()
            .enumerate()
            .filter_map(|(i, g)| self.stats.get(&g.id).filter(|s| s.last_played > 0).map(|s| (i, s.clone())))
            .max_by_key(|(_, s)| s.last_played);
        let Some((i, stat)) = recent else {
            return;
        };
        let g = self.cfg.custom_games[i].clone();
        let id = g.id.clone();
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 136.0), egui::Sense::click());
        let hover = ctx.animate_bool_with_time(egui::Id::unique(("jump-hover", &id)), resp.hovered(), self.secs(0.16));
        let painter = ui.painter().clone();
        match self.game_hero(ctx, i) {
            Some(t) => {
                painter.add(cover_fill_shape_ex(&t, rect, 14, 1.0, 1.0 + 0.04 * hover, 0.35));
            }
            None => {
                painter.rect_filled(rect, rad(14), pal().card);
            }
        }
        // Darken from the reading side so the text holds up over any artwork.
        let (dark, light) = (egui::Color32::from_black_alpha(215), egui::Color32::from_black_alpha(70));
        let (l, r) = if rtl() { (light, dark) } else { (dark, light) };
        let mut mesh = egui::Mesh::default();
        for (pos, c) in [(rect.left_top(), l), (rect.right_top(), r), (rect.right_bottom(), r), (rect.left_bottom(), l)] {
            mesh.colored_vertex(pos, c);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        // The mesh has square corners: clip it to the card, then redraw the
        // corners' edge with the border so nothing pokes out.
        painter.with_clip_rect(rect.shrink(if pal().square { 0.0 } else { 2.0 })).add(mesh);
        let stroke = egui::Stroke::new(1.0 + hover, pal().border.lerp_to_gamma(pal().accent, hover));
        painter.rect_stroke(rect, rad(14), stroke, egui::StrokeKind::Inside);

        let pad = 16.0;
        let ph = rect.height() - 2.0 * pad;
        let pw = ph * 2.0 / 3.0;
        let px = if rtl() { rect.right() - pad - pw } else { rect.left() + pad };
        let poster = egui::Rect::from_min_size(egui::pos2(px, rect.top() + pad), egui::vec2(pw, ph));
        if let Some(t) = self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
            painter.add(cover_fill_shape(&t, poster, 8, 1.0));
        }
        painter.rect_stroke(poster, rad(8), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)), egui::StrokeKind::Inside);

        let (state, phase) = self.play_state(&id);
        let dx = self.shake_dx(&id);
        let button_w = 170.0;
        let text = if rtl() {
            egui::Rect::from_min_max(egui::pos2(rect.left() + button_w, rect.top() + pad), egui::pos2(poster.left() - 18.0, rect.bottom() - pad))
        } else {
            egui::Rect::from_min_max(egui::pos2(poster.right() + 18.0, rect.top() + pad), egui::pos2(rect.right() - button_w, rect.bottom() - pad))
        };
        let align = if rtl() { egui::Align::Max } else { egui::Align::Min };
        let over = theme::OverImage::new();
        // Child uis that don't allocate in the page: the banner already did.
        {
            let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(text).layout(egui::Layout::top_down(align)));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tr("Jump back in")).small().strong().color(pal().accent));
            ui.add_space(2.0);
            ui.add(egui::Label::new(egui::RichText::new(disp(&g.name)).size(22.0).strong().color(pal().text)).truncate());
            ui.add_space(2.0);
            let sub = trf("{} played · last played {}", &[&format_duration(stat.playtime_secs), &format_ago(stat.last_played, unix_now())]);
            ui.label(egui::RichText::new(sub).color(pal().subtext));
        }
        let bx = if rtl() { rect.left() + pad } else { rect.right() - pad - 158.0 };
        let slot = egui::Rect::from_min_size(egui::pos2(bx, rect.center().y - 22.0), egui::vec2(158.0, 44.0));
        let layout = if rtl() { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) };
        let clicked = play_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(layout)), state, phase, dx).clicked();
        drop(over);
        if clicked {
            let act = if self.running.contains_key(&id) { GameAct::Stop } else { GameAct::Play };
            acts.game.push((id.clone(), act));
        } else if resp.clicked() {
            acts.game.push((id.clone(), GameAct::Open));
        }
        let running = self.running.contains_key(&id);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, running, false, &self.cfg.settings.collections, acts)));
        ui.add_space(22.0);
    }

    /// "At a glance": how much you've played and how much room the game takes.
    pub fn glance_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, stat: &GameStats, acts: &mut Acts) {
        let g = self.cfg.custom_games[i].clone();
        let root = game_root(&g.exe);
        let prefix = (g.kind == GameKind::Windows).then(|| game_prefix(&g, &self.cfg.settings));
        let disk = self.disk_use(ctx, &g.id, root.clone(), prefix.clone());
        // Sessions were only counted from this version on: never show fewer than one.
        let sessions = if stat.last_played > 0 { stat.sessions.max(1) } else { 0 };
        section(ui, "At a glance", |ui| {
            ui.horizontal(|ui| {
                let dash = || "—".to_string();
                let played = if stat.playtime_secs > 0 { disp(&format_duration(stat.playtime_secs)) } else { dash() };
                let avg = if stat.sessions > 0 { disp(&format_duration(stat.playtime_secs / u64::from(stat.sessions))) } else { dash() };
                let count = if sessions > 0 { sessions.to_string() } else { dash() };
                let size = match disk {
                    Some((a, b)) => format_bytes(a + b),
                    None => "…".to_string(),
                };
                hltb::stat_block(ui, &tr("Total played"), &played);
                ui.add_space(26.0);
                hltb::stat_block(ui, &tr("Sessions"), &count);
                ui.add_space(26.0);
                hltb::stat_block(ui, &tr("Average session"), &avg);
                ui.add_space(26.0);
                let r = vcol(ui, |ui| hltb::stat_block(ui, &tr("Disk use"), &size)).response;
                if let Some((a, b)) = disk {
                    let mut tip = lfmt("Game folder: {}", &[&format_bytes(a)]);
                    if prefix.is_some() {
                        tip = format!("{tip}\n{}", lfmt("Prefix: {}", &[&format_bytes(b)]));
                    }
                    r.on_hover_text(disp(&tip));
                }
                ui.with_layout(far_end(), |ui| {
                    if prefix.is_some() && ui.button(tr("Prefix")).on_hover_text(tr("Open prefix folder")).clicked() {
                        acts.game.push((g.id.clone(), GameAct::OpenPrefix));
                    }
                    if ui.button(tr("Game folder")).on_hover_text(tr("Open game folder")).clicked() {
                        acts.game.push((g.id.clone(), GameAct::OpenGameFolder));
                    }
                });
            });
        });
    }

    /// Bytes used by a game's folder and prefix, measured once in the background.
    fn disk_use(&mut self, ctx: &egui::Context, id: &str, root: Option<PathBuf>, prefix: Option<PathBuf>) -> Option<(u64, u64)> {
        let mut map = self.extras.disk.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = map.get(id) {
            return *v;
        }
        map.insert(id.to_string(), None);
        let (disk, id, ctx) = (self.extras.disk.clone(), id.to_string(), ctx.clone());
        std::thread::spawn(move || {
            let size = |p: Option<PathBuf>| p.filter(|p| p.is_dir()).map_or(0, |p| dir_size(&p));
            let v = (size(root), size(prefix));
            disk.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Some(v));
            ctx.request_repaint();
        });
        None
    }

    /// Forget a game's measured size (after a session, files may have changed).
    pub fn forget_disk_use(&mut self, id: &str) {
        self.extras.disk.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_keep_the_exe_name() {
        assert_eq!(short_path("Blur/Blur.exe"), "Blur/Blur.exe");
        assert_eq!(short_path("Thronefall-v1-61/Thronefall.v1.61/Thronefall.v1.61/Thronefall.exe"), "…/Thronefall.v1.61/Thronefall.exe");
    }

    #[test]
    fn game_root_steps_out_of_binary_folders() {
        assert_eq!(game_root("/g/Khazan/BBQ/Binaries/Win64/BBQ-Win64-Shipping.exe"), Some(PathBuf::from("/g/Khazan/BBQ")));
        assert_eq!(game_root("/g/Hades/x64/Hades.exe"), Some(PathBuf::from("/g/Hades")));
        assert_eq!(game_root("/g/Celeste/Celeste.exe"), Some(PathBuf::from("/g/Celeste")));
        // A loose exe in the home folder or at the root has no folder of its own.
        let home = std::env::var("HOME").unwrap();
        assert_eq!(game_root(&format!("{home}/game.exe")), None);
        assert_eq!(game_root("/game.exe"), None);
    }

    #[test]
    fn log_lines_get_levels_and_hints() {
        let p = std::env::temp_dir().join(format!("plaunch-log-{}.log", std::process::id()));
        fs::write(&p, "starting\nwarn: slow\n0024:err:module:import_dll Library VCRUNTIME140.dll not found\nwine: Unhandled page fault\n").unwrap();
        let (lines, hints) = read_log(&p);
        assert_eq!(lines.iter().map(|l| l.level).collect::<Vec<_>>(), vec![0, 1, 2, 2]);
        assert_eq!(hints.len(), 3, "{hints:?}");
        let _ = fs::remove_file(&p);
        let dir = std::env::temp_dir().join(format!("plaunch-wt-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("winetricks.log"), "vcrun2022\nd3dx9\n").unwrap();
        assert!(installed_verbs(&dir).contains("d3dx9"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Read-only: what a scan of your games folder would offer.
    /// GAMES=dir cargo test -- --ignored scan_real --nocapture
    #[test]
    #[ignore]
    fn scan_real() {
        let root = PathBuf::from(std::env::var("GAMES").expect("set GAMES"));
        let known: Vec<PathBuf> = load_config().custom_games.iter().filter_map(|g| Path::new(&g.exe).parent().map(Path::to_path_buf)).collect();
        for f in scan_folder(&root, &known) {
            eprintln!("{:40} {}", f.name, f.exes.iter().map(|p| p.strip_prefix(&root).unwrap().display().to_string()).collect::<Vec<_>>().join(" | "));
        }
    }

    /// Read-only: your Steam shortcuts as the import would see them.
    /// cargo test -- --ignored steam_real --nocapture
    #[test]
    #[ignore]
    fn steam_real() {
        let known: Vec<PathBuf> = load_config().custom_games.iter().map(|g| PathBuf::from(&g.exe)).collect();
        for s in steam_shortcuts(&known) {
            eprintln!("{:28} prefix={} cover={} proton={:?} args={:?} env={:?} mh={}", s.name, s.prefix.is_some(), s.cover.is_some(), s.proton, s.args, s.env, s.mangohud);
        }
    }

    #[test]
    fn lutris_configs_are_read() {
        let yml = "game:\n  exe: /games/Hades/Hades.exe\n  prefix: '/home/me/Games/hades'\n  args: -dx11\nsystem:\n  exe: /wrong\nwine:\n  version: lutris-GE\n";
        let (exe, prefix, args, dir) = lutris_game_section(yml);
        assert_eq!(exe, "/games/Hades/Hades.exe");
        assert_eq!(prefix, "/home/me/Games/hades");
        assert_eq!(args, "-dx11");
        assert_eq!(dir, "");
    }

    #[test]
    fn steam_shortcuts_are_read() {
        // Two entries as Steam writes them.
        let mut d = Vec::new();
        d.extend(b"\x00shortcuts\x00\x000\x00");
        let entry = |d: &mut Vec<u8>, id: u32, name: &str, exe: &str, opts: &str| {
            d.extend(b"\x02appid\x00");
            d.extend(id.to_le_bytes());
            for (k, v) in [("AppName", name), ("Exe", exe), ("StartDir", "/g/"), ("LaunchOptions", opts)] {
                d.push(1);
                d.extend(k.as_bytes());
                d.push(0);
                d.extend(v.as_bytes());
                d.push(0);
            }
            d.extend(b"\x02IsHidden\x00\x00\x00\x00\x00\x08\x08");
        };
        entry(&mut d, 2996523584, "Lou's Lagoon.exe", "\"/g/Lou's Lagoon.exe\"", "mangohud %command%");
        entry(&mut d, 7, "Other", "/g/o.exe", "");
        let e = parse_shortcuts(&d);
        assert_eq!(e.len(), 2);
        let get = |i: usize, k: &str| e[i].iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(get(0, "appid"), "2996523584");
        assert_eq!(get(0, "exe"), "\"/g/Lou's Lagoon.exe\"");
        assert_eq!(get(1, "appname"), "Other");
        assert_eq!(
            split_launch_options("mangohud DXVK_HUD=1 gamemoderun %command% -dx11 -skipintro"),
            ("-dx11 -skipintro".to_string(), "DXVK_HUD=1".to_string(), true, true)
        );
        assert_eq!(split_launch_options("-windowed").0, "-windowed");
    }

    #[test]
    fn scanning_finds_the_main_executables() {
        let root = std::env::temp_dir().join(format!("plaunch-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let file = |p: &str, size: usize| {
            let p = root.join(p);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![0u8; size]).unwrap();
        };
        file("Blur/Blur.exe", 3000);
        file("Blur/Uninstall/unins000.exe", 10);
        file("RE4/Bin32/bio4.exe", 5000);
        file("RE4/Bin32/Language Selector.exe", 100);
        file("RE4/_Redist/dxwebsetup.exe", 100);
        file("Repack/setup.exe", 100);
        file("Known/game.exe", 100);
        let found = scan_folder(&root, &[root.join("Known")]);
        let picks: Vec<(String, String)> = found
            .iter()
            .map(|f| (f.name.clone(), f.exes[0].file_name().unwrap().to_string_lossy().to_string()))
            .collect();
        assert_eq!(picks, vec![("Blur".to_string(), "Blur.exe".to_string()), ("RE4".to_string(), "bio4.exe".to_string())]);
        let _ = fs::remove_dir_all(&root);
    }
}
