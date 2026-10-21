use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod actions;
mod art;
mod artwork_ui;
mod cleanup;
mod couch;
mod emulation;
mod extras;
mod frame;
mod game_page;
mod gamepad;
mod glyphs;
mod games;
mod hltb;
mod home;
mod i18n;
mod i18n_ar;
mod icons;
mod launchers;
mod library;
mod library_list;
mod model;
mod net;
mod optiscaler;
mod proton;
mod quick;
mod rtl_edit;
mod runner;
mod safety;
mod saves;
mod saves_ui;
mod sessions;
mod settings_ui;
mod setup;
mod sgdb;
mod shaping;
mod stats_ui;
mod store;
mod stores;
mod stores_ui;
mod style;
mod theme;
mod tray;
mod widgets;
use i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr, trf};
use theme::{pal, rad};
use icons::*;
use model::*;
use net::*;
use runner::*;
use sgdb::*;
use style::*;
use glyphs::*;
use widgets::*;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Library tile sizes: (settings key, label, poster width). Posters are 2:3.
const TILE_SIZES: [(&str, &str, f32); 3] = [("small", "Small", 120.0), ("medium", "Medium", 150.0), ("large", "Large", 190.0)];

thread_local! {
    /// Poster width in the library grid, from the "tile size" setting.
    static TILE: std::cell::Cell<f32> = const { std::cell::Cell::new(150.0) };
}

fn tile_w() -> f32 {
    TILE.with(|t| t.get())
}

fn tile_h() -> f32 {
    tile_w() * 1.5
}

/// How many posters fit across `avail`.
fn grid_cols(avail: f32) -> f32 {
    ((avail + 20.0) / (tile_w() + 20.0)).floor().max(1.0)
}

/// The gap that spreads the columns across the full width instead of
/// leaving a ragged edge.
fn grid_gap(avail: f32) -> f32 {
    let cols = grid_cols(avail);
    if cols > 1.0 { ((avail - cols * tile_w()) / (cols - 1.0)).clamp(20.0, 72.0) } else { 20.0 }
}

/// Follow the "tile size" setting (unknown or unset = medium).
fn set_tile_size(key: &str) {
    let w = TILE_SIZES.iter().find(|(k, ..)| *k == key).map_or(150.0, |t| t.2);
    TILE.with(|t| t.set(w));
}

/// The small "add game" dialog shown after picking an executable.
struct AddModal {
    name: String,
    kind: GameKind,
    exe: String,
    proton_idx: usize,
}

/// A per-game action requested by the UI.
#[derive(Clone)]
enum GameAct {
    Play,
    Stop,
    Open,
    Artwork,
    Saves,
    Shortcut,
    CopyCommand,
    InitPrefix,
    Winecfg,
    OpenPrefix,
    OpenGameFolder,
    OpenLog,
    AskRemove,
    Remove,
    SetFlags(GameFlags),
    SetKind(GameKind),
    SetProton(usize),
    BrowseExe,
    Commit(Field),
    SetNoFixes(bool),
    SetUmuId(String),
    SetGamescope(GamescopeOpts),
    Favorite,
    SetStatus(GameStatus),
    SetTuning(Tuning),
    ToggleCollection(String),
}

/// A text field on the game page; saved when editing ends.
#[derive(Clone, Copy)]
enum Field {
    Name,
    Args,
    Workdir,
    Prefix,
    UmuId,
    Exe,
    Env,
}

/// Everything the UI asked for this frame, applied once drawing is done.
#[derive(Default)]
struct Acts {
    game: Vec<(String, GameAct)>,
    art: Vec<ArtCmd>,
    copy: Option<String>,
    dep: Option<DepAction>,
    rescan: bool,
    save_settings: bool,
    add: bool,
    fill_art: bool,
    back: bool,
    /// A URL to open in the browser.
    open: Option<String>,
    test_key: bool,
    clear_cache: bool,
    /// Switch to this page.
    goto: Option<Page>,
    /// Pick a folder and look for games in it.
    scan: bool,
    /// List Steam's non-Steam games for importing.
    steam_import: bool,
    lutris_import: bool,
    /// Open the new/rename collection dialog.
    collection: Option<CollectionEdit>,
    delete_collection: Option<String>,
}

/// The new/rename collection dialog.
#[derive(Clone, Default)]
struct CollectionEdit {
    /// A new collection gets this game added straight away.
    game: Option<String>,
    /// Renaming this collection (None = making a new one).
    renaming: Option<String>,
    name: String,
}

