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
    /// A GOG game's portrait cover URL (empty: none).
    Cover(String, String),
}

/// What the Stores page knows about one owned game.
struct Info {
    task: Option<f32>,
    busy: bool,
    /// Its library entry, once added.
    game: Option<String>,
    update: bool,
    running: bool,
    text: String,
    color: egui::Color32,
}

/// A click on the Stores page, applied after drawing.
enum Do {
    Install(Owned),
    Add(Owned, Installed),
    Reinstall(Owned),
    Remove(Installed),
    Folder(String),
    Game(String, GameAct),
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
    /// GOG portrait covers by product id, fetched as tiles come into view.
    pub covers: HashMap<String, String>,
    /// Stores whose sign-in page was opened (step 2 done).
    pub opened: HashSet<Store>,
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
            covers: HashMap::new(),
            opened: HashSet::new(),
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
                Msg::Cover(app, url) => {
                    self.stores.busy.remove(&format!("cover:{app}"));
                    self.stores.covers.insert(app, url);
                }
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
        let store = self.stores.tab;
        let tool = stores::tool_path(store);
        let account = tool.as_ref().and_then(|_| stores::account(store));
        if account.is_some() && !self.stores.loaded.contains(&store) {
            self.refresh_store(store);
        }
        self.stores_header(ui, store, account.as_deref());
        ui.add_space(14.0);
        match account {
            None => {
                self.account_card(ui, store, tool.is_some(), acts);
                ui.add_space(12.0);
                self.heroic_card(ui, store);
            }
            Some(_) => self.store_library(ui, ctx, store, acts),
        }
    }

    /// Title and counts on one side; the store switch, refresh and account
    /// menu on the other — laid out like the Library's header.
    fn stores_header(&mut self, ui: &mut egui::Ui, store: Store, account: Option<&str>) {
        let owned = self.stores.owned.get(&store).map_or(0, Vec::len);
        let installed = self.stores.installed.get(&store).map_or(0, Vec::len);
        ui.horizontal(|ui| {
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(tr("Stores")).size(24.0).strong().color(pal().text));
                let sub = match account {
                    Some(_) if owned > 0 => {
                        let games = if owned == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&owned]) };
                        format!("{} · {games} · {}", store.name(), lfmt("{} installed", &[&installed]))
                    }
                    Some(name) => trf("Signed in as {}", &[&name]),
                    None => tr("Your Epic Games and GOG libraries — install them here and play through Proton"),
                };
                ui.label(egui::RichText::new(disp(&sub)).color(pal().subtext).small());
            });
            ui.with_layout(far_end(), |ui| {
                // Added from the far end: GOG, then Epic, reads "Epic  GOG".
                for s in [Store::Gog, Store::Epic] {
                    if choice_chip(ui, self.stores.tab == s, s.name()).clicked() {
                        self.stores.tab = s;
                    }
                }
                let Some(name) = account else { return };
                ui.add_space(10.0);
                let chip = account_chip(ui, name);
                egui::Popup::menu(&chip)
                    .width(300.0)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| menu_ui(ui, |ui| self.account_panel(ui, store, name, owned, installed)));
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
    }

    /// The panel behind the account chip: who's signed in, where games go,
    /// and signing out (asked twice).
    fn account_panel(&mut self, ui: &mut egui::Ui, store: Store, name: &str, owned: usize, installed: usize) {
        ui.set_min_width(280.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            avatar(ui.painter(), r.center(), 22.0, name);
            ui.add_space(6.0);
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(disp(name)).size(16.0).strong().color(pal().text));
                ui.label(egui::RichText::new(trf("{} account", &[&store.name()])).small().color(pal().subtext));
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let games = if owned == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&owned]) };
            pill(ui, &games, pal().subtext);
            pill(ui, &lfmt("{} installed", &[&installed]), pal().ok);
        });
        ui.separator();
        ui.label(egui::RichText::new(tr("Install folder")).small().color(pal().faint));
        let dir = self.cfg.settings.store_dir(store);
        ui.label(egui::RichText::new(dir.to_string_lossy()).monospace().small().color(pal().text));
        ui.horizontal(|ui| {
            if let Some(free) = stores::free_space(&dir) {
                ui.label(egui::RichText::new(trf("{} free", &[&format_bytes(free)])).small().color(pal().subtext));
            }
            ui.with_layout(far_end(), |ui| {
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
            });
        });
        ui.separator();
        let asking_id = egui::Id::unique(("sign-out-asking", key(store)));
        let asking = ui.data(|d| d.get_temp::<bool>(asking_id)).unwrap_or(false);
        if !asking {
            if ui.button(egui::RichText::new(tr("Sign out…")).color(pal().danger)).clicked() {
                ui.data_mut(|d| d.insert_temp(asking_id, true));
            }
            return;
        }
        para(ui, &lfmt("Sign out of {}? Installed games stay installed and in your library.", &[&store.name()]), pal().text, true);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if danger_button(ui, "Sign out").clicked() {
                let _ = stores::logout(store);
                self.stores.owned.remove(&store);
                self.stores.installed.remove(&store);
                self.stores.loaded.remove(&store);
                self.stores.opened.remove(&store);
                ui.data_mut(|d| d.remove::<bool>(asking_id));
                ui.close();
            }
            if ui.button(tr("Cancel")).clicked() {
                ui.data_mut(|d| d.remove::<bool>(asking_id));
            }
        });
    }

    /// Connecting a store, as three steps: get the helper tool, sign in on
    /// the store's page, paste what it shows. Shown until there's an account.
    fn account_card(&mut self, ui: &mut egui::Ui, store: Store, has_tool: bool, acts: &mut Acts) {
        let tool_name = match store {
            Store::Epic => "legendary",
            Store::Gog => "gogdl",
        };
        let code = stores::extract_code(store, &self.stores.code);
        let opened = self.stores.opened.contains(&store) || !self.stores.code.trim().is_empty();
        let current = if !has_tool { 1 } else if !opened { 2 } else { 3 };
        let error = self.stores.errors.get(&store).cloned();
        card_frame().inner_margin(egui::Margin::same(22)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
                store_mark(ui.painter(), r, store);
                ui.add_space(8.0);
                vcol(ui, |ui| {
                    ui.label(egui::RichText::new(trf("Connect {}", &[&store.name()])).size(19.0).strong().color(pal().text));
                    para(ui, &lfmt("Bring your {} library into Gamelo: install, update and play your games through Proton.", &[&store.name()]), pal().subtext, true);
                });
            });
            ui.add_space(18.0);

            let state = |n: u8| if n < current { Step::Done } else if n == current { Step::Now } else { Step::Later };
            // 1. The helper tool.
            step_row(ui, 1, state(1), &lfmt("Get {}", &[&tool_name]), &lfmt("A small open-source helper (the same one Heroic uses) that talks to {}. Downloaded from GitHub and checked before it's used.", &[&store.name()]), |ui| {
                let busy = self.stores.busy.contains(&format!("tool:{}", key(store)));
                if has_tool {
                    pill(ui, "Ready", pal().ok);
                } else if busy {
                    ui.spinner();
                } else if primary_button(ui, &lfmt("Download {}", &[&tool_name])).clicked() {
                    self.stores_job(format!("tool:{}", key(store)), move || Msg::Tool(store, stores::install_tool(store)));
                }
            });
            // 2. The store's own sign-in page.
            step_row(ui, 2, state(2), &lfmt("Sign in to {}", &[&store.name()]), "It opens in your browser. Sign in there as you normally would.", |ui| {
                ui.add_enabled_ui(has_tool, |ui| {
                    let label = trf("Open {} sign-in", &[&store.name()]);
                    let clicked = if current == 2 { primary_button(ui, &label).clicked() } else { ui.button(label).clicked() };
                    if clicked {
                        acts.open = Some(store.login_url().to_string());
                        self.stores.opened.insert(store);
                    }
                });
            });
            // 3. Paste what the page shows.
            let hint = match store {
                Store::Epic => "The page then shows a line of text with \"authorizationCode\" — copy all of it (or just the code) and paste it here.",
                Store::Gog => "You land on a mostly blank page — copy its address from the address bar and paste it here.",
            };
            step_row(ui, 3, state(3), "Paste what the page shows", hint, |_| {});
            ui.add_enabled_ui(has_tool, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(STEP_INDENT);
                    let busy = self.stores.busy.contains(&format!("login:{}", key(store)));
                    let w = (ui.available_width() - button_width(ui, "Paste") - button_width(ui, "Sign in") - 20.0).max(160.0);
                    let (r, _) = text_field(ui, &mut self.stores.code, "Paste here", w);
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button(tr("Paste")).clicked()
                        && let Some(text) = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok())
                    {
                        self.stores.code = text.trim().to_string();
                    }
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
                if !self.stores.code.trim().is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(STEP_INDENT);
                        let (text, color) = if code.is_some() { ("That looks right — press Sign in.", pal().ok) } else { ("That doesn't look like a sign-in code yet.", pal().warn) };
                        ui.label(egui::RichText::new(tr(text)).small().color(color));
                    });
                }
            });
            if let Some(e) = &error {
                ui.add_space(10.0);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .corner_radius(rad(8))
                    .fill(pal().warn.gamma_multiply(0.10))
                    .stroke(egui::Stroke::new(1.0, pal().warn.gamma_multiply(0.45)))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        para(ui, &lfmt("Couldn't sign in: {}", &[&e.as_str()]), pal().warn, true);
                    });
            }
            ui.add_space(14.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                lock_icon(ui.painter(), r.center(), pal().faint);
                ui.add_space(2.0);
                vcol(ui, |ui| para(ui, "Your password never goes through Gamelo: you sign in on the store's own page, and the code is only good for a few minutes.", pal().faint, true));
            });
        });
    }

    /// What we know about one owned game: its install, task and library entry.
    fn store_info(&self, store: Store, o: &Owned, i: Option<&Installed>) -> Info {
        let busy_key = format!("app:{}:{}", key(store), o.app);
        let task = self.tasks.iter().find(|t| t.key == busy_key).map(|t| t.fraction());
        let game = self.cfg.custom_games.iter().find(|g| g.store.as_ref().is_some_and(|l| l.store == store && l.app == o.app)).map(|g| g.id.clone());
        let update = i.is_some_and(|i| !o.version.is_empty() && !i.version.is_empty() && o.version != i.version);
        let (text, color) = match (task, i) {
            (Some(f), _) => (lfmt("Installing… {}%", &[&((f * 100.0) as u32)]), pal().accent),
            (None, Some(_)) if update => (lg("Update available").to_string(), GameStatus::Backlog.color()),
            (None, Some(_)) if game.is_some() => (lg("Installed · in your library").to_string(), pal().ok),
            (None, Some(_)) => (lg("Installed").to_string(), pal().ok),
            (None, None) => (lg("Not installed").to_string(), pal().faint),
        };
        let running = game.as_ref().is_some_and(|id| self.running.contains_key(id));
        Info { busy: task.is_some() || self.stores.busy.contains(&busy_key), task, game, update, running, text, color }
    }

    /// The game's portrait cover: Epic lists it; GOG's comes from its public
    /// API, fetched as tiles come into view. Falls back to the wide image.
    fn store_cover(&mut self, ctx: &egui::Context, store: Store, o: &Owned) -> Option<egui::TextureHandle> {
        let url = match store {
            Store::Epic if !o.cover.is_empty() => o.cover.clone(),
            Store::Epic => o.thumb.clone(),
            Store::Gog => match self.stores.covers.get(&o.app) {
                Some(u) if !u.is_empty() => u.clone(),
                Some(_) => o.thumb.clone(),
                None => {
                    let app = o.app.clone();
                    self.stores_job(format!("cover:{app}"), move || Msg::Cover(app.clone(), stores::gog_art(&app).0));
                    return None;
                }
            },
        };
        (!url.is_empty()).then(|| self.store_image(ctx, &url)).flatten()
    }

    /// The signed-in page: search and layout, filters, then the games as
    /// posters (or rows), like the Library.
    fn store_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, store: Store, acts: &mut Acts) {
        let owned = self.stores.owned.get(&store).cloned().unwrap_or_default();
        let installed = self.stores.installed.get(&store).cloned().unwrap_or_default();
        let inst = |app: &str| installed.iter().find(|i| i.app == app).cloned();
        let mut rows: Vec<(Owned, Option<Installed>)> = owned.iter().map(|o| (o.clone(), inst(&o.app))).collect();
        // Installed games the store no longer lists (or that we know only locally).
        for i in &installed {
            if !owned.iter().any(|o| o.app == i.app) {
                rows.push((Owned { app: i.app.clone(), title: i.title.clone(), ..Default::default() }, Some(i.clone())));
            }
        }
        let n_updates = rows.iter().filter(|(o, i)| self.store_info(store, o, i.as_ref()).update).count();

        ui.horizontal(|ui| {
            let (search, _) = text_field(ui, &mut self.stores.search, "Search games…", 280.0);
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                search.request_focus();
            }
            ui.with_layout(far_end(), |ui| {
                if let Some(k) = tile_size_toggle(ui, &self.cfg.settings) {
                    self.set_layout(k);
                }
            });
        });
        ui.add_space(10.0);
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
        if let Some(e) = self.stores.errors.get(&store) {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
        }
        ui.add_space(18.0);
        self.heroic_card(ui, store);

        let q = self.stores.search.trim().to_lowercase();
        let show = self.stores.show;
        rows.retain(|(o, i)| {
            (q.is_empty() || o.title.to_lowercase().contains(&q))
                && match show {
                    Show::All => true,
                    Show::Installed => i.is_some(),
                    Show::NotInstalled => i.is_none(),
                    Show::Updates => self.store_info(store, o, i.as_ref()).update,
                }
        });
        if self.stores.busy.contains(&format!("lib:{}", key(store))) && owned.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new(tr("Loading your library…")).color(pal().subtext));
            });
            return;
        }
        if rows.is_empty() {
            empty_hint(ui, if q.is_empty() { "Nothing to show." } else { "No games match your search." });
            return;
        }

        let mut todo = None;
        if self.cfg.settings.library_view == "list" {
            for (o, i) in &rows {
                self.store_row(ui, ctx, store, o, i.as_ref(), &mut todo);
            }
        } else {
            let gap = grid_gap(ui.available_width() - 1.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, 22.0);
                for (o, i) in &rows {
                    self.store_tile(ui, ctx, store, o, i.as_ref(), &mut todo);
                }
            });
        }
        match todo {
            Some(Do::Install(o)) => {
                let app = o.app.clone();
                self.stores.confirm = Some((store, o, None));
                let tx = self.stores.tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Size(store, app.clone(), stores::size_info(store, &app)));
                    ctx.request_repaint();
                });
            }
            Some(Do::Add(o, i)) => {
                self.stores_job(format!("app:{}:{}", key(store), i.app), move || Msg::Added(Box::new(prepare(store, i, Some(o), None))));
            }
            Some(Do::Reinstall(o)) => self.start_store_install(store, o),
            Some(Do::Remove(i)) => self.stores.uninstall = Some((store, i, true)),
            Some(Do::Folder(path)) => acts.open = Some(path),
            Some(Do::Game(id, act)) => acts.game.push((id, act)),
            None => {}
        }
    }

    /// The tile's main action: install it, add it to the library, or play it.
    fn store_primary(o: &Owned, i: Option<&Installed>, info: &Info) -> Option<Do> {
        match (i, &info.game) {
            _ if info.busy => None,
            (None, _) => Some(Do::Install(o.clone())),
            (Some(i), None) => Some(Do::Add(o.clone(), i.clone())),
            (Some(_), Some(id)) => Some(Do::Game(id.clone(), if info.running { GameAct::Stop } else { GameAct::Play })),
        }
    }

    /// Everything else, on right-click (and behind "More" in the list).
    fn store_menu(ui: &mut egui::Ui, o: &Owned, i: Option<&Installed>, info: &Info, todo: &mut Option<Do>) {
        let mut pick = |ui: &mut egui::Ui, label: egui::RichText, d: Do| {
            if ui.button(label).clicked() {
                *todo = Some(d);
                ui.close();
            }
        };
        let Some(i) = i else {
            pick(ui, egui::RichText::new(tr("Install")), Do::Install(o.clone()));
            return;
        };
        match &info.game {
            Some(id) => {
                let (label, act) = if info.running { ("Stop", GameAct::Stop) } else { ("Play", GameAct::Play) };
                pick(ui, egui::RichText::new(tr(label)), Do::Game(id.clone(), act));
                pick(ui, egui::RichText::new(tr("Open game page")), Do::Game(id.clone(), GameAct::Open));
            }
            None => pick(ui, egui::RichText::new(tr("Add to library")), Do::Add(o.clone(), i.clone())),
        }
        let label = if info.update { "Update" } else { "Update / repair" };
        pick(ui, egui::RichText::new(tr(label)), Do::Reinstall(o.clone()));
        pick(ui, egui::RichText::new(tr("Open game folder")), Do::Folder(i.path.clone()));
        ui.separator();
        pick(ui, egui::RichText::new(tr("Uninstall…")).color(pal().danger), Do::Remove(i.clone()));
    }

    /// One poster in the grid, with the Library tile's lift, shadow and glow.
    fn store_tile(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, store: Store, o: &Owned, i: Option<&Installed>, todo: &mut Option<Do>) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(tile_w(), tile_h() + 46.0), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let info = self.store_info(store, o, i);
        let id = format!("store-tile-{}-{}", key(store), o.app);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let h = ctx.animate_bool_with_time_and_easing(egui::Id::unique((&id, "h")), resp.hovered(), self.secs(0.16), egui::emath::easing::cubic_out);
        let spring = ctx.animate_bool_with_time_and_easing(egui::Id::unique((&id, "s")), resp.hovered(), self.secs(0.34), egui::emath::easing::back_out);
        let press = ctx.animate_bool_with_time(egui::Id::unique((&id, "p")), resp.is_pointer_button_down_on(), self.secs(0.07));
        let poster = egui::Rect::from_min_size(rect.min, egui::vec2(tile_w(), tile_h()));
        let poster = egui::Rect::from_center_size(
            poster.center() + egui::vec2(0.0, -7.0 * spring + 3.0 * press),
            poster.size() * (1.0 + 0.04 * spring - 0.05 * press),
        );
        let painter = ui.painter();
        if h > 0.0 {
            for (grow, alpha) in [(2.0, 70.0), (6.0, 40.0), (11.0, 22.0)] {
                painter.rect_filled(
                    poster.translate(egui::vec2(0.0, 5.0 + 7.0 * spring)).expand(grow * spring.max(0.0)),
                    rad(10 + grow as u8),
                    theme::shadow(alpha * h),
                );
            }
        }
        let tex = self.store_cover(ctx, store, o);
        let painter = ui.painter();
        match &tex {
            Some(t) => {
                painter.add(cover_fill_shape_pan(t, poster, 10, 1.0, 1.0 + 0.07 * h, egui::vec2(0.5, 0.5)));
            }
            None => {
                painter.rect_filled(poster, rad(10), pal().card2);
                let galley = painter.layout(disp(&o.title), egui::FontId::proportional(15.0), pal().subtext, poster.width() - 24.0);
                painter.galley(poster.center() - galley.size() / 2.0, galley, pal().subtext);
            }
        }
        let over = theme::OverImage::new();
        let center = poster.center();
        if let Some(f) = info.task {
            painter.rect_filled(poster, rad(10), egui::Color32::from_black_alpha(150));
            spinner_arc(painter, center, 22.0, self.sweep(1.1) * std::f32::consts::TAU, pal().accent);
            painter.text(center + egui::vec2(0.0, 36.0), egui::Align2::CENTER_TOP, format!("{}%", (f * 100.0) as u32), egui::FontId::proportional(13.0), pal().text);
            let bar = egui::Rect::from_min_size(poster.left_bottom() + egui::vec2(10.0, -14.0), egui::vec2(poster.width() - 20.0, 4.0));
            painter.rect_filled(bar, rad(2), egui::Color32::from_white_alpha(40));
            painter.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * f, 4.0)), rad(2), pal().accent);
        } else if info.busy {
            painter.rect_filled(poster, rad(10), egui::Color32::from_black_alpha(130));
            spinner_arc(painter, center, 22.0, self.sweep(1.1) * std::f32::consts::TAU, pal().accent);
        } else if h > 0.0 {
            // Hover: dim a little and spring the main action's button in.
            painter.rect_filled(poster, rad(10), egui::Color32::from_black_alpha((70.0 * h) as u8));
            let fill = if info.running { pal().danger } else { pal().accent };
            let r = 27.0 * (0.55 + 0.45 * spring) * (1.0 - 0.08 * press);
            if pal().square {
                painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r)), 0, fill.gamma_multiply(h));
            } else {
                painter.circle_filled(center, r, fill.gamma_multiply(h));
            }
            let icon = pal().on_accent.gamma_multiply(h);
            let s = r / 27.0;
            match (i, &info.game) {
                (None, _) => {
                    // A download arrow.
                    painter.rect_filled(egui::Rect::from_center_size(center - egui::vec2(0.0, 4.0 * s), egui::vec2(4.0, 12.0) * s), rad(1), icon);
                    let tip = vec![center + egui::vec2(-8.0, 1.0) * s, center + egui::vec2(8.0, 1.0) * s, center + egui::vec2(0.0, 10.0) * s];
                    painter.add(egui::Shape::convex_polygon(tip, icon, egui::Stroke::NONE));
                }
                (Some(_), None) => {
                    // A plus: add to the library.
                    painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(18.0, 4.0) * s), rad(1), icon);
                    painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(4.0, 18.0) * s), rad(1), icon);
                }
                (Some(_), Some(_)) if info.running => {
                    painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(16.0, 16.0) * s), rad(2), icon);
                }
                (Some(_), Some(_)) => play_triangle(painter, center, 11.0 * s, icon),
            }
        }
        if i.is_some() {
            let (label, color) = if info.update { ("Update available", GameStatus::Backlog.color()) } else { ("Installed", pal().ok) };
            poster_badge(painter, poster.left_top() + egui::vec2(8.0, 8.0), label, color, 1.0);
        }
        drop(over);
        painter.rect_stroke(poster, rad(10), egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h)), egui::StrokeKind::Inside);
        if h > 0.0 {
            painter.rect_stroke(poster.expand(2.0), rad(12), egui::Stroke::new(3.0, pal().accent.gamma_multiply(0.18 * h)), egui::StrokeKind::Outside);
        }
        // Title and state under the poster.
        let mut job = egui::text::LayoutJob::simple_singleline(disp(&o.title), egui::FontId::proportional(14.5), pal().text.lerp_to_gamma(pal().accent, 0.55 * h));
        job.wrap = egui::text::TextWrapping { max_width: tile_w(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let title = painter.layout_job(job);
        let state = painter.layout_no_wrap(disp(&info.text), egui::FontId::proportional(12.5), info.color);
        let x = |w: f32| if rtl() { rect.right() - w } else { rect.left() };
        painter.galley(egui::pos2(x(title.size().x), rect.top() + tile_h() + 8.0), title, pal().text);
        painter.galley(egui::pos2(x(state.size().x), rect.top() + tile_h() + 28.0), state, info.color);

        if resp.clicked()
            && let Some(d) = Self::store_primary(o, i, &info)
        {
            *todo = Some(d);
        }
        if !info.busy {
            resp.context_menu(|ui| menu_ui(ui, |ui| Self::store_menu(ui, o, i, &info, todo)));
        }
    }

    /// One row of the list view: a wide thumbnail, title and state, actions.
    fn store_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, store: Store, o: &Owned, i: Option<&Installed>, todo: &mut Option<Do>) {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let info = self.store_info(store, o, i);
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
        let text_rect = if rtl() {
            egui::Rect::from_min_max(egui::pos2(rect.left() + 260.0, rect.top()), egui::pos2(thumb.left() - 14.0, rect.bottom()))
        } else {
            egui::Rect::from_min_max(egui::pos2(thumb.right() + 14.0, rect.top()), egui::pos2(rect.right() - 260.0, rect.bottom()))
        };
        let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
        let tx = if rtl() { text_rect.right() } else { text_rect.left() };
        ui.painter().with_clip_rect(text_rect).text(egui::pos2(tx, rect.center().y - 9.0), align, disp(&o.title), egui::FontId::proportional(15.0), pal().text);
        ui.painter().text(egui::pos2(tx, rect.center().y + 11.0), align, disp(&info.text), egui::FontId::proportional(12.5), info.color);
        if let Some(f) = info.task {
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
        if info.busy {
            child.spinner();
            return;
        }
        if i.is_some() {
            child.menu_button(tr("More"), |ui| menu_ui(ui, |ui| Self::store_menu(ui, o, i, &info, todo)));
        }
        let label = match (i, &info.game) {
            (None, _) => "Install",
            (Some(_), None) => "Add to library",
            (Some(_), Some(_)) if info.running => "Stop",
            (Some(_), Some(_)) => "Play",
        };
        if primary_button(&mut child, label).clicked() {
            *todo = Self::store_primary(o, i, &info);
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
            para(ui, &lfmt("Found {} games: {}. Import them to play from Gamelo — no download, and each keeps its Heroic prefix so saves carry over.", &[&todo.len(), &names.join(", ")]), pal().subtext, true);
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
                check(ui, &mut remove, tr("Also remove it from the Gamelo library"));
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

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Done,
    Now,
    Later,
}

/// How far a step's text sits from the edge (past its number).
const STEP_INDENT: f32 = 50.0;

/// One step of connecting a store: its number (a tick once done), a title
/// and a line of help, and its control at the far end.
fn step_row(ui: &mut egui::Ui, n: u8, state: Step, title: &str, sub: &str, control: impl FnOnce(&mut egui::Ui)) {
    let control_w = 230.0;
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
        let p = ui.painter();
        let c = r.center();
        match state {
            Step::Done => {
                p.circle_filled(c, 14.0, pal().ok.gamma_multiply(0.18));
                p.circle_stroke(c, 14.0, egui::Stroke::new(1.5, pal().ok));
                check_mark(p, c, 12.0, pal().ok);
            }
            Step::Now => {
                p.circle_filled(c, 14.0, pal().accent);
                p.text(c, egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(14.0), pal().on_accent);
            }
            Step::Later => {
                p.circle_stroke(c, 13.5, egui::Stroke::new(1.5, pal().border));
                p.text(c, egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(14.0), pal().faint);
            }
        }
        ui.add_space(12.0);
        let text_w = (ui.available_width() - control_w).max(200.0);
        ui.allocate_ui(egui::vec2(text_w, 0.0), |ui| {
            vcol(ui, |ui| {
                let color = if state == Step::Later { pal().faint } else { pal().text };
                ui.add_space(4.0);
                ui.label(egui::RichText::new(tr(title)).size(15.0).strong().color(color));
                para(ui, sub, if state == Step::Later { pal().faint } else { pal().subtext }, true);
            });
        });
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 34.0), far_end(), control);
    });
    ui.add_space(14.0);
}

