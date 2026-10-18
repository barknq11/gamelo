//! The Stores page: connect Epic Games and GOG, browse what you own, install,
//! update and uninstall, and bring games installed with Heroic over.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr, trf};
use crate::stores::{self, Installed, LaunchInfo, Owned, SizeInfo, Store, StoreLink};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, Sender};

/// A game ready to go into the library.
pub struct Added {
    pub store: Store,
    pub title: String,
    pub installed: Installed,
    pub launch: LaunchInfo,
    pub cover: Option<String>,
    pub hero: Option<String>,
    /// A prefix to keep using (Heroic's), so saves carry over.
    pub prefix: Option<PathBuf>,
}

pub enum Msg {
    Tool(Store, Result<String, String>),
    Login(Store, Result<String, String>),
    Library(Store, Result<(Vec<Owned>, Vec<Installed>), String>),
    Size(Store, String, Result<SizeInfo, String>),
    Added(Box<Result<Added, String>>),
    Uninstalled(Store, String, bool, Result<(), String>),
}

/// The install dialog: the game and, once known, its sizes.
pub type Confirm = (Store, Owned, Option<Result<SizeInfo, String>>);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Show {
    All,
    Installed,
    NotInstalled,
    Updates,
}

pub struct StoresState {
    pub tab: Store,
    pub code: String,
    /// Jobs in flight: "tool:epic", "login:gog", "lib:epic", "app:epic:<app>"…
    pub busy: HashSet<String>,
    pub owned: HashMap<Store, Vec<Owned>>,
    pub installed: HashMap<Store, Vec<Installed>>,
    pub errors: HashMap<Store, String>,
    pub search: String,
    pub show: Show,
    /// Libraries fetched since the app started.
    pub loaded: HashSet<Store>,
    /// The install dialog: the game and, once known, its sizes.
    pub confirm: Option<Confirm>,
    /// The uninstall dialog: the game and "also remove from the library".
    pub uninstall: Option<(Store, Installed, bool)>,
    /// Heroic installs found on this machine (scanned once).
    pub heroic: Option<Vec<(Store, Installed, Option<PathBuf>)>>,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for StoresState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        StoresState {
            tab: Store::Epic,
            code: String::new(),
            busy: HashSet::new(),
            owned: HashMap::new(),
            installed: HashMap::new(),
            errors: HashMap::new(),
            search: String::new(),
            show: Show::All,
            loaded: HashSet::new(),
            confirm: None,
            uninstall: None,
            heroic: None,
            tx,
            rx,
        }
    }
}

fn key(store: Store) -> &'static str {
    match store {
        Store::Epic => "epic",
        Store::Gog => "gog",
    }
}

/// Launch info and artwork for an installed game (worker thread).
fn prepare(store: Store, inst: Installed, owned: Option<Owned>, prefix: Option<PathBuf>) -> Result<Added, String> {
    let launch = stores::launch_info(store, &inst.app, std::path::Path::new(&inst.path))?;
    let (cover, hero) = match (store, &owned) {
        (Store::Epic, Some(o)) => (o.cover.clone(), o.hero.clone()),
        (Store::Gog, _) => stores::gog_art(&inst.app),
        _ => (String::new(), String::new()),
    };
    let slug = format!("{}-{}", key(store), slugify(&inst.app));
    let title = owned.map(|o| o.title).filter(|t| !t.is_empty()).unwrap_or_else(|| inst.title.clone());
    Ok(Added {
        store,
        title,
        cover: stores::save_art(&cover, &format!("{slug}-cover")),
        hero: stores::save_art(&hero, &format!("{slug}-hero")),
        installed: inst,
        launch,
        prefix,
    })
}