/// An install action offered in the Dependencies window.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DepAction {
    Proton,
    Umu,
}

/// One dependency row in the Dependencies window.
struct DepRow {
    name: &'static str,
    found: bool,
    detail: String,
    install_cmd: Option<String>,
    action: Option<(DepAction, &'static str)>,
}

/// An in-flight background download.
struct Download {
    label: String,
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
}

/// What a background task was for, so its completion can refresh the right things.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Store,
    Proton,
    OptiScaler,
    Saves,
    SaveDb,
    Components,
    Launcher,
}

/// A long-running background job (a download, an install, a save backup).
struct Task {
    /// Identifies what the task is about, e.g. "proton:GE-Proton11-9".
    key: String,
    label: String,
    kind: TaskKind,
    /// A file being downloaded and its expected size, to show progress.
    progress: Option<(PathBuf, u64)>,
    /// Progress reported by the work itself, in ten-thousandths.
    shared: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
}

impl Task {
    /// Download progress 0..1 (0 when unknown).
    fn fraction(&self) -> f32 {
        if let Some(s) = &self.shared {
            return s.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10_000.0;
        }
        match &self.progress {
            Some((p, total)) if *total > 0 => {
                let got = fs::metadata(p).map_or(0, |m| m.len());
                (got as f32 / *total as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

/// Replies from SteamGridDB worker threads.
enum ArtMsg {
    Games {
        generation: u64,
        result: Result<Vec<SgdbGame>, String>,
    },
    Assets {
        generation: u64,
        page: u32,
        result: Result<(Vec<SgdbAsset>, bool), String>,
    },
    Thumb {
        url: String,
        img: Option<image::RgbaImage>,
    },
    Applied {
        id: String,
        kind: ArtKind,
        result: Result<PathBuf, String>,
    },
    AutoFetched {
        id: String,
        result: Result<FetchedArt, String>,
    },
    KeyChecked(Result<(), String>),
}

/// A user action on the Artwork page, applied after the frame is drawn.
enum ArtCmd {
    Search,
    SelectMatch(u64),
    Kind(ArtKind),
    More,
    Pick(usize),
    Apply,
    Local(ArtKind),
    Reset(ArtKind),
    SaveKey,
}

/// State of the SteamGridDB artwork browser.
#[derive(Default)]
struct ArtState {
    /// Index into the game list.
    game: Option<usize>,
    query: String,
    matches: Vec<SgdbGame>,
    sgdb_id: Option<u64>,
    kind: ArtKind,
    assets: Vec<SgdbAsset>,
    page: u32,
    more: bool,
    picked: Option<usize>,
    /// Bumped per request so late replies to an older search are dropped.
    generation: u64,
    loading: bool,
    applying: bool,
    error: Option<String>,
    /// Key typed into the page's "key needed" box, saved on demand.
    key_input: String,
    thumbs: HashMap<String, egui::TextureHandle>,
    thumb_pending: HashSet<String>,
    thumb_failed: HashSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Page {
    Home,
    Launchers,
    Emulation,
    Stats,
    Library,
    Stores,
    Cleanup,
    Game,
    Artwork,
    Settings,
    Dependencies,
    Proton,
    Saves,
}

/// The sections of a game's page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GameTab {
    Overview,
    Settings,
    Mods,
}

/// Which games the library shows.
#[derive(Clone, PartialEq, Eq, Debug)]
enum LibFilter {
    All,
    Favorites,
    Unplayed,
    /// Games added by hand: not from a store, a launcher or a ROM folder.
    Mine,
    Status(GameStatus),
    Collection(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Recent,
    Name,
    Playtime,
}

impl SortMode {
    fn key(self) -> &'static str {
        match self {
            SortMode::Recent => "recent",
            SortMode::Name => "name",
            SortMode::Playtime => "playtime",
        }
    }

    fn from_key(k: &str) -> SortMode {
        match k {
            "name" => SortMode::Name,
            "playtime" => SortMode::Playtime,
            _ => SortMode::Recent,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SortMode::Recent => "Recently played",
            SortMode::Name => "Name",
            SortMode::Playtime => "Most played",
        }
    }
}

/// Text being edited on the game page, committed when a field loses focus.
#[derive(Clone, Default)]
struct EditBuf {
    id: String,
    name: String,
    args: String,
    workdir: String,
    prefix: String,
    umu_id: String,
    exe: String,
    env: String,
}

/// Eased (cubic-out) progress from `since`, 0→1 over `secs`.
fn ease_since(now: f64, since: f64, secs: f32) -> f32 {
    if secs <= 0.0 {
        return 1.0;
    }
    egui::emath::easing::cubic_out(((now - since) as f32 / secs).clamp(0.0, 1.0))
}

/// A value that eases from where it was towards a new target.
#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: f64,
}

impl Tween {
    fn at(v: f32, now: f64) -> Self {
        Tween { from: v, to: v, start: now }
    }

    fn value(&self, now: f64, secs: f32) -> f32 {
        self.from + (self.to - self.from) * ease_since(now, self.start, secs)
    }

    /// Retarget if `target` moved, then return the current value.
    fn follow(&mut self, target: f32, now: f64, secs: f32) -> f32 {
        if (target - self.to).abs() > 0.5 {
            self.from = self.value(now, secs);
            self.to = target;
            self.start = now;
        }
        self.value(now, secs)
    }

    fn settled(&self, now: f64, secs: f32) -> bool {
        self.from == self.to || now - self.start >= f64::from(secs)
    }
}

/// A poster flying between its grid slot and the game page.
struct Flight {
    id: String,
    from: egui::Rect,
    to_library: bool,
    since: f64,
    tex: egui::TextureHandle,
}

/// A removed game's poster, fading out where it stood in the grid.
struct Ghost {
    /// Relative to the grid origin.
    rel: egui::Rect,
    tex: Option<egui::TextureHandle>,
    since: f64,
}

/// Animation bookkeeping. Presentation only; nothing here is saved.
#[derive(Default)]
struct Motion {
    flight: Option<Flight>,
    /// Each poster's rect relative to the grid, and the grid's screen origin,
    /// as last drawn (where flights and ghosts start).
    tile_rel: HashMap<String, egui::Rect>,
    grid_origin: egui::Pos2,
    /// The game page poster as last drawn.
    hero_poster: Option<egui::Rect>,
    ghosts: Vec<Ghost>,
    /// Games added this session: they pop in the first time the grid shows them.
    fresh: HashMap<String, Option<f64>>,
    /// Failed actions shake their control (by game id).
    shake: HashMap<String, f64>,
    /// Play was pressed: a ring bursts from the poster's play button (by game id).
    ripple: HashMap<String, f64>,
    /// Playtime a just-ended session added: (old, new, count-up start). The
    /// count-up waits for the window to have focus, so it's actually seen.
    played: HashMap<String, (u64, u64, Option<f64>)>,
    focused: bool,
    /// Each poster's offset in the grid, eased when its slot moves.
    glide: HashMap<String, (Tween, Tween)>,
    /// When new cover art arrived (by game id), to fade it in over the old poster.
    art_arrived: HashMap<String, f64>,
    /// When each artwork-browser thumbnail loaded (by URL), to fade it in.
    thumb_loaded: HashMap<String, f64>,
    /// Games we started and are waiting on: (start time, runner pid).
    launching: HashMap<String, (f64, u32)>,
    /// The page drawn last frame, and when it changed.
    page: Option<Page>,
    page_since: f64,
    /// Sidebar highlight position.
    nav_y: Option<Tween>,
    /// Game page tab underline: left edge and width.
    tab_x: Option<(Tween, Tween)>,
    /// Something is mid-transition this frame: draw the next one right away.
    busy: bool,
    /// A looping effect (pulse, sweep) is on screen: keep it moving while focused.
    looping: bool,
}

struct App {
    steam_root: Option<PathBuf>,
    proton_tools: Vec<ProtonTool>,
    cfg: AppConfig,
    page: Page,
    /// Game shown on the game and artwork pages.
    selected: Option<String>,
    search: String,
    sort: SortMode,
    lib_filter: LibFilter,
    /// Keyboard focus in the library grid (a game id), shown while the arrow
    /// keys are in use; `lib_keys` turns off again when the mouse moves.
    lib_cursor: Option<String>,
    lib_keys: bool,
    lib_scroll: bool,
    stores: stores_ui::StoresState,
    cleanup: cleanup::CleanupState,
    /// For store worker threads to wake the UI.
    stores_ctx: Option<egui::Context>,
    /// The new/rename collection dialog, when open.
    collection_edit: Option<CollectionEdit>,
    /// Couch mode (fullscreen, controller-first), when it's on.
    couch: Option<couch::Couch>,
    /// Game controller reader, started with the window (not in tests).
    pad: Option<gamepad::Gamepad>,
    /// Points the right stick scrolls the page by this frame.
    pad_scroll: f32,
    status: String,
    status_err: bool,
    add_modal: Option<AddModal>,
    confirm_remove: Option<String>,
    edit: EditBuf,
    stats: Stats,
    /// Every session since tracking started (for Home and Stats).
    sessions: Vec<sessions::Session>,
    home: home::HomeState,
    /// The Ctrl+K quick search, while open.
    quick: Option<quick::Quick>,
    /// First-run setup, while showing.
    setup: Option<setup::Setup>,
    /// The tray icon and global shortcut (not in tests).
    tray: Option<tray::TrayHandle>,
    /// When the Launchers page last looked for new games (app time).
    launchers_scanned: f64,
    /// Launchers whose session just ended: look for games they installed.
    launchers_due: Vec<String>,
    /// The launcher being removed (confirm dialog).
    launcher_remove: Option<&'static str>,
    emu: emulation::EmuState,
    /// A banner: crash last time, or a damaged config restored.
    safety_notice: Option<String>,
    crash_at: Option<u64>,
    running: HashMap<String, RunMarker>,
    /// Games the user asked to stop, so their exit isn't reported as a crash.
    stopping: HashSet<String>,
    /// Child processes we started (runners, winecfg, xdg-open…), reaped so
    /// they don't linger as zombies.
    children: Vec<Child>,
    /// Dependency check results, refreshed on demand (it scans PATH and /etc).
    deps: Vec<DepRow>,
    last_scan: Instant,
    last_zoom: f32,
    download: Option<Download>,
    icon_tex: HashMap<String, egui::TextureHandle>,
    icon_fail: HashSet<String>,
    art: ArtState,
    art_tx: std::sync::mpsc::Sender<ArtMsg>,
    art_rx: std::sync::mpsc::Receiver<ArtMsg>,
    /// Games with an artwork auto-fetch in flight.
    auto_pending: HashSet<String>,
    motion: Motion,
    /// Frame time (seconds) for animations.
    now: f64,
    status_since: f64,
    /// A Ctrl +/− zoom change waiting to be saved (when it settles).
    zoom_save_at: Option<f64>,
    show_key: bool,
    key_checking: bool,
    key_check: Option<Result<(), String>>,
    /// Size of the image caches, measured when Settings opens.
    cache_bytes: Option<u64>,
    /// The palette last pushed to egui.
    applied_pal: Option<theme::Palette>,
    applied_lang: Option<String>,
    proton: proton::ProtonState,
    tasks: Vec<Task>,
    saves: saves_ui::SavesState,
    hltb: hltb::HltbState,
    store: store::StoreState,
    opti: optiscaler::OptiState,
    game_tab: GameTab,
    extras: extras::ExtrasState,
    scan: extras::ScanState,
    steam_import: Option<Vec<extras::SteamShortcut>>,
    /// The import checklist is showing Lutris games, not Steam's.
    import_lutris: bool,
}

impl App {
    fn new(steam_root: Option<PathBuf>) -> Self {
        let proton_tools = detect_proton_tools(steam_root.as_deref());
        let (cfg, restored) = load_config_noting();
        let (art_tx, art_rx) = std::sync::mpsc::channel();

        let mut app = App {
            steam_root,
            proton_tools,
            cfg,
            page: Page::Home,
            selected: None,
            search: String::new(),
            sort: SortMode::Recent,
            lib_filter: LibFilter::All,
            lib_cursor: None,
            lib_keys: false,
            lib_scroll: false,
            stores: stores_ui::StoresState::default(),
            cleanup: cleanup::CleanupState::default(),
            stores_ctx: None,
            collection_edit: None,
            couch: None,
            pad: None,
            pad_scroll: 0.0,
            status: String::new(),
            status_err: false,
            add_modal: None,
            confirm_remove: None,
            edit: EditBuf::default(),
            stats: load_stats(),
            sessions: sessions::load(),
            home: Default::default(),
            quick: None,
            setup: None,
            tray: None,
            launchers_scanned: -100.0,
            launchers_due: Vec::new(),
            launcher_remove: None,
            emu: Default::default(),
            safety_notice: None,
            crash_at: None,
            running: scan_running(),
            stopping: HashSet::new(),
            children: Vec::new(),
            deps: Vec::new(),
            last_scan: Instant::now(),
            last_zoom: 1.0,
            download: None,
            icon_tex: HashMap::new(),
            icon_fail: HashSet::new(),
            art: ArtState::default(),
            art_tx,
            art_rx,
            auto_pending: HashSet::new(),
            motion: Motion::default(),
            now: 0.0,
            status_since: 0.0,
            zoom_save_at: None,
            show_key: false,
            key_checking: false,
            key_check: None,
            cache_bytes: None,
            applied_pal: None,
            applied_lang: None,
            proton: proton::ProtonState::default(),
            tasks: Vec::new(),
            saves: saves_ui::SavesState::default(),
            hltb: hltb::HltbState::default(),
            store: store::StoreState::default(),
            opti: optiscaler::OptiState::default(),
            game_tab: GameTab::Overview,
            extras: extras::ExtrasState::default(),
            scan: extras::ScanState::default(),
            steam_import: None,
            import_lutris: false,
        };
        app.deps = app.dep_rows();
        app.sort = SortMode::from_key(&app.cfg.settings.library_sort);
        app.startup_notices(restored);
        if app.cfg.custom_games.is_empty() && !app.cfg.settings.setup_done {
            app.setup = Some(setup::Setup::default());
        }
        app
    }

    /// Whether dark mode is in effect (following the desktop when set to "system").
    fn dark_mode(&self, ctx: &egui::Context) -> bool {
        match self.cfg.settings.theme_mode.as_str() {
            "light" => false,
            "system" => ctx.system_theme().is_none_or(|t| t == egui::Theme::Dark),
            _ => true,
        }
    }

    /// Push the configured theme (and language) to egui when it changed.
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let s = &self.cfg.settings;
        if self.applied_lang.as_deref() != Some(s.language.as_str()) {
            i18n::set_lang(i18n::Lang::from_key(&s.language));
            self.applied_lang = Some(s.language.clone());
        }
        let dark = self.dark_mode(ctx);
        let key = if dark { &s.dark_theme } else { &s.light_theme };
        let p = theme::build(key, dark, &s.accent, s.square_corners);
        if self.applied_pal != Some(p) {
            theme::apply(ctx, p);
            self.applied_pal = Some(p);
        }
    }

    fn set_status(&mut self, msg: String, err: bool) {
        self.status = msg;
        self.status_err = err;
        self.status_since = self.now;
    }

    /// An animation length, or 0 with "Reduce motion" on.
    fn secs(&self, s: f32) -> f32 {
        if self.cfg.settings.reduce_motion { 0.0 } else { s }
    }

    /// Eased 0→1 progress of a transition that began at `since`.
    fn progress(&mut self, since: f64, secs: f32) -> f32 {
        let t = ease_since(self.now, since, self.secs(secs));
        self.motion.busy |= t < 1.0;
        t
    }

    /// A 0→1 sawtooth repeating every `period` seconds (0 with reduced motion).
    fn sweep(&mut self, period: f32) -> f32 {
        if self.cfg.settings.reduce_motion {
            return 0.0;
        }
        self.motion.looping = true;
        (self.now / f64::from(period)).fract() as f32
    }

    /// A slow 0..1 breathing value (steady 1 with reduced motion).
    fn pulse(&mut self, period: f32) -> f32 {
        if self.cfg.settings.reduce_motion {
            return 1.0;
        }
        0.5 - 0.5 * (self.sweep(period) * std::f32::consts::TAU).cos()
    }

    /// Start the poster's shared-element flight when moving between the grid
    /// and a game page (it flies from where it was last drawn).
    fn start_flight(&mut self, ctx: &egui::Context, prev: Option<Page>) {
        let Some(i) = self.selected_index() else {
            return;
        };
        let id = self.cfg.custom_games[i].id.clone();
        let (from, to_library) = match (prev, self.page) {
            (Some(Page::Library), Page::Game) => (
                self.motion.tile_rel.get(&id).map(|r| r.translate(self.motion.grid_origin.to_vec2())),
                false,
            ),
            (Some(Page::Game), Page::Library) => (self.motion.hero_poster, true),
            _ => return,
        };
        if self.cfg.settings.reduce_motion {
            return;
        }
        let Some(from) = from else {
            return;
        };
        if let Some(tex) = self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
            self.motion.fresh.remove(&id);
            self.motion.flight = Some(Flight { id, from, to_library, since: self.now, tex });
        }
    }

    /// Draw the in-flight poster heading for `target`. True while it's still in
    /// the air, so the destination leaves its own poster out.
    fn fly(&mut self, ctx: &egui::Context, id: &str, to_library: bool, target: egui::Rect) -> bool {
        let Some(f) = self.motion.flight.as_ref().filter(|f| f.id == id && f.to_library == to_library) else {
            return false;
        };
        let raw = ((self.now - f.since) as f32 / 0.42).clamp(0.0, 1.0);
        if raw >= 1.0 {
            self.motion.flight = None;
            return false;
        }
        let t = egui::emath::easing::cubic_in_out(raw);
        // Picked up: it rises in an arc and grows a little on the way.
        let lift = (raw * std::f32::consts::PI).sin();
        let r = f.from.lerp_towards(&target, t);
        let r = egui::Rect::from_center_size(r.center() - egui::vec2(0.0, 26.0 * lift), r.size() * (1.0 + 0.07 * lift));
        // Above the pages, below dialogs and menus.
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::unique("poster-flight"),
        ));
        // Lifted mid-flight: the shadow is deepest halfway.
        painter.rect_filled(
            r.translate(egui::vec2(0.0, 6.0 + 10.0 * lift)).expand(2.0),
            rad(10),
            theme::shadow(60.0 + 90.0 * lift),
        );
        painter.add(cover_fill_shape(&f.tex, r, 10, 1.0));
        self.motion.busy = true;
        true
    }