/// A tick centred on `c`, `s` wide.
fn check_mark(p: &egui::Painter, c: egui::Pos2, s: f32, color: egui::Color32) {
    let pts = vec![c + egui::vec2(-0.45, 0.0) * s, c + egui::vec2(-0.12, 0.32) * s, c + egui::vec2(0.45, -0.32) * s];
    p.add(egui::Shape::line(pts, egui::Stroke::new(2.0, color)));
}

/// A small padlock centred on `c`.
fn lock_icon(p: &egui::Painter, c: egui::Pos2, color: egui::Color32) {
    let body = egui::Rect::from_center_size(c + egui::vec2(0.0, 2.5), egui::vec2(11.0, 8.0));
    p.rect_filled(body, rad(2), color);
    let pts: Vec<egui::Pos2> = (0..=12)
        .map(|k| {
            let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
            c + egui::vec2(3.5 * a.cos(), -1.5 + 3.5 * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.6, color)));
}

/// A round avatar with the account's initial.
fn avatar(p: &egui::Painter, c: egui::Pos2, r: f32, name: &str) {
    p.circle_filled(c, r, pal().accent);
    let initial: String = name.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
    p.text(c, egui::Align2::CENTER_CENTER, disp(&initial), egui::FontId::proportional(r * 1.0), pal().on_accent);
}

