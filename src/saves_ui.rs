//! The Save Vault page and the game page's saves summary.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::saves::{self, Found, GameSaves, MGame, Origin, SetInfo, SyncState};
use crate::theme::pal;
use crate::*;
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

pub enum Msg {
    Index(Option<Vec<MGame>>),
    Found(String, Vec<Found>),
}

/// One location as shown on the page.
pub struct LocRow {
    pub pattern: String,
    pub origin: Origin,
    pub ignored: bool,
    pub matches: Vec<PathBuf>,
    pub size: u64,
    pub newest: u64,
}

/// Everything the page shows for one game, computed when it opens or after a change.
pub struct View {
    pub id: String,
    pub gs: GameSaves,
    pub rows: Vec<LocRow>,
    pub backups: Vec<(PathBuf, SetInfo)>,
    pub sync: SyncState,
}

pub struct SavesState {
    pub index: Option<Arc<Vec<MGame>>>,
    pub index_loading: bool,
    pub view: Option<View>,
    pub found: HashMap<String, Vec<Found>>,
    pub scanning: Option<String>,
    /// Game page summaries: (locations with saves here, size, last backup time).
    pub summary: HashMap<String, (usize, u64, u64)>,
    pub search: Option<String>,
    /// An import waiting for the user to pick its location.
    pub import_src: Option<PathBuf>,
    pub import_target: Option<String>,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for SavesState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        SavesState {
            index: None,
            index_loading: false,
            view: None,
            found: HashMap::new(),
            scanning: None,
            summary: HashMap::new(),
            search: None,
            import_src: None,
            import_target: None,
            tx,
            rx,
        }
    }
}