    /// The Play button's state for a game, and its working-animation phase.
    fn play_state(&mut self, id: &str) -> (PlayState, f32) {
        let state = match (self.running.contains_key(id), self.stopping.contains(id)) {
            (true, true) => PlayState::Stopping,
            (true, false) => PlayState::Stop,
            (false, _) if self.motion.launching.contains_key(id) => PlayState::Starting,
            (false, _) => PlayState::Play,
        };
        let phase = match state {
            PlayState::Starting | PlayState::Stopping => self.sweep(1.2),
            _ => 0.0,
        };
        (state, phase)
    }

    /// Horizontal offset for a "no" shake after a failed action (0 when idle).
    fn shake_dx(&mut self, id: &str) -> f32 {
        let Some(&since) = self.motion.shake.get(id) else {
            return 0.0;
        };
        let el = (self.now - since) as f32;
        if el >= 0.45 || self.cfg.settings.reduce_motion {
            self.motion.shake.remove(id);
            return 0.0;
        }
        self.motion.busy = true;
        let t = el / 0.45;
        (t * std::f32::consts::PI * 6.0).sin() * 8.0 * (1.0 - t)
    }

    /// Playtime to show: after a session it counts up from the old total, with a
    /// fading "+25m". Returns (seconds to show, Some((delta text, alpha))).
    fn shown_playtime(&mut self, id: &str, actual: u64) -> (u64, Option<(String, f32)>) {
        let Some(&(old, new, start)) = self.motion.played.get(id) else {
            return (actual, None);
        };
        if self.cfg.settings.reduce_motion {
            self.motion.played.remove(id);
            return (actual, None);
        }
        let start = match start {
            Some(s) => s,
            None if self.motion.focused => {
                let s = self.now + 0.35;
                self.motion.played.insert(id.to_string(), (old, new, Some(s)));
                s
            }
            None => return (old, None),
        };
        let el = (self.now - start) as f32;
        if el > 3.6 {
            self.motion.played.remove(id);
            return (actual, None);
        }
        self.motion.busy = true;
        let shown = old + ((new - old) as f32 * ease_since(self.now, start, 1.3)) as u64;
        let alpha = (1.0 - (el - 2.3) / 1.3).clamp(0.0, 1.0);
        (shown, Some((format!("+{}", format_duration(new - old)), alpha)))
    }