impl App {
    fn stores_job(&mut self, busy: String, work: impl FnOnce() -> Msg + Send + 'static) {
        if !self.stores.busy.insert(busy) {
            return;
        }
        let (tx, ctx) = (self.stores.tx.clone(), self.stores_ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    fn refresh_store(&mut self, store: Store) {
        self.stores.loaded.insert(store);
        self.stores.errors.remove(&store);
        self.stores_job(format!("lib:{}", key(store)), move || {
            Msg::Library(store, stores::library(store).and_then(|o| stores::installed(store).map(|i| (o, i))))
        });
    }

    pub fn stores_poll(&mut self, ctx: &egui::Context) {
        self.stores_ctx = Some(ctx.clone());
        while let Ok(msg) = self.stores.rx.try_recv() {
            match msg {
                Msg::Tool(store, r) => {
                    self.stores.busy.remove(&format!("tool:{}", key(store)));
                    match r {
                        Ok(m) => self.set_status(m, false),
                        Err(e) => {
                            self.stores.errors.insert(store, e.clone());
                            self.set_status(e, true);
                        }
                    }
                }
                Msg::Login(store, r) => {
                    self.stores.busy.remove(&format!("login:{}", key(store)));
                    match r {
                        Ok(name) => {
                            self.stores.code.clear();
                            self.set_status(lfmt("Signed in to {} as {}", &[&store.name(), &name]), false);
                            self.refresh_store(store);
                        }
                        Err(e) => {
                            self.stores.errors.insert(store, e);
                        }
                    }
                }
                Msg::Library(store, r) => {
                    self.stores.busy.remove(&format!("lib:{}", key(store)));
                    match r {
                        Ok((owned, installed)) => {
                            self.stores.owned.insert(store, owned);
                            self.stores.installed.insert(store, installed);
                        }
                        Err(e) => {
                            self.stores.errors.insert(store, e);
                        }
                    }
                }
                Msg::Size(store, app, r) => {
                    if let Some((s, o, size)) = self.stores.confirm.as_mut()
                        && *s == store
                        && o.app == app
                    {
                        *size = Some(r);
                    }
                }
                Msg::Added(r) => match *r {
                    Ok(a) => {
                        self.stores.busy.remove(&format!("app:{}:{}", key(a.store), a.installed.app));
                        let store = a.store;
                        let title = a.title.clone();
                        self.add_store_game(ctx, a);
                        self.set_status(lfmt("{} is in your library", &[&title]), false);
                        self.refresh_store(store);
                    }
                    Err(e) => {
                        self.stores.busy.retain(|b| !b.starts_with("app:"));
                        self.set_status(e, true);
                    }
                },
                Msg::Uninstalled(store, app, remove, r) => {
                    self.stores.busy.remove(&format!("app:{}:{app}", key(store)));
                    match r {
                        Ok(()) => {
                            if remove {
                                self.cfg.custom_games.retain(|g| g.store.as_ref().is_none_or(|l| !(l.store == store && l.app == app)));
                                save_config(&self.cfg);
                            }
                            self.set_status(lg("Uninstalled").to_string(), false);
                            self.refresh_store(store);
                        }
                        Err(e) => self.set_status(e, true),
                    }
                }
            }
        }
    }

    /// Put an installed store game in the library, or update the entry it already has.
    fn add_store_game(&mut self, ctx: &egui::Context, a: Added) {
        let args = a.launch.args.iter().map(|x| shell_quote(x)).collect::<Vec<_>>().join(" ");
        let workdir = a.launch.workdir.as_ref().map(|w| w.to_string_lossy().to_string()).unwrap_or_default();
        let link = StoreLink { store: a.store, app: a.installed.app.clone(), path: a.installed.path.clone() };
        let existing = self.cfg.custom_games.iter().position(|g| g.store.as_ref().is_some_and(|l| l.store == link.store && l.app == link.app));
        let i = match existing {
            Some(i) => i,
            None => {
                let mut m = self.new_add_modal(a.launch.exe.clone());
                m.name = a.title.clone();
                m.kind = GameKind::Windows;
                self.insert_game(m);
                self.cfg.custom_games.len() - 1
            }
        };
        let g = &mut self.cfg.custom_games[i];
        g.exe = a.launch.exe.to_string_lossy().into();
        g.args = args;
        g.workdir = workdir;
        g.store = Some(link);
        if let Some(p) = &a.prefix
            && g.prefix.is_empty()
        {
            g.prefix = p.to_string_lossy().into();
        }
        let id = g.id.clone();
        let mut need_art = existing.is_none();
        for (kind, path) in [(ArtKind::Cover, a.cover), (ArtKind::Hero, a.hero)] {
            if let Some(p) = path {
                self.set_art(&id, kind, p);
                need_art = kind != ArtKind::Cover && need_art;
            }
        }
        save_config(&self.cfg);
        if need_art && self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
            self.auto_fetch(ctx, vec![id]);
        }
    }

    pub fn page_stores(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        page_header(ui, "Stores", "Your Epic Games and GOG libraries — install them here and play through Proton");
        ui.horizontal(|ui| {
            for store in [Store::Epic, Store::Gog] {
                if choice_chip(ui, self.stores.tab == store, store.name()).clicked() {
                    self.stores.tab = store;
                }
            }
        });
        ui.add_space(14.0);
        let store = self.stores.tab;
        let tool = stores::tool_path(store);
        let account = tool.as_ref().and_then(|_| stores::account(store));
        if account.is_some() && !self.stores.loaded.contains(&store) {
            self.refresh_store(store);
        }
        self.account_card(ui, store, tool.is_some(), account.as_deref(), acts);
        if account.is_some() {
            ui.add_space(12.0);
            self.store_library_card(ui, ctx, store, acts);
        }
        ui.add_space(12.0);
        self.heroic_card(ui, store);
    }

    fn account_card(&mut self, ui: &mut egui::Ui, store: Store, has_tool: bool, account: Option<&str>, acts: &mut Acts) {
        let tool_name = match store {
            Store::Epic => "legendary",
            Store::Gog => "gogdl",
        };
        let error = self.stores.errors.get(&store).cloned();
        section(ui, "Account", |ui| {
            if !has_tool {
                para(ui, &lfmt("ProtonLaunch talks to {} through {}, a small open-source tool (the same one Heroic uses). It's downloaded from GitHub and checked before it's used.", &[&store.name(), &tool_name]), pal().subtext, true);
                ui.add_space(8.0);
                let busy = self.stores.busy.contains(&format!("tool:{}", key(store)));
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if primary_button(ui, &lfmt("Download {}", &[&tool_name])).clicked() {
                            self.stores_job(format!("tool:{}", key(store)), move || Msg::Tool(store, stores::install_tool(store)));
                        }
                    });
                    if busy {
                        ui.spinner();
                    }
                });
            } else if let Some(name) = account {
                ui.horizontal(|ui| {
                    status_dot(ui, pal().ok, true);
                    ui.label(egui::RichText::new(trf("Signed in as {}", &[&name])).color(pal().text));
                    ui.with_layout(far_end(), |ui| {
                        if ui.button(tr("Sign out")).clicked() {
                            let _ = stores::logout(store);
                            self.stores.owned.remove(&store);
                            self.stores.installed.remove(&store);
                            self.stores.loaded.remove(&store);
                        }
                        let busy = self.stores.busy.contains(&format!("lib:{}", key(store)));
                        ui.add_enabled_ui(!busy, |ui| {
                            if ui.button(tr("Refresh")).clicked() {
                                self.refresh_store(store);
                            }
                        });
                        if busy {
                            ui.spinner();
                        }
                    });
                });
                ui.add_space(8.0);
                let dir = self.cfg.settings.store_dir(store);
                modal_row(ui, "Install folder", |ui| {
                    ui.label(egui::RichText::new(dir.to_string_lossy()).monospace().small().color(pal().subtext));
                    if ui.small_button(tr("Change…")).clicked()
                        && let Some(p) = pick_folder(&tr("Install folder"))
                    {
                        let p = p.to_string_lossy().to_string();
                        match store {
                            Store::Epic => self.cfg.settings.epic_dir = p,
                            Store::Gog => self.cfg.settings.gog_dir = p,
                        }
                        save_config(&self.cfg);
                    }
                    if let Some(free) = stores::free_space(&dir) {
                        ui.label(egui::RichText::new(trf("{} free", &[&format_bytes(free)])).small().color(pal().faint));
                    }
                });
            } else {
                let busy = self.stores.busy.contains(&format!("login:{}", key(store)));
                ui.label(egui::RichText::new(tr("1. Open the sign-in page and sign in.")).color(pal().text));
                ui.add_space(4.0);
                if ui.button(trf("Open {} sign-in", &[&store.name()])).clicked() {
                    acts.open = Some(store.login_url().to_string());
                }
                ui.add_space(10.0);
                let hint = match store {
                    Store::Epic => "2. The page then shows a line of text with \"authorizationCode\" — copy all of it (or just the code) and paste it here.",
                    Store::Gog => "2. You land on a mostly blank page — copy its address from the address bar and paste it here.",
                };
                para(ui, hint, pal().text, true);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - button_width(ui, "Sign in") - 8.0).max(160.0);
                    let (r, _) = text_field(ui, &mut self.stores.code, "Paste here", w);
                    let code = stores::extract_code(store, &self.stores.code);
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_enabled_ui(code.is_some() && !busy, |ui| {
                        if primary_button(ui, "Sign in").clicked() || (enter && code.is_some()) {
                            let code = code.clone().unwrap_or_default();
                            self.stores.errors.remove(&store);
                            self.stores_job(format!("login:{}", key(store)), move || Msg::Login(store, stores::login(store, &code)));
                        }
                    });
                    if busy {
                        ui.spinner();
                    }
                });
                if !self.stores.code.trim().is_empty() && stores::extract_code(store, &self.stores.code).is_none() {
                    note(ui, 0.0, "That doesn't look like a sign-in code yet.");
                }
                note(ui, 0.0, "Your password never goes through ProtonLaunch: you sign in on the store's own page, and the code is only good for a few minutes.");
            }
            if let Some(e) = &error {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
            }
        });
    }

    fn store_library_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, store: Store, acts: &mut Acts) {
        let owned = self.stores.owned.get(&store).cloned().unwrap_or_default();
        let installed = self.stores.installed.get(&store).cloned().unwrap_or_default();
        let inst = |app: &str| installed.iter().find(|i| i.app == app).cloned();
        let update = |o: &Owned, i: &Installed| !o.version.is_empty() && !i.version.is_empty() && o.version != i.version;
        let n_updates = owned.iter().filter(|o| inst(&o.app).is_some_and(|i| update(o, &i))).count();
        // Installed games the store no longer lists (or that we know only locally).
        let mut rows: Vec<(Owned, Option<Installed>)> = owned.iter().map(|o| (o.clone(), inst(&o.app))).collect();
        for i in &installed {
            if !owned.iter().any(|o| o.app == i.app) {
                rows.push((Owned { app: i.app.clone(), title: i.title.clone(), ..Default::default() }, Some(i.clone())));
            }
        }
        let q = self.stores.search.trim().to_lowercase();
        let show = self.stores.show;
        rows.retain(|(o, i)| {
            (q.is_empty() || o.title.to_lowercase().contains(&q))
                && match show {
                    Show::All => true,
                    Show::Installed => i.is_some(),
                    Show::NotInstalled => i.is_none(),
                    Show::Updates => i.as_ref().is_some_and(|i| update(o, i)),
                }
        });
        let loading = self.stores.busy.contains(&format!("lib:{}", key(store)));
        let mut confirm: Option<Owned> = None;
        let mut add: Option<(Owned, Installed)> = None;
        let mut reinstall: Option<Owned> = None;
        let mut remove: Option<Installed> = None;
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(trf("Library · {}", &[&owned.len()])).strong().color(pal().text));
                ui.with_layout(far_end(), |ui| {
                    text_field(ui, &mut self.stores.search, "Search…", 200.0);
                });
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let options = [
                    (Show::All, lg("All").to_string()),
                    (Show::Installed, lg("Installed").to_string()),
                    (Show::NotInstalled, lg("Not installed").to_string()),
                    (Show::Updates, lfmt("Updates ({})", &[&n_updates])),
                ];
                for (s, label) in options {
                    if (s != Show::Updates || n_updates > 0) && choice_chip(ui, self.stores.show == s, &label).clicked() {
                        self.stores.show = s;
                    }
                }
            });
            ui.add_space(8.0);
            if loading && owned.is_empty() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(tr("Loading your library…")).color(pal().subtext));
                });
                return;
            }
            if rows.is_empty() {
                ui.label(egui::RichText::new(tr("Nothing to show.")).color(pal().subtext));
                return;
            }
            let width = ui.available_width();
            for (o, i) in &rows {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::hover());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                if resp.hovered() {
                    ui.painter().rect_filled(rect, rad(10), pal().card2);
                }
                let thumb_x = if rtl() { rect.right() - 8.0 - 96.0 } else { rect.left() + 8.0 };
                let thumb = egui::Rect::from_min_size(egui::pos2(thumb_x, rect.top() + 5.0), egui::vec2(96.0, 54.0));
                match (!o.thumb.is_empty()).then(|| self.store_image(ctx, &o.thumb)).flatten() {
                    Some(t) => {
                        ui.painter().add(cover_fill_shape(&t, thumb, 6, 1.0));
                    }
                    None => {
                        ui.painter().rect_filled(thumb, rad(6), pal().card2);
                    }
                }
                let busy_key = format!("app:{}:{}", key(store), o.app);
                let task = self.tasks.iter().find(|t| t.key == busy_key).map(|t| t.fraction());
                let in_library = self.cfg.custom_games.iter().any(|g| g.store.as_ref().is_some_and(|l| l.store == store && l.app == o.app));
                let status = match (task, i) {
                    (Some(f), _) => (lfmt("Installing… {}%", &[&((f * 100.0) as u32)]), pal().accent),
                    (None, Some(i)) if update(o, i) => (lg("Update available").to_string(), GameStatus::Backlog.color()),
                    (None, Some(_)) if in_library => (lg("Installed · in your library").to_string(), pal().ok),
                    (None, Some(_)) => (lg("Installed").to_string(), pal().ok),
                    (None, None) => (lg("Not installed").to_string(), pal().faint),
                };
                let text_rect = if rtl() {
                    egui::Rect::from_min_max(egui::pos2(rect.left() + 260.0, rect.top()), egui::pos2(thumb.left() - 14.0, rect.bottom()))
                } else {
                    egui::Rect::from_min_max(egui::pos2(thumb.right() + 14.0, rect.top()), egui::pos2(rect.right() - 260.0, rect.bottom()))
                };
                let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
                let tx = if rtl() { text_rect.right() } else { text_rect.left() };
                ui.painter().with_clip_rect(text_rect).text(egui::pos2(tx, rect.center().y - 9.0), align, disp(&o.title), egui::FontId::proportional(15.0), pal().text);
                ui.painter().text(egui::pos2(tx, rect.center().y + 11.0), align, disp(&status.0), egui::FontId::proportional(12.5), status.1);
                if let Some(f) = task {
                    let bar = egui::Rect::from_min_size(egui::pos2(text_rect.left(), rect.bottom() - 8.0), egui::vec2(text_rect.width().min(260.0), 4.0));
                    ui.painter().rect_filled(bar, rad(2), pal().card2);
                    ui.painter().rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * f, 4.0)), rad(2), pal().accent);
                }
                // Actions at the far end.
                let actions = if rtl() {
                    egui::Rect::from_min_max(egui::pos2(rect.left() + 8.0, rect.top()), egui::pos2(rect.left() + 250.0, rect.bottom()))
                } else {
                    egui::Rect::from_min_max(egui::pos2(rect.right() - 250.0, rect.top()), egui::pos2(rect.right() - 8.0, rect.bottom()))
                };
                let layout = if rtl() { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) };
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(actions).layout(layout));
                if task.is_some() || self.stores.busy.contains(&busy_key) {
                    child.spinner();
                    continue;
                }
                match i {
                    None => {
                        if primary_button(&mut child, "Install").clicked() {
                            confirm = Some(o.clone());
                        }
                    }
                    Some(i) => {
                        child.menu_button(tr("More"), |ui| menu_ui(ui, |ui| {
                            if ui.button(tr("Update / repair")).clicked() {
                                reinstall = Some(o.clone());
                                ui.close();
                            }
                            if ui.button(tr("Open game folder")).clicked() {
                                acts.open = Some(i.path.clone());
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(egui::RichText::new(tr("Uninstall…")).color(pal().danger)).clicked() {
                                remove = Some(i.clone());
                                ui.close();
                            }
                        }));
                        if update(o, i) && primary_button(&mut child, "Update").clicked() {
                            reinstall = Some(o.clone());
                        }
                        if in_library {
                            if let Some(g) = self.cfg.custom_games.iter().find(|g| g.store.as_ref().is_some_and(|l| l.store == store && l.app == o.app))
                                && child.button(tr("Open game page")).clicked()
                            {
                                acts.game.push((g.id.clone(), GameAct::Open));
                            }
                        } else if child.button(tr("Add to library")).clicked() {
                            add = Some((o.clone(), i.clone()));
                        }
                    }
                }
            }
        });
        if let Some(o) = confirm {
            let app = o.app.clone();
            self.stores.confirm = Some((store, o, None));
            let tx = self.stores.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Size(store, app.clone(), stores::size_info(store, &app)));
                ctx.request_repaint();
            });
        }
        if let Some((o, i)) = add {
            self.stores_job(format!("app:{}:{}", key(store), i.app), move || Msg::Added(Box::new(prepare(store, i, Some(o), None))));
        }
        if let Some(o) = reinstall {
            self.start_store_install(store, o);
        }
        if let Some(i) = remove {
            self.stores.uninstall = Some((store, i, true));
        }
    }

    /// Install or update `o` as a tracked task; it lands in the library when done.
    fn start_store_install(&mut self, store: Store, o: Owned) {
        let busy_key = format!("app:{}:{}", key(store), o.app);
        if self.tasks.iter().any(|t| t.key == busy_key) {
            return;
        }
        let base = self.cfg.settings.store_dir(store);
        let shared = Arc::new(AtomicU32::new(0));
        let (tx, ctx) = (self.stores.tx.clone(), self.stores_ctx.clone());
        let progress = shared.clone();
        let title = o.title.clone();
        let label = lfmt("Installing {}", &[&title]);
        self.start_task(TaskKind::Store, &busy_key, &label, None, move || {
            let r = stores::install(store, &o.app, &base, progress).and_then(|inst| prepare(store, inst, Some(o), None));
            let msg = r.as_ref().map(|_| lfmt("Installed {}", &[&title])).map_err(|e| e.clone());
            let _ = tx.send(Msg::Added(Box::new(r)));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
            msg
        });
        if let Some(t) = self.tasks.last_mut() {
            t.shared = Some(shared);
        }
    }

    /// Games installed with Heroic, offered for import.
    fn heroic_card(&mut self, ui: &mut egui::Ui, store: Store) {
        let found = self.stores.heroic.get_or_insert_with(stores::heroic_installs).clone();
        let todo: Vec<_> = found
            .into_iter()
            .filter(|(s, i, _)| *s == store && !self.cfg.custom_games.iter().any(|g| g.store.as_ref().is_some_and(|l| l.store == *s && l.app == i.app)))
            .collect();
        if todo.is_empty() {
            return;
        }
        let names: Vec<String> = todo.iter().map(|(_, i, _)| i.title.clone()).take(6).collect();
        let busy = self.stores.busy.contains(&format!("heroic:{}", key(store)));
        let mut go = false;
        section(ui, "Installed with Heroic", |ui| {
            para(ui, &lfmt("Found {} games: {}. Import them to play from ProtonLaunch — no download, and each keeps its Heroic prefix so saves carry over.", &[&todo.len(), &names.join(", ")]), pal().subtext, true);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    go = primary_button(ui, "Import them").clicked();
                });
                if busy {
                    ui.spinner();
                }
            });
        });
        if go {
            for (s, inst, prefix) in todo {
                let owned = self.stores.owned.get(&s).and_then(|l| l.iter().find(|o| o.app == inst.app).cloned());
                self.stores_job(format!("app:{}:{}", key(s), inst.app), move || {
                    Msg::Added(Box::new(stores::import(s, &inst).and_then(|_| prepare(s, inst, owned, prefix))))
                });
            }
            self.stores.heroic = None;
        }
    }

    /// The install and uninstall dialogs.
    pub fn stores_modals(&mut self, ctx: &egui::Context) {
        if let Some((store, o, size)) = self.stores.confirm.take() {
            let dir = self.cfg.settings.store_dir(store);
            let mut go = false;
            let mut cancel = false;
            let resp = egui::Modal::new(egui::Id::unique("store-install")).show(ctx, |ui| dir_ui(ui, |ui| {
                ui.set_width(440.0);
                ui.label(egui::RichText::new(trf("Install {}", &[&o.title])).size(18.0).strong().color(pal().text));
                ui.add_space(10.0);
                match &size {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new(tr("Checking the size…")).color(pal().subtext));
                        });
                    }
                    Some(Ok(s)) => {
                        modal_row(ui, "Download", |ui| {
                            ui.label(egui::RichText::new(format_bytes(s.download)).color(pal().text));
                        });
                        modal_row(ui, "On disk", |ui| {
                            ui.label(egui::RichText::new(format_bytes(s.disk)).color(pal().text));
                        });
                        let free = stores::free_space(&dir);
                        modal_row(ui, "Free space", |ui| {
                            let enough = free.is_none_or(|f| f > s.disk);
                            let text = free.map_or_else(|| "?".into(), format_bytes);
                            ui.label(egui::RichText::new(text).color(if enough { pal().text } else { pal().warn }));
                        });
                    }
                    Some(Err(e)) => {
                        ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
                    }
                }
                modal_row(ui, "Folder", |ui| {
                    ui.label(egui::RichText::new(dir.join(size.as_ref().and_then(|s| s.as_ref().ok()).map_or("", |s| s.folder.as_str())).to_string_lossy()).monospace().small().color(pal().subtext));
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(size.as_ref().is_some_and(|s| s.is_ok()), |ui| {
                        go = primary_button(ui, "Install").clicked();
                    });
                    cancel = ui.button(tr("Cancel")).clicked();
                });
            }));
            if go {
                self.start_store_install(store, o);
            } else if !(cancel || resp.should_close()) {
                self.stores.confirm = Some((store, o, size));
            }
        }
        if let Some((store, inst, mut remove)) = self.stores.uninstall.take() {
            let mut go = false;
            let mut cancel = false;
            let resp = egui::Modal::new(egui::Id::unique("store-uninstall")).show(ctx, |ui| dir_ui(ui, |ui| {
                ui.set_width(440.0);
                ui.label(egui::RichText::new(trf("Uninstall {}?", &[&inst.title])).size(18.0).strong().color(pal().text));
                ui.add_space(8.0);
                para(ui, &lfmt("This deletes the game's files in {}. Cloud saves stay with {}.", &[&inst.path, &store.name()]), pal().subtext, true);
                ui.add_space(8.0);
                check(ui, &mut remove, tr("Also remove it from the ProtonLaunch library"));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    go = danger_button(ui, "Uninstall").clicked();
                    cancel = ui.button(tr("Cancel")).clicked();
                });
            }));
            if go {
                let app = inst.app.clone();
                self.stores_job(format!("app:{}:{app}", key(store)), move || {
                    Msg::Uninstalled(store, app.clone(), remove, stores::uninstall(store, &app, std::path::Path::new(&inst.path)))
                });
            } else if !(cancel || resp.should_close()) {
                self.stores.uninstall = Some((store, inst, remove));
            }
        }
    }
}