/// The store's tile: its initial on its colour (not the store's logo).
fn store_mark(p: &egui::Painter, rect: egui::Rect, store: Store) {
    let (bg, letter) = match store {
        Store::Epic => (egui::Color32::from_rgb(0x2C, 0x2C, 0x33), "E"),
        Store::Gog => (egui::Color32::from_rgb(0x6C, 0x2D, 0xB8), "G"),
    };
    p.rect_filled(rect, rad(14), bg);
    p.rect_stroke(rect, rad(14), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(28)), egui::StrokeKind::Inside);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(rect.height() * 0.5), egui::Color32::WHITE);
}

/// The signed-in account in the header: avatar, name and a caret. Opens the
/// account panel.
fn account_chip(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let galley = ui.painter().layout_no_wrap(disp(name), font, pal().text);
    let size = egui::vec2(galley.size().x + 64.0, 34.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let p = ui.painter();
    let fill = if resp.hovered() || open { pal().hover } else { pal().card2 };
    p.rect_filled(rect, rad(255), fill);
    p.rect_stroke(rect, rad(255), egui::Stroke::new(1.0, if open { pal().accent } else { pal().border }), egui::StrokeKind::Inside);
    let (av, text_x, caret) = if rtl() {
        (rect.right() - 17.0, rect.right() - 36.0 - galley.size().x, rect.left() + 16.0)
    } else {
        (rect.left() + 17.0, rect.left() + 36.0, rect.right() - 16.0)
    };
    avatar(p, egui::pos2(av, rect.center().y), 12.0, name);
    p.galley(egui::pos2(text_x, rect.center().y - galley.size().y / 2.0), galley, pal().text);
    let cy = rect.center().y;
    let tri = vec![egui::pos2(caret - 4.0, cy - 2.0), egui::pos2(caret + 4.0, cy - 2.0), egui::pos2(caret, cy + 3.0)];
    p.add(egui::Shape::convex_polygon(tri, pal().subtext, egui::Stroke::NONE));
    resp
}