    /// Where to draw a grid slot: it glides to its new place when the slot moves.
    /// Positions are relative to the grid, so scrolling doesn't count as moving.
    fn glide(&mut self, key: &str, slot: egui::Rect, origin: egui::Pos2) -> egui::Rect {
        let (now, secs) = (self.now, self.secs(0.32));
        let rel = slot.min - origin;
        let g = self
            .motion
            .glide
            .entry(key.to_string())
            .or_insert_with(|| (Tween::at(rel.x, now), Tween::at(rel.y, now)));
        let pos = origin + egui::vec2(g.0.follow(rel.x, now, secs), g.1.follow(rel.y, now, secs));
        self.motion.busy |= !(g.0.settled(now, secs) && g.1.settled(now, secs));
        egui::Rect::from_min_size(pos, slot.size())
    }

    /// Start a background download/install job and show progress in the status bar.
    fn start_download<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.set_status(format!("{label}…"), false);
        self.download = Some(Download {
            label: label.to_string(),
            rx,
        });
    }

    /// Run `work` on a thread as a tracked task (shown with progress until done).
    fn start_task<F>(&mut self, kind: TaskKind, key: &str, label: &str, progress: Option<(PathBuf, u64)>, work: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(work());
        });
        self.set_status(format!("{label}…"), false);
        self.tasks.push(Task { key: key.to_string(), label: label.to_string(), kind, progress, shared: None, rx });
    }

    /// Collect finished tasks and report them.
    fn poll_tasks(&mut self, ctx: &egui::Context) {
        if self.tasks.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let mut done = Vec::new();
        self.tasks.retain(|t| match t.rx.try_recv() {
            Ok(r) => {
                done.push((t.kind, t.label.clone(), r));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                done.push((t.kind, t.label.clone(), Err(lg("the task ended unexpectedly").into())));
                false
            }
        });
        for (kind, label, r) in done {
            if kind == TaskKind::Proton {
                self.refresh();
            }
            self.task_done(kind);
            match r {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(format!("{label}: {e}"), true),
            }
        }
    }

    /// Refresh whatever a finished task of this kind may have changed.
    fn task_done(&mut self, kind: TaskKind) {
        match kind {
            TaskKind::Saves => {
                self.saves.view = None;
                self.saves.summary.clear();
            }
            TaskKind::SaveDb => {
                self.saves.index = None;
                self.saves.index_loading = false;
                self.saves.view = None;
                self.saves.summary.clear();
            }
            TaskKind::Proton | TaskKind::OptiScaler | TaskKind::Components | TaskKind::Store => {}
            TaskKind::Launcher => self.launchers_scanned = -100.0,
        }
    }

    /// Current state of every external tool the app relies on.
    fn dep_rows(&self) -> Vec<DepRow> {
        let s = &self.cfg.settings;
        let mut rows = Vec::new();

        let proton_ok = !self.proton_tools.is_empty();
        rows.push(DepRow {
            name: "Proton (GE-Proton)",
            found: proton_ok,
            detail: if proton_ok {
                let extra = self.proton_tools.len().saturating_sub(1);
                if extra > 0 {
                    format!("{}  (+{extra} more)", self.proton_tools[0].label)
                } else {
                    self.proton_tools[0].label.clone()
                }
            } else {
                "no Proton build found — needed to run Windows games".into()
            },
            install_cmd: None,
            action: (!proton_ok).then_some((DepAction::Proton, "Download latest")),
        });

        let umu = s.umu_bin();
        rows.push(DepRow {
            name: "umu-run",
            found: umu.is_some(),
            detail: match &umu {
                Some(p) => p.display().to_string(),
                None => "not found — runs Windows games outside Steam".into(),
            },
            install_cmd: None,
            action: umu.is_none().then_some((DepAction::Umu, "Download latest")),
        });

        let py = find_in_path("python3");
        rows.push(DepRow {
            name: "Python 3",
            found: py.is_some(),
            detail: match &py {
                Some(p) => p.display().to_string(),
                None => "not found — required by the umu-run zipapp".into(),
            },
            install_cmd: install_command(&["python3"]),
            action: None,
        });

        let mh = find_in_path(s.mangohud_bin());
        rows.push(DepRow {
            name: "MangoHud",
            found: mh.is_some(),
            detail: match &mh {
                Some(p) => p.display().to_string(),
                None => "not found — in-game performance overlay".into(),
            },
            install_cmd: install_command(&["mangohud"]),
            action: None,
        });

        let gm = find_in_path(s.gamemoderun_bin());
        rows.push(DepRow {
            name: "GameMode",
            found: gm.is_some(),
            detail: match &gm {
                Some(p) => p.display().to_string(),
                None => "not found — CPU/GPU performance tweaks".into(),
            },
            install_cmd: install_command(&["gamemode"]),
            action: None,
        });

        let sz = ["7z", "7za", "7zz", "bsdtar"].into_iter().find_map(find_in_path);
        let sz_pkg = match detect_pkgmgr() {
            PkgMgr::Pacman | PkgMgr::Apt => "7zip",
            _ => "p7zip",
        };
        rows.push(DepRow {
            name: "7-Zip",
            found: sz.is_some(),
            detail: match &sz {
                Some(p) => p.display().to_string(),
                None => "not found — unpacks OptiScaler downloads".into(),
            },
            install_cmd: install_command(&[sz_pkg]),
            action: None,
        });

        let picker = ["zenity", "kdialog", "yad"]
            .into_iter()
            .find_map(|n| find_in_path(n).map(|p| (n, p)));
        rows.push(DepRow {
            name: "File picker",
            found: picker.is_some(),
            detail: match &picker {
                Some((n, p)) => format!("{n}  ({})", p.display()),
                None => "not found — used to browse for game executables".into(),
            },
            install_cmd: install_command(&["zenity"]),
            action: None,
        });

        rows
    }

    fn refresh(&mut self) {
        self.steam_root = find_steam_root();
        self.proton_tools = detect_proton_tools(self.steam_root.as_deref());
        self.deps = self.dep_rows();
        self.stats = load_stats();
        self.sessions = sessions::load();
        self.running = scan_running();
        self.set_status(lg("Refreshed").into(), false);
    }

    fn copy_text(&mut self, text: &str) {
        match arboard::Clipboard::new() {
            Ok(mut cb) => match cb.set_text(text.to_string()) {
                Ok(_) => self.set_status(lfmt("Copied: {}", &[&text]), false),
                Err(e) => self.set_status(lfmt("Clipboard error: {}", &[&e]), true),
            },
            Err(e) => self.set_status(lfmt("Clipboard unavailable: {}", &[&e]), true),
        }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.cfg.custom_games.iter().position(|g| g.id == id)
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected.as_deref().and_then(|id| self.index_of(id))
    }

    /// A generated stand-in poster for game `i` (used when it has no cover).
    fn game_generated_poster(
        &mut self,
        ctx: &egui::Context,
        i: usize,
    ) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        let key = format!("gen-{}", g.id);
        let (seed, icon, exe, windows) = (
            g.name.clone(),
            g.icon.clone(),
            g.exe.clone(),
            g.kind == GameKind::Windows,
        );
        self.get_tex(ctx, &key, || {
            let ic = if !icon.is_empty() && Path::new(&icon).is_file() {
                decode_image(Path::new(&icon))
            } else if windows {
                extract_exe_icon(Path::new(&exe))
            } else {
                None
            };
            Some(generated_poster(ic.as_ref(), &seed))
        })
    }

}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    /// Controller input becomes key presses before the frame sees its input.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.cfg.settings.no_gamepad {
            self.pad = None;
            return;
        }
        let pad = self.pad.get_or_insert_with(|| gamepad::Gamepad::start(ctx.clone()));
        let now = raw.time.unwrap_or(0.0);
        pad.drain(now);
        let (keys, scroll) = pad.nav.frame(now);
        // Only while Gamelo is in front: a running game has the controller.
        if !raw.focused {
            self.pad_scroll = 0.0;
            return;
        }
        for key in keys {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() });
            }
        }
        self.pad_scroll = scroll * raw.predicted_dt;
        if pad.nav.active() {
            ctx.request_repaint();
        }
    }
}