/// A local date and time: "2026-10-18 14:02".
pub fn format_time(ts: u64) -> String {
    let t = ts as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// A pattern as people read it: "AppData/LocalLow/Team Cherry/Hollow Knight/*.dat".
pub fn pretty_pattern(p: &str) -> String {
    p.replace("<home>/", "")
        .replace("<winAppData>", "AppData/Roaming")
        .replace("<winLocalAppData>", "AppData/Local")
        .replace("<winDocuments>", "Documents")
        .replace("<winPublic>", "Users/Public")
        .replace("<winProgramData>", "ProgramData")
        .replace("<xdgConfig>", "~/.config")
        .replace("<xdgData>", "~/.local/share")
        .replace("<storeUserId>", "*")
        .replace("<root>", "[Steam]")
        .replace("<base>", "[game folder]")
        .replace("<game>", "[game]")
}

/// A folder the user picked, as a pattern relative to the game's context.
fn pattern_for(dir: &Path, ctx: &saves::Ctx) -> String {
    if let Some(base) = &ctx.base
        && let Ok(rel) = dir.strip_prefix(base)
    {
        return format!("<base>/{}", rel.display());
    }
    if let Ok(rel) = dir.strip_prefix(&ctx.home) {
        return format!("<home>/{}", rel.display());
    }
    if let Some(public) = ctx.home.parent().map(|p| p.join("Public"))
        && let Ok(rel) = dir.strip_prefix(&public)
    {
        return format!("<winPublic>/{}", rel.display());
    }
    dir.to_string_lossy().to_string()
}

impl App {
    /// Load the save database index in the background, downloading it first if
    /// it's missing or over a month old.
    pub fn ensure_save_index(&mut self, ctx: &egui::Context) {
        if self.saves.index.is_some() || self.saves.index_loading {
            return;
        }
        let stale = saves::index_age().is_none_or(|t| unix_now().saturating_sub(t) > 30 * 86_400);
        if stale && !self.tasks.iter().any(|t| t.key == "save-db") {
            self.update_save_db();
            if saves::index_age().is_none() {
                return;
            }
        }
        self.saves.index_loading = true;
        let (tx, ctx) = (self.saves.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Index(saves::load_index()));
            ctx.request_repaint();
        });
    }

    pub fn update_save_db(&mut self) {
        let part = data_dir().join("cache/ludusavi-manifest.yaml.part");
        self.start_task(
            TaskKind::SaveDb,
            "save-db",
            lg("Downloading the save database"),
            Some((part, 17_700_000)),
            saves::update_manifest,
        );
    }

    pub fn saves_poll(&mut self) {
        while let Ok(msg) = self.saves.rx.try_recv() {
            match msg {
                Msg::Index(ix) => {
                    self.saves.index_loading = false;
                    self.saves.index = ix.map(Arc::new);
                    self.saves.view = None;
                    self.saves.summary.clear();
                }
                Msg::Found(id, f) => {
                    self.saves.scanning = None;
                    let n = f.len();
                    self.saves.found.insert(id, f);
                    self.set_status(
                        if n == 0 { tr("No other copies of these saves were found") } else { trf("Found saves in {} other places", &[&n]) },
                        false,
                    );
                }
            }
        }
    }

    pub fn game_saves(&self, i: usize) -> GameSaves {
        let ix = self.saves.index.as_deref().map(|v| v.as_slice());
        GameSaves::new(&self.cfg.custom_games[i], &self.cfg.settings, ix)
    }

    /// Build (or rebuild) the page's view of game `i`.
    fn saves_view(&mut self, i: usize) -> &View {
        let id = self.cfg.custom_games[i].id.clone();
        if self.saves.view.as_ref().is_none_or(|v| v.id != id) {
            let gs = self.game_saves(i);
            let resolved = gs.resolved();
            let rows: Vec<LocRow> = gs
                .locations
                .iter()
                .map(|l| {
                    let matches = resolved.iter().find(|(p, _)| *p == l.pattern).map(|(_, m)| m.clone()).unwrap_or_default();
                    let (size, _, newest) = saves::measure(&matches);
                    LocRow {
                        pattern: l.pattern.clone(),
                        origin: l.origin,
                        ignored: gs.vault.ignored.contains(&l.pattern),
                        matches,
                        size,
                        newest,
                    }
                })
                .collect();
            let total: u64 = rows.iter().map(|r| r.size).sum();
            let backups = saves::list_backups(&id);
            let sync = saves::sync_state(&gs, &self.cfg.settings);
            self.saves.summary.insert(
                id.clone(),
                (rows.iter().filter(|r| !r.matches.is_empty()).count(), total, backups.first().map_or(0, |b| b.1.created)),
            );
            self.saves.view = Some(View { id, gs, rows, backups, sync });
        }
        self.saves.view.as_ref().expect("view just built")
    }

    /// Run a save operation in the background; the page refreshes when it ends.
    fn saves_task<F>(&mut self, label: String, work: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        self.start_task(TaskKind::Saves, &format!("saves:{label}"), &label, None, work);
    }

    fn edit_vault(&mut self, i: usize, f: impl FnOnce(&mut saves::Vault)) {
        let id = self.cfg.custom_games[i].id.clone();
        let mut v = saves::load_vault(&id);
        f(&mut v);
        saves::save_vault(&id, &v);
        self.saves.view = None;
        self.saves.summary.remove(&id);
    }

    /// The "Saves" card on the game page.
    pub fn saves_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, acts: &mut Acts) {
        self.ensure_save_index(ctx);
        let id = self.cfg.custom_games[i].id.clone();
        if !self.saves.summary.contains_key(&id) && !self.saves.index_loading {
            self.saves.view = None;
            self.saves_view(i);
        }
        let summary = self.saves.summary.get(&id).copied();
        section(ui, "Saves", |ui| {
            ui.horizontal(|ui| {
                match summary {
                    Some((0, _, _)) | None => {
                        status_dot(ui, pal().faint, false);
                        ui.label(egui::RichText::new(tr("No saves found yet — they're learned the first time you play")).color(pal().subtext));
                    }
                    Some((n, size, last)) => {
                        status_dot(ui, pal().ok, true);
                        let backup = if last == 0 { lg("not backed up yet").to_string() } else { lfmt("last backup {}", &[&format_ago(last, unix_now())]) };
                        let locs = if n == 1 { lg("1 location").to_string() } else { lfmt("{} locations", &[&n]) };
                        ui.label(egui::RichText::new(disp(&format!("{locs} · {} · {backup}", format_bytes(size)))).color(pal().subtext));
                    }
                }
                ui.with_layout(far_end(), |ui| {
                    if primary_button(ui, "Open Save Vault").clicked() {
                        acts.game.push((id.clone(), GameAct::Saves));
                    }
                });
            });
        });
    }

    pub fn page_saves(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let Some(i) = self.selected_index() else {
            acts.back = true;
            return;
        };
        self.ensure_save_index(ctx);
        let name = self.cfg.custom_games[i].name.clone();
        acts.back |= back_button(ui, ctx, &disp(&name));
        ui.add_space(10.0);
        page_header(ui, "Save Vault", &lfmt("Backups, transfers and sync for {}", &[&name]));
        ui.set_max_width(900.0);
        if self.saves.index.is_none() && (self.saves.index_loading || self.tasks.iter().any(|t| t.key == "save-db")) {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(tr("Loading the save database…")).color(pal().subtext));
            });
            ui.add_space(8.0);
        }
        self.saves_view(i);
        let keep = self.cfg.settings.save_keep();
        let settings = self.cfg.settings.clone();

        // ---- Locations
        let mut add_folder = false;
        let mut toggle: Option<String> = None;
        let mut remove: Option<String> = None;
        let mut open_search = false;
        {
            let view = self.saves.view.as_ref().expect("view");
            let manifest = view.gs.manifest.as_ref().map(|m| m.name.clone());
            let chosen = view.gs.vault.manifest_name.is_some();
            let has_index = self.saves.index.is_some();
            section_sub(ui, "Where the saves are", "Found from the Ludusavi save database, learned while you play, or added by you", |ui| {
                ui.horizontal_wrapped(|ui| {
                    match &manifest {
                        Some(m) => {
                            status_dot(ui, pal().ok, true);
                            let what = if chosen { lg("Using the save database entry") } else { lg("Matched in the save database as") };
                            ui.label(egui::RichText::new(disp(&format!("{what} “{m}”"))).color(pal().subtext));
                        }
                        None if has_index => {
                            status_dot(ui, pal().warn, true);
                            ui.label(egui::RichText::new(tr("Not in the save database — play once and its save folder is learned, or add it")).color(pal().subtext));
                        }
                        None => {}
                    }
                    if has_index && ui.small_button(tr("Change…")).clicked() {
                        open_search = true;
                    }
                });
                ui.add_space(8.0);
                if view.rows.is_empty() {
                    ui.label(egui::RichText::new(tr("No save locations yet.")).color(pal().faint));
                }
                for r in &view.rows {
                    card_inner().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let here = !r.matches.is_empty();
                            status_dot(ui, if r.ignored { pal().faint } else if here { pal().ok } else { pal().faint }, here && !r.ignored);
                            vcol(ui, |ui| {
                                ui.add(
                                    egui::Label::new(egui::RichText::new(pretty_pattern(&r.pattern)).color(if r.ignored { pal().faint } else { pal().text }).monospace())
                                        .truncate(),
                                )
                                .on_hover_text(&r.pattern);
                                let (origin, color) = match r.origin {
                                    Origin::Database => (tr("save database"), pal().accent),
                                    Origin::Learned => (tr("learned while playing"), pal().ok),
                                    Origin::Manual => (tr("added by you"), pal().subtext),
                                };
                                let detail = if r.ignored {
                                    lg("left out of backups").to_string()
                                } else if r.matches.is_empty() {
                                    lg("nothing here yet").to_string()
                                } else {
                                    format!("{} · {}", format_bytes(r.size), format_time(r.newest))
                                };
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(origin).color(color).small());
                                    ui.label(egui::RichText::new(disp(&format!("· {detail}"))).color(pal().subtext).small());
                                });
                            });
                            ui.with_layout(far_end(), |ui| {
                                if r.origin != Origin::Database && ui.small_button(tr("Remove")).clicked() {
                                    remove = Some(r.pattern.clone());
                                }
                                let label = if r.ignored { tr("Include") } else { tr("Leave out") };
                                if ui.small_button(label).clicked() {
                                    toggle = Some(r.pattern.clone());
                                }
                                if let Some(m) = r.matches.first() {
                                    let dir = if m.is_dir() { m.clone() } else { m.parent().map(Path::to_path_buf).unwrap_or_default() };
                                    if ui.small_button(tr("Open")).clicked() {
                                        acts.open = Some(dir.to_string_lossy().into());
                                    }
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
                ui.add_space(4.0);
                if ui.button(tr("Add a folder…")).clicked() {
                    add_folder = true;
                }
            });
        }
        if open_search {
            self.saves.search = Some(name.clone());
        }
        if let Some(p) = toggle {
            self.edit_vault(i, |v| {
                if let Some(k) = v.ignored.iter().position(|x| *x == p) {
                    v.ignored.remove(k);
                } else {
                    v.ignored.push(p);
                }
            });
        }
        if let Some(p) = remove {
            self.edit_vault(i, |v| {
                v.learned.retain(|x| *x != p);
                v.manual.retain(|x| *x != p);
            });
        }
        if add_folder && let Some(dir) = pick_folder(lg("Pick the folder the game saves into")) {
            let pattern = pattern_for(&dir, &self.saves.view.as_ref().expect("view").gs.ctx);
            self.edit_vault(i, |v| {
                if !v.manual.contains(&pattern) {
                    v.manual.push(pattern);
                }
            });
        }
        ui.add_space(12.0);
        self.saves_view(i);

        // ---- Backups
        let mut backup_now = false;
        let mut restore: Option<PathBuf> = None;
        let mut delete: Option<PathBuf> = None;
        let mut auto_toggle: Option<bool> = None;
        {
            let view = self.saves.view.as_ref().expect("view");
            let auto = !view.gs.vault.no_auto_backup;
            let global_auto = settings.save_auto_backup();
            let any_saves = view.rows.iter().any(|r| !r.matches.is_empty() && !r.ignored);
            section_sub(ui, "Backups", &lfmt("Kept in Gamelo's own folder — the newest {} are kept", &[&keep]), |ui| {
                ui.horizontal(|ui| {
                    let mut on = auto;
                    if ui.add_enabled_ui(global_auto, |ui| check(ui, &mut on, tr("Back up after every session"))).inner
                        .on_disabled_hover_text(tr("Automatic backups are off in Settings"))
                        .changed()
                    {
                        auto_toggle = Some(on);
                    }
                    ui.with_layout(far_end(), |ui| {
                        if ui.add_enabled(any_saves, egui::Button::new(tr("Back up now"))).clicked() {
                            backup_now = true;
                        }
                    });
                });
                ui.add_space(6.0);
                if view.backups.is_empty() {
                    ui.label(egui::RichText::new(tr("No backups yet.")).color(pal().faint));
                }
                for (dir, info) in view.backups.iter().take(12) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format_time(info.created)).color(pal().text));
                        let label = if info.label.is_empty() { String::new() } else { format!("{} · ", lg(&info.label)) };
                        let files = if info.files == 1 { lg("1 file").to_string() } else { lfmt("{} files", &[&info.files]) };
                        ui.label(egui::RichText::new(disp(&format!("{label}{files} · {}", format_bytes(info.size)))).color(pal().subtext).small());
                        ui.with_layout(far_end(), |ui| {
                            if ui.small_button(tr("Delete")).clicked() {
                                delete = Some(dir.clone());
                            }
                            if ui.small_button(tr("Open")).clicked() {
                                acts.open = Some(dir.join("files").to_string_lossy().into());
                            }
                            if ui.small_button(tr("Restore")).on_hover_text(tr("Your current saves are backed up first")).clicked() {
                                restore = Some(dir.clone());
                            }
                        });
                    });
                }
            });
        }
        if let Some(on) = auto_toggle {
            self.edit_vault(i, |v| v.no_auto_backup = !on);
        }
        if backup_now {
            let gs = self.game_saves(i);
            self.saves_task(lg("Backing up saves").into(), move || match saves::backup(&gs, lg("manual"), keep, true)? {
                Some(info) => Ok(lfmt("Backed up {} files ({})", &[&info.files, &format_bytes(info.size)])),
                None => Err(lg("No saves found to back up").into()),
            });
        }
        if let Some(dir) = restore {
            let gs = self.game_saves(i);
            self.saves_task(lg("Restoring saves").into(), move || saves::restore_into_game(&gs, &dir, keep));
        }
        if let Some(dir) = delete {
            let _ = std::fs::remove_dir_all(&dir);
            self.saves.view = None;
            self.set_status(tr("Backup deleted"), false);
        }
        ui.add_space(12.0);
        self.saves_view(i);

        // ---- Elsewhere
        let id = self.cfg.custom_games[i].id.clone();
        let scanning = self.saves.scanning.as_deref() == Some(id.as_str());
        let found = self.saves.found.get(&id).cloned();
        let mut scan = false;
        let mut copy_here: Option<saves::Ctx> = None;
        let mut copy_there: Option<saves::Ctx> = None;
        let own_newest = self.saves.view.as_ref().map_or(0, |v| v.rows.iter().map(|r| r.newest).max().unwrap_or(0));
        let has_own = own_newest > 0;
        section_sub(
            ui,
            "Find saves elsewhere",
            "Looks in Steam, Heroic, Lutris and Bottles prefixes, other Gamelo prefixes and Windows drives",
            |ui| {
                ui.horizontal(|ui| {
                    if ui.add_enabled(!scanning, egui::Button::new(if found.is_some() { tr("Scan again") } else { tr("Scan") })).clicked() {
                        scan = true;
                    }
                    if scanning {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Searching…")).color(pal().subtext).small());
                    }
                });
                if let Some(found) = &found {
                    ui.add_space(6.0);
                    if found.is_empty() {
                        ui.label(egui::RichText::new(tr("No other copies found.")).color(pal().faint));
                    }
                    for f in found {
                        card_inner().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                vcol(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(disp(&f.ctx.label)).strong().color(pal().text));
                                        if f.newest > own_newest && has_own {
                                            pill(ui, "newer than yours", pal().warn);
                                        } else if !has_own {
                                            pill(ui, "you have none", pal().accent);
                                        }
                                    });
                                    let files = if f.files == 1 { lg("1 file").to_string() } else { lfmt("{} files", &[&f.files]) };
                                    ui.label(
                                        egui::RichText::new(disp(&format!("{files} · {} · {}", format_bytes(f.size), format_time(f.newest))))
                                            .color(pal().subtext)
                                            .small(),
                                    )
                                    .on_hover_text(f.paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n"));
                                });
                                ui.with_layout(far_end(), |ui| {
                                    if f.ctx.windows_drive && has_own && ui.button(tr("Copy mine there")).clicked() {
                                        copy_there = Some(f.ctx.clone());
                                    }
                                    if primary_button(ui, "Copy here").on_hover_text(tr("Your current saves are backed up first")).clicked() {
                                        copy_here = Some(f.ctx.clone());
                                    }
                                    if let Some(p) = f.paths.first() {
                                        let dir = if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_default() };
                                        if ui.small_button(tr("Open")).clicked() {
                                            acts.open = Some(dir.to_string_lossy().into());
                                        }
                                    }
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                }
            },
        );
        if scan {
            self.saves.scanning = Some(id.clone());
            let gs = self.game_saves(i);
            let (tx, ctx2, settings) = (self.saves.tx.clone(), ctx.clone(), self.cfg.settings.clone());
            std::thread::spawn(move || {
                let found = saves::find_elsewhere(&gs, &settings);
                let _ = tx.send(Msg::Found(gs.game.id.clone(), found));
                ctx2.request_repaint();
            });
        }
        if let Some(from) = copy_here {
            let gs = self.game_saves(i);
            let to = gs.ctx.clone();
            self.saves_task(lfmt("Copying saves from {}", &[&from.label]), move || saves::transfer(&gs, &from, &to, keep));
        }
        if let Some(to) = copy_there {
            let gs = self.game_saves(i);
            let from = gs.ctx.clone();
            self.saves_task(lfmt("Copying saves to {}", &[&to.label]), move || saves::transfer(&gs, &from, &to, keep));
        }
        ui.add_space(12.0);
        self.saves_view(i);

        // ---- Sync & move
        let mut push = false;
        let mut pull = false;
        let mut sync_toggle: Option<bool> = None;
        let mut export = false;
        let mut import = false;
        {
            let view = self.saves.view.as_ref().expect("view");
            let sync_dir = saves::sync_dir_for(&settings, &view.gs.game);
            let no_sync = view.gs.vault.no_sync;
            section_sub(ui, "Sync & move", "Keep saves the same on every computer, or carry them by hand", |ui| {
                match &sync_dir {
                    None => {
                        ui.label(
                            egui::RichText::new(tr("Set a sync folder in Settings › Saves (for example a Syncthing, Nextcloud or Dropbox folder, or a drive Windows can see) and saves are copied there after every session and picked up before the next one."))
                                .color(pal().subtext),
                        );
                        if ui.button(tr("Open Settings")).clicked() {
                            acts.goto = Some(Page::Settings);
                        }
                    }
                    Some(dir) => {
                        ui.horizontal(|ui| {
                            let mut on = !no_sync;
                            if check(ui, &mut on, tr("Sync this game")).changed() {
                                sync_toggle = Some(on);
                            }
                            ui.add(egui::Label::new(egui::RichText::new(dir.display().to_string()).color(pal().subtext).small().monospace()).truncate());
                        });
                        if !no_sync {
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| match &view.sync {
                                SyncState::Same => {
                                    status_dot(ui, pal().ok, true);
                                    ui.label(egui::RichText::new(tr("In sync")).color(pal().ok));
                                }
                                SyncState::Newer(info) => {
                                    status_dot(ui, pal().accent, true);
                                    ui.label(
                                        egui::RichText::new(trf("The sync folder has newer saves (from {}, {})", &[&info.machine, &format_time(info.created)]))
                                            .color(pal().accent),
                                    );
                                    if primary_button(ui, "Get them").clicked() {
                                        pull = true;
                                    }
                                }
                                SyncState::LocalNewer => {
                                    status_dot(ui, pal().warn, true);
                                    ui.label(egui::RichText::new(tr("This computer has changes the sync folder doesn't")).color(pal().subtext));
                                    if ui.button(tr("Send now")).clicked() {
                                        push = true;
                                    }
                                }
                                SyncState::Conflict(info) => {
                                    status_dot(ui, pal().danger, true);
                                    ui.label(
                                        egui::RichText::new(trf("Saves changed both here and on {} — pick which to keep", &[&info.machine]))
                                            .color(pal().danger),
                                    );
                                    if ui.button(tr("Use the synced copy")).clicked() {
                                        pull = true;
                                    }
                                    if ui.button(tr("Keep this computer's")).clicked() {
                                        push = true;
                                    }
                                }
                                SyncState::Empty => {
                                    ui.label(egui::RichText::new(tr("Nothing to sync yet")).color(pal().faint));
                                }
                                SyncState::Off => {}
                            });
                        }
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(tr("Export to a folder…")).on_hover_text(tr("A folder you can take anywhere, with a note saying where each part goes on Windows")).clicked() {
                        export = true;
                    }
                    if ui.button(tr("Import from a folder…")).on_hover_text(tr("An export, a backup, or save files copied from Windows")).clicked() {
                        import = true;
                    }
                });
            });
        }
        if let Some(on) = sync_toggle {
            self.edit_vault(i, |v| v.no_sync = !on);
        }
        if push {
            let mut gs = self.game_saves(i);
            let s = settings.clone();
            self.saves_task(lg("Sending saves to the sync folder").into(), move || saves::push_sync(&mut gs, &s));
        }
        if pull {
            let mut gs = self.game_saves(i);
            let s = settings.clone();
            self.saves_task(lg("Getting saves from the sync folder").into(), move || saves::pull_sync(&mut gs, &s));
        }
        if export && let Some(dest) = pick_folder(lg("Where to put the exported saves")) {
            let gs = self.game_saves(i);
            self.saves_task(lg("Exporting saves").into(), move || saves::export_pack(&gs, &dest));
        }
        if import && let Some(src) = pick_folder(lg("Folder with the saves to import")) {
            if src.join("set.json").is_file() {
                let gs = self.game_saves(i);
                self.saves_task(lg("Importing saves").into(), move || saves::import_folder(&gs, &src, None, keep));
            } else {
                let first = self.saves.view.as_ref().and_then(|v| v.rows.iter().find(|r| !r.ignored)).map(|r| r.pattern.clone());
                self.saves.import_src = Some(src);
                self.saves.import_target = first;
            }
        }
    }

    /// Dialogs: picking a save-database entry, and where an import goes.
    pub fn saves_modals(&mut self, ctx: &egui::Context) {
        if let Some(mut query) = self.saves.search.take() {
            let mut pick: Option<Option<String>> = None;
            let mut close = false;
            let index = self.saves.index.clone();
            let resp = egui::Modal::new(egui::Id::unique("save-db-search")).show(ctx, |ui| dir_ui(ui, |ui| {
                ui.set_width(480.0);
                ui.label(egui::RichText::new(tr("Find the game in the save database")).size(18.0).strong().color(pal().text));
                ui.add_space(8.0);
                text_field(ui, &mut query, &tr("Game name…"), f32::INFINITY);
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    if ui.selectable_label(false, tr("Automatic (match by name or Steam ID)")).clicked() {
                        pick = Some(None);
                    }
                    if let Some(ix) = &index {
                        for m in saves::search_index(ix, &query) {
                            let n = m.files.len();
                            let locs = if n == 1 { lg("1 location").to_string() } else { lfmt("{} locations", &[&n]) };
                            let label = disp(&format!("{}  ·  {locs}", m.name));
                            if ui.selectable_label(false, label).clicked() {
                                pick = Some(Some(m.name.clone()));
                            }
                        }
                    }
                });
                ui.add_space(8.0);
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
            }));
            match pick {
                Some(choice) => {
                    if let Some(i) = self.selected_index() {
                        self.edit_vault(i, |v| v.manifest_name = choice);
                    }
                }
                None if close || resp.should_close() => {}
                None => self.saves.search = Some(query),
            }
        }
        if let Some(src) = self.saves.import_src.clone() {
            let rows: Vec<String> = self.saves.view.as_ref().map(|v| v.rows.iter().map(|r| r.pattern.clone()).collect()).unwrap_or_default();
            let (mut go, mut cancel) = (false, false);
            let resp = egui::Modal::new(egui::Id::unique("save-import")).show(ctx, |ui| dir_ui(ui, |ui| {
                ui.set_width(520.0);
                ui.label(egui::RichText::new(tr("Where do these saves go?")).size(18.0).strong().color(pal().text));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(src.display().to_string()).color(pal().subtext).small().monospace());
                ui.add_space(8.0);
                if rows.is_empty() {
                    ui.label(egui::RichText::new(tr("This game has no save locations yet — add one first.")).color(pal().warn));
                }
                for p in &rows {
                    let on = self.saves.import_target.as_deref() == Some(p.as_str());
                    if ui.radio(on, pretty_pattern(p)).clicked() {
                        self.saves.import_target = Some(p.clone());
                    }
                }
                ui.add_space(6.0);
                note(ui, 0.0, "A folder with the same name as the location replaces it; otherwise its contents are copied in. Your current saves are backed up first.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(self.saves.import_target.is_some(), |ui| {
                        if primary_button(ui, "Import").clicked() {
                            go = true;
                        }
                    });
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            }));
            if go && let Some(i) = self.selected_index() {
                let gs = self.game_saves(i);
                let (keep, target) = (self.cfg.settings.save_keep(), self.saves.import_target.clone());
                self.saves.import_src = None;
                self.saves_task(lg("Importing saves").into(), move || saves::import_folder(&gs, &src, target.as_deref(), keep));
            } else if cancel || resp.should_close() {
                self.saves.import_src = None;
            }
        }
    }
}