fn print_summary() {
    let steam_root = find_steam_root();
    println!(
        "Steam root : {}",
        steam_root
            .as_ref()
            .map_or("<none> (optional)".into(), |p| p.display().to_string())
    );

    println!("\nProton tools:");
    for t in detect_proton_tools(steam_root.as_deref()) {
        println!("  - {}  ({})", t.label, t.dir.display());
    }

    match find_umu() {
        Some(p) => println!("\numu-run : {}", p.display()),
        None => println!("\numu-run : not found (Windows games will not launch)"),
    }

    let cfg = load_config();
    println!("\nGames ({}):", cfg.custom_games.len());
    for g in &cfg.custom_games {
        let kind = match g.kind {
            GameKind::Windows => "windows",
            GameKind::Linux => "linux",
        };
        let cmd = launch_command_string(g, &cfg.settings)
            .unwrap_or_else(|e| format!("<error: {e}>"));
        println!("  {:24} [{}]  {}", g.name, kind, cmd);
    }
}

fn main() -> eframe::Result {
    safety::install_crash_log();
    migrate_old_name();
    let steam_root = find_steam_root();
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--run") {
        let Some(id) = args.get(pos + 1) else {
            eprintln!("usage: gamelo --run <game-id>");
            std::process::exit(2);
        };
        std::process::exit(run_headless(id));
    }
    if args.iter().any(|a| a == "--list") {
        print_summary();
        return Ok(());
    }
    if args.iter().any(|a| a == "--install-proton") {
        match install_ge_proton(steam_root.clone()) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("gamelo: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--install-umu") {
        match install_umu() {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("gamelo: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let app = App::new(steam_root);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("Gamelo")
            .with_app_id("gamelo")
            .with_icon({
                let img = icon_rgba();
                let (width, height) = img.dimensions();
                Arc::new(egui::IconData { rgba: img.into_raw(), width, height })
            }),
        ..Default::default()
    };
    eframe::run_native(
        "gamelo",
        options,
        Box::new(move |cc| {
            configure_style(&cc.egui_ctx);
            let mut app = app;
            app.tray = Some(tray::TrayHandle::start(&cc.egui_ctx));
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests;
