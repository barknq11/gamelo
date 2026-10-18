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

mod cleanup;
mod couch;
mod extras;
mod gamepad;
mod hltb;
mod home;
mod i18n;
mod library_list;
mod proton;
mod quick;
mod saves;
mod saves_ui;
mod sessions;
mod setup;
mod stats_ui;
mod i18n_ar;
mod optiscaler;
mod rtl_edit;
mod shaping;
mod stores;
mod stores_ui;
mod store;
mod theme;
mod tray;
use i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr, trf};
use theme::{pal, rad};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

// ---------------------------------------------------------------------------
// Steam install (only used to locate compatibilitytools.d for Proton builds)
// ---------------------------------------------------------------------------

fn find_steam_root() -> Option<PathBuf> {
    let h = home();
    let candidates = [
        h.join(".local/share/Steam"),
        h.join(".steam/steam"),
        h.join(".steam/root"),
    ];
    for c in candidates {
        if c.join("steamapps").is_dir() {
            return Some(c.canonicalize().unwrap_or(c));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Flags model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
enum Fsr4Mode {
    #[default]
    Off,
    Upgrade,
    Rrna3,
}

impl Fsr4Mode {
    fn label(&self) -> &'static str {
        match self {
            Fsr4Mode::Off => "FSR4: Off",
            Fsr4Mode::Upgrade => "FSR4: Upgrade",
            Fsr4Mode::Rrna3 => "FSR4: RDNA3",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
struct GameFlags {
    mangohud: bool,
    fsr4: Fsr4Mode,
    watermark: bool,
    ntsync: bool,
    wayland: bool,
    gamemode: bool,
    /// Turn off umu-protonfixes for this game.
    no_protonfixes: bool,
    /// Run inside gamescope (Valve's micro-compositor).
    gamescope: bool,
}

/// How gamescope runs a game.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
struct GamescopeOpts {
    /// Output size; 0 = the monitor's.
    width: u32,
    height: u32,
    /// Size the game renders at (upscaled to the output); 0 = same as output.
    game_width: u32,
    game_height: u32,
    /// Refresh / frame limit; 0 = unlimited.
    refresh: u32,
    fullscreen: bool,
    /// "", "fsr", "nis", "pixel" or "linear".
    filter: String,
    hdr: bool,
    /// Extra command-line options.
    extra: String,
}

impl GamescopeOpts {
    /// gamescope's options, up to (not including) the `--` before the game.
    fn args(&self, mangoapp: bool) -> Vec<String> {
        let mut a = Vec::new();
        let mut num = |flag: &str, v: u32| {
            if v > 0 {
                a.push(flag.to_string());
                a.push(v.to_string());
            }
        };
        num("-W", self.width);
        num("-H", self.height);
        num("-w", self.game_width);
        num("-h", self.game_height);
        num("-r", self.refresh);
        if self.fullscreen {
            a.push("-f".into());
        }
        if !self.filter.is_empty() {
            a.push("-F".into());
            a.push(self.filter.clone());
        }
        if self.hdr {
            a.push("--hdr-enabled".into());
        }
        if mangoapp {
            a.push("--mangoapp".into());
        }
        a.extend(tokenize(&self.extra));
        a
    }
}

/// Parse "KEY=value" lines (blank lines and # comments ignored).
fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.trim().trim_start_matches("export ").trim();
            let ok = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            ok.then(|| (k.to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        match c {
            '"' => in_q = !in_q,
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Frame cap and MangoHud look for one game.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Debug)]
struct Tuning {
    /// Frames per second; 0 = no cap.
    #[serde(default)]
    fps_limit: u32,
    /// "" (MangoHud's own config), "minimal", "standard", "full" or "custom".
    #[serde(default)]
    overlay: String,
    /// MangoHud position ("top-left", "top-right"…); "" = its default.
    #[serde(default)]
    position: String,
    /// Custom MangoHud options, comma or line separated.
    #[serde(default)]
    custom: String,
}

/// Environment for a game's tuning. With MangoHud on and a preset chosen, it
/// all goes into MANGOHUD_CONFIG (whose limiter works for every API).
/// Otherwise the cap uses DXVK's and VKD3D's own limiters, so a MangoHud.conf
/// the user keeps is never overridden.
fn tuning_envs(t: &Tuning, mangohud: bool) -> Vec<(String, String)> {
    let custom = t.custom.replace('\n', ",").split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(",");
    let preset = match t.overlay.as_str() {
        "minimal" => Some("fps_only".to_string()),
        "standard" => Some("fps,frame_timing,cpu_stats,gpu_stats".to_string()),
        "full" => Some("fps,frame_timing,cpu_stats,cpu_temp,cpu_mhz,gpu_stats,gpu_temp,gpu_core_clock,vram,ram".to_string()),
        "custom" if !custom.is_empty() => Some(custom),
        _ => None,
    };
    let mut v = Vec::new();
    match preset.filter(|_| mangohud) {
        Some(mut cfg) => {
            if !t.position.is_empty() && !cfg.contains("position=") {
                cfg.push_str(&format!(",position={}", t.position));
            }
            if t.fps_limit > 0 && !cfg.contains("fps_limit=") {
                cfg.push_str(&format!(",fps_limit={}", t.fps_limit));
            }
            v.push(("MANGOHUD_CONFIG".to_string(), cfg));
        }
        None if t.fps_limit > 0 => {
            v.push(("DXVK_FRAME_RATE".to_string(), t.fps_limit.to_string()));
            v.push(("VKD3D_FRAME_RATE".to_string(), t.fps_limit.to_string()));
        }
        None => {}
    }
    v
}

fn flag_envs(f: &GameFlags) -> Vec<(&'static str, &'static str)> {
    let mut v = Vec::new();
    match f.fsr4 {
        Fsr4Mode::Off => {}
        Fsr4Mode::Upgrade => v.push(("PROTON_FSR4_UPGRADE", "1")),
        Fsr4Mode::Rrna3 => v.push(("PROTON_FSR4_RDNA3_UPGRADE", "1")),
    }
    if f.watermark {
        v.push(("PROTON_FSR4_INDICATOR", "1"));
    }
    if f.ntsync {
        v.push(("PROTON_USE_NTSYNC", "1"));
    }
    if f.wayland {
        v.push(("PROTON_ENABLE_WAYLAND", "1"));
    }
    if f.no_protonfixes {
        v.push(("PROTONFIXES_DISABLE", "1"));
    }
    v
}

// ---------------------------------------------------------------------------
// Proton tools + custom (non-Steam) games
// ---------------------------------------------------------------------------

struct ProtonTool {
    label: String,
    dir: PathBuf,
}

/// Directories that may contain Proton builds (GE-Proton etc.).
fn proton_search_roots(steam_root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(sr) = steam_root {
        roots.push(sr.join("compatibilitytools.d"));
    }
    roots.push(home().join(".local/share/Steam/compatibilitytools.d"));
    roots.push(home().join(".steam/steam/compatibilitytools.d"));
    roots.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
    roots.push(PathBuf::from("/usr/share/proton"));
    roots.push(PathBuf::from("/usr/share/proton-cachyos"));
    roots.push(PathBuf::from("/usr/lib/proton"));
    // Builds downloaded by us land here.
    roots.push(data_dir().join("proton"));
    roots
}

fn detect_proton_tools(steam_root: Option<&Path>) -> Vec<ProtonTool> {
    let mut roots: Vec<PathBuf> = proton_search_roots(steam_root);
    if let Ok(rd) = fs::read_dir(home()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if (n.starts_with("GE-Proton") || n.starts_with("Proton-")) && e.path().is_dir() {
                roots.push(e.path());
            }
        }
    }
    let mut out: Vec<ProtonTool> = Vec::new();
    for d in roots {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join("proton").is_file() {
                out.push(ProtonTool {
                    label: e.file_name().to_string_lossy().to_string(),
                    dir: p,
                });
            }
        }
        // a directory that itself is a proton tool (e.g. /usr/share/proton/<ver> handled above)
        if d.join("proton").is_file() {
            out.push(ProtonTool {
                label: d
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| d.display().to_string()),
                dir: d,
            });
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out.dedup_by(|a, b| a.dir == b.dir || a.label == b.label);
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
enum GameKind {
    /// Windows executable run through a Proton layer.
    #[default]
    Windows,
    /// Native Linux executable, run directly.
    Linux,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CustomGame {
    /// Stable identifier used by shortcuts, stats and running markers.
    #[serde(default)]
    id: String,
    name: String,
    #[serde(default)]
    kind: GameKind,
    exe: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    workdir: String,
    /// Proton tool directory (Windows games only).
    #[serde(default)]
    proton_dir: String,
    /// Custom prefix path; empty = auto under the prefix base dir.
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    flags: GameFlags,
    /// umu GAMEID; empty = `umu-<slug>`. A umu-database ID enables protonfixes.
    #[serde(default)]
    umu_id: String,
    /// Icon image path; empty = the exe's embedded icon.
    #[serde(default)]
    icon: String,
    /// Cover (600×900 poster) image path; empty = none.
    #[serde(default)]
    cover: String,
    /// Hero (wide banner) image path for the game page; empty = blurred cover.
    #[serde(default)]
    hero: String,
    /// SteamGridDB auto-fetch has already been tried for this game.
    #[serde(default)]
    art_checked: bool,
    /// Extra environment variables, one KEY=value per line.
    #[serde(default)]
    env: String,
    #[serde(default)]
    gamescope_opts: GamescopeOpts,
    /// Starred: listed first in the library.
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    status: GameStatus,
    /// Names of the collections the game is in (see `Settings::collections`).
    #[serde(default)]
    collections: Vec<String>,
    #[serde(default)]
    tuning: Tuning,
    /// Installed from Epic or GOG: launched with fresh online parameters and
    /// cloud saves synced around each session.
    #[serde(default)]
    store: Option<stores::StoreLink>,
    /// Unix time it was added (0 = before this was recorded).
    #[serde(default)]
    added: u64,
}

/// Where a game stands: set by hand, except that launching an unsorted or
/// backlog game makes it Playing.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
enum GameStatus {
    #[default]
    None,
    Playing,
    Backlog,
    Finished,
    Dropped,
}

impl GameStatus {
    const ALL: [GameStatus; 4] = [GameStatus::Playing, GameStatus::Backlog, GameStatus::Finished, GameStatus::Dropped];

    fn label(self) -> &'static str {
        match self {
            GameStatus::None => "No status",
            GameStatus::Playing => "Playing",
            GameStatus::Backlog => "Backlog",
            GameStatus::Finished => "Finished",
            GameStatus::Dropped => "Dropped",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            GameStatus::None => pal().faint,
            GameStatus::Playing => pal().ok,
            GameStatus::Backlog => egui::Color32::from_rgb(0x6c, 0x9e, 0xf8),
            GameStatus::Finished => GOLD,
            GameStatus::Dropped => pal().danger,
        }
    }

    /// The next one round the cycle (for a single button that steps through them).
    fn next(self) -> GameStatus {
        match self {
            GameStatus::None => GameStatus::Playing,
            GameStatus::Playing => GameStatus::Backlog,
            GameStatus::Backlog => GameStatus::Finished,
            GameStatus::Finished => GameStatus::Dropped,
            GameStatus::Dropped => GameStatus::None,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Settings {
    /// Base directory for auto prefixes (empty = ~/.local/share/gamelo/prefixes).
    #[serde(default)]
    prefix_base: String,
    #[serde(default)]
    mangohud_bin: String,
    #[serde(default)]
    gamemoderun_bin: String,
    /// Path to umu-run (empty = search PATH).
    #[serde(default)]
    umu_bin: String,
    /// UI zoom factor (1.0 = default). 0 = unset.
    #[serde(default)]
    zoom: f32,
    /// Optional SteamGridDB API key, used to fetch game artwork.
    #[serde(default)]
    steamgriddb_key: String,
    /// Turn off animations.
    #[serde(default)]
    reduce_motion: bool,
    /// Let games use VR (Wine's OpenXR bridge). Off by default: on systems
    /// without a working OpenXR runtime the bridge can crash DXVK as a game
    /// starts (it probes OpenXR for extensions even in flat games).
    #[serde(default)]
    allow_openxr: bool,
    /// Don't navigate with a game controller.
    #[serde(default)]
    no_gamepad: bool,
    /// Where Epic and GOG games are installed (empty = ~/Games/Epic, ~/Games/GOG).
    #[serde(default)]
    epic_dir: String,
    #[serde(default)]
    gog_dir: String,
    /// The user's collections, in the order they're shown.
    #[serde(default)]
    collections: Vec<String>,
    /// Library layout: "grid" (default) or "list".
    #[serde(default)]
    library_view: String,
    /// Library tile size: "small", "medium" (default) or "large".
    #[serde(default)]
    tile_size: String,
    /// Library sort: "recent" (default), "name" or "playtime".
    #[serde(default)]
    library_sort: String,
    /// Don't download artwork automatically when a game is added.
    #[serde(default)]
    skip_auto_fetch: bool,
    /// First-run setup was finished or skipped.
    #[serde(default)]
    setup_done: bool,
    /// Performance flags new games start with (None = MangoHud + FSR4 upgrade).
    #[serde(default)]
    new_flags: Option<GameFlags>,
    /// Proton build new games use (empty = the one the last added game uses).
    #[serde(default)]
    default_proton: String,
    /// "dark", "light" or "system" (empty = dark).
    #[serde(default)]
    theme_mode: String,
    /// Theme keys used in dark and in light mode (empty = the first of each).
    #[serde(default)]
    dark_theme: String,
    #[serde(default)]
    light_theme: String,
    /// Accent colour key replacing the theme's own (empty = the theme's).
    #[serde(default)]
    accent: String,
    #[serde(default)]
    square_corners: bool,
    /// Interface language: "en", "ar", or empty to follow the desktop locale.
    #[serde(default)]
    language: String,
    /// Folder each game's saves are mirrored to (Syncthing, Nextcloud, a
    /// shared drive…); empty = off.
    #[serde(default)]
    save_sync_dir: String,
    /// How many save backups to keep per game (0 = 10).
    #[serde(default)]
    save_keep: u32,
    #[serde(default)]
    save_no_auto_backup: bool,
    /// Don't look games up on HowLongToBeat.
    #[serde(default)]
    no_hltb: bool,
    /// Don't show Steam store info (description, screenshots…) on game pages.
    #[serde(default)]
    no_store: bool,
}

/// UI scale presets offered in Settings (Ctrl +/− and Ctrl+scroll can still land between them).
const ZOOM_STEPS: [f32; 8] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

#[derive(Default, Serialize, Deserialize)]
struct AppConfig {
    #[serde(default)]
    custom_games: Vec<CustomGame>,
    #[serde(default)]
    settings: Settings,
}

fn config_path() -> PathBuf {
    home().join(".config/gamelo/config.json")
}

fn load_config() -> AppConfig {
    let mut cfg: AppConfig = fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Games saved before ids existed get one now.
    let mut changed = false;
    for g in &mut cfg.custom_games {
        if g.id.is_empty() {
            g.id = new_game_id();
            changed = true;
        }
    }
    if changed {
        save_config(&cfg);
    }
    cfg
}

/// A short random hex id for a new game.
fn new_game_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 6];
    let random = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b));
    if random.is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mixed = nanos ^ (u128::from(std::process::id()) << 64);
        b.copy_from_slice(&mixed.to_le_bytes()[..6]);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Playtime stats (separate from the config so headless runs never clobber it)
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize, Deserialize)]
struct GameStats {
    #[serde(default)]
    playtime_secs: u64,
    /// Unix time the last session started.
    #[serde(default)]
    last_played: u64,
    #[serde(default)]
    last_session_secs: u64,
    /// Exit code of the last session (None = killed by a signal).
    #[serde(default)]
    last_exit: Option<i32>,
    /// Sessions counted since this field was added.
    #[serde(default)]
    sessions: u32,
}

type Stats = HashMap<String, GameStats>;

fn load_stats_in(dir: &Path) -> Stats {
    fs::read_to_string(dir.join("stats.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn load_stats() -> Stats {
    load_stats_in(&data_dir())
}

/// Add a finished session to `dir`/stats.json. Locked, so two games ending at
/// once can't overwrite each other's update.
fn record_session_in(dir: &Path, id: &str, started: u64, secs: u64, exit: Option<i32>) {
    let _ = fs::create_dir_all(dir);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("stats.lock"));
    if let Ok(f) = &lock {
        unsafe {
            libc::flock(f.as_raw_fd(), libc::LOCK_EX);
        }
    }
    let mut stats = load_stats_in(dir);
    let s = stats.entry(id.to_string()).or_default();
    s.playtime_secs += secs;
    s.last_played = started;
    s.last_session_secs = secs;
    s.last_exit = exit;
    s.sessions += 1;
    if secs > 0 {
        sessions::append(dir, &sessions::Session { id: id.to_string(), start: started, secs });
    }
    let path = dir.join("stats.json");
    let tmp = dir.join("stats.json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(&stats).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
    // The lock is released when `lock` is dropped.
}

/// Total size of the files under `dir`.
fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

/// "12.3 MB", "640 KB".
fn format_bytes(n: u64) -> String {
    match n {
        0..1_000_000 => format!("{} KB", n.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}

/// "12h 40m", "25m", "40s".
fn format_duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    // Logical text: callers draw it through tr/disp.
    if h > 0 {
        lfmt("{}h {}m", &[&h, &m])
    } else if m > 0 {
        lfmt("{}m", &[&m])
    } else {
        lfmt("{}s", &[&secs])
    }
}

/// "today", "yesterday", "3 days ago", "5 weeks ago", "2 years ago".
fn format_ago(then: u64, now: u64) -> String {
    let days = now.saturating_sub(then) / 86_400;
    match days {
        0 => lg("today").into(),
        1 => lg("yesterday").into(),
        2..=13 => lfmt("{} days ago", &[&days]),
        14..=59 => lfmt("{} weeks ago", &[&(days / 7)]),
        60..=729 => lfmt("{} months ago", &[&(days / 30)]),
        _ => lfmt("{} years ago", &[&(days / 365)]),
    }
}

// ---------------------------------------------------------------------------
// Running games: marker files written by the headless runner
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
struct RunMarker {
    id: String,
    name: String,
    runner_pid: u32,
    game_pid: u32,
    started: u64,
    log: String,
}

fn running_dir() -> PathBuf {
    data_dir().join("running")
}

fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Live running markers by game id; markers whose runner is gone are removed.
fn scan_running() -> HashMap<String, RunMarker> {
    let mut out = HashMap::new();
    let Ok(rd) = fs::read_dir(running_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(m) = fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str::<RunMarker>(&s).ok())
        else {
            continue;
        };
        if pid_alive(m.runner_pid) {
            out.insert(m.id.clone(), m);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    out
}

/// `root` and all of its descendants, from the parent pids in /proc/*/stat.
fn process_tree(root: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(rd) = fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            // The command name may contain spaces or parens; ppid is the 2nd field after the last ')'.
            let ppid = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u32>().ok());
            if let Some(ppid) = ppid {
                children.entry(ppid).or_default().push(pid);
            }
        }
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(c) = children.get(&out[i]) {
            out.extend(c.iter().copied());
        }
        i += 1;
    }
    out
}

/// Stop a running game: SIGTERM its whole process tree, shut down its wine
/// server, then SIGKILL anything still alive after a grace period.
/// Returns the `wineserver -k` process, if one was started, so it can be reaped.
fn stop_game(marker: &RunMarker, game: Option<&CustomGame>, settings: &Settings) -> Option<Child> {
    let tree = process_tree(marker.game_pid);
    for &pid in &tree {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    let wineserver = game
        .filter(|g| g.kind == GameKind::Windows)
        .and_then(|g| {
            let ws = ["files/bin/wineserver", "dist/bin/wineserver"]
                .iter()
                .map(|rel| Path::new(&g.proton_dir).join(rel))
                .find(|p| p.is_file())?;
            Command::new(ws)
                .arg("-k")
                .env("WINEPREFIX", game_prefix(g, settings))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
        });
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        for pid in tree {
            if pid_alive(pid) {
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
            }
        }
    });
    wineserver
}

/// What to re-invoke for headless runs: the AppImage file when packaged (its
/// mount disappears once the window closes), else this executable.
fn self_exe() -> PathBuf {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("gamelo"))
}

/// Start `gamelo --run <id>` detached, so the game (and its playtime
/// tracking) outlives the window.
fn spawn_runner(id: &str) -> Result<Child, String> {
    let mut cmd = Command::new(self_exe());
    cmd.arg("--run")
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| format!("could not start the game runner: {e}"))
}

/// Headless launch used by the window and by app-menu shortcuts: start the
/// game, wait for it, record the session. Returns the process exit code.
fn run_headless(id: &str) -> i32 {
    let cfg = load_config();
    // Messages (notifications, backup labels) in the chosen language.
    i18n::set_lang(i18n::Lang::from_key(&cfg.settings.language));
    let Some(g) = cfg
        .custom_games
        .iter()
        .find(|g| g.id == id || slugify(&g.name) == id)
    else {
        eprintln!("gamelo: no game with id {id}");
        return 2;
    };
    if scan_running().contains_key(&g.id) {
        eprintln!("gamelo: {} is already running", g.name);
        return 0;
    }
    // Pull newer saves from the sync folder and note the save folders' state.
    let pre = saves::before_launch(g, &cfg.settings);
    // Store games: cloud saves down first, and Epic's fresh online parameters.
    let mut g = g.clone();
    if let Some(link) = g.store.clone() {
        let prefix = game_prefix(&g, &cfg.settings);
        if let Err(e) = stores::sync_saves(&link, &prefix, Path::new(&link.path), false) {
            eprintln!("gamelo: cloud saves: {e}");
        }
        if link.store == stores::Store::Epic {
            let online = stores::epic_online_args(&link.app);
            if !online.is_empty() {
                g.args = format!("{} {}", g.args, online.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")).trim().to_string();
            }
        }
    }
    let g = &g;
    let (mut child, log) = match launch_game(g, &cfg.settings) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("gamelo: {e}");
            return 1;
        }
    };
    let started = unix_now();
    let t0 = Instant::now();
    let marker = RunMarker {
        id: g.id.clone(),
        name: g.name.clone(),
        runner_pid: std::process::id(),
        game_pid: child.id(),
        started,
        log: log.to_string_lossy().into(),
    };
    let dir = running_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(format!("{}.json", g.id));
    let tmp = dir.join(format!(".{}.tmp", g.id));
    if fs::write(&tmp, serde_json::to_string(&marker).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
    let code = child.wait().ok().and_then(|s| s.code());
    let secs = t0.elapsed().as_secs();
    record_session_in(&data_dir(), &g.id, started, secs, code);
    // A game that dies right away from an app-menu launch would otherwise
    // fail silently.
    if code.is_some_and(|c| c != 0) && secs < 30 {
        notify(
            &lfmt("{} closed after {}s", &[&g.name, &secs]),
            lg("Open Gamelo › the game › Last session › View log to see why."),
        );
    }
    // Back up and sync before the marker goes, so the window shows the game
    // as running until its saves are safe.
    saves::after_session(g, &cfg.settings, pre);
    if let Some(link) = &g.store
        && let Err(e) = stores::sync_saves(link, &game_prefix(g, &cfg.settings), Path::new(&link.path), true)
    {
        eprintln!("gamelo: cloud saves: {e}");
    }
    let _ = fs::remove_file(&path);
    code.unwrap_or(1)
}

/// A desktop notification, if notify-send is available.
fn notify(title: &str, body: &str) {
    if find_in_path("notify-send").is_some() {
        let _ = Command::new("notify-send")
            .args(["-a", "Gamelo", "-i", "dialog-warning", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn save_config(cfg: &AppConfig) {
    let p = config_path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(e) = fs::write(&p, serde_json::to_string_pretty(cfg).unwrap_or_default()) {
        eprintln!("gamelo: config save failed: {e}");
    }
    // The config may hold the SteamGridDB API key.
    let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
}

fn slugify(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('_').to_string();
    if s.is_empty() { "game".into() } else { s }
}

fn data_dir() -> PathBuf {
    home().join(".local/share/gamelo")
}

// ---------------------------------------------------------------------------
// Game icons
// ---------------------------------------------------------------------------

/// Where normalized (already square, 64×64) icon PNGs are cached.
fn icons_dir() -> PathBuf {
    data_dir().join("icons")
}

/// Where artwork downloaded from SteamGridDB is stored.
fn art_dir() -> PathBuf {
    data_dir().join("art")
}

/// Where an icon should come from.
enum IconSource {
    /// An image file (custom or downloaded artwork) to crop/resize.
    Image(PathBuf),
    /// A Windows PE executable to extract the embedded icon from.
    Exe(PathBuf),
    /// Nothing available; the UI draws a lettered placeholder.
    None,
}

/// Decode any supported image file into an RGBA buffer (format sniffed from content).
fn decode_image(path: &Path) -> Option<image::RgbaImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
        .map(|i| i.to_rgba8())
}

/// Center-crop to a square and scale to `size`×`size`.
fn square_icon(img: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h).max(1);
    let cropped = image::imageops::crop_imm(img, (w - side) / 2, (h - side) / 2, side, side)
        .to_image();
    if side == size {
        cropped
    } else {
        image::imageops::resize(&cropped, size, size, image::imageops::FilterType::Triangle)
    }
}

/// Scale down (never up) to fit inside `max_w`×`max_h`, keeping the aspect ratio.
fn fit_within(img: &image::RgbaImage, max_w: u32, max_h: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max_w && h <= max_h {
        return img.clone();
    }
    let s = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let nw = ((w as f32 * s).round() as u32).max(1);
    let nh = ((h as f32 * s).round() as u32).max(1);
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

/// Center-crop to the `w`:`h` aspect ratio and scale to exactly `w`×`h`.
fn cover_fit(img: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let (iw, ih) = img.dimensions();
    let (cw, ch) = if iw as u64 * h as u64 > ih as u64 * w as u64 {
        ((((ih as u64) * w as u64) / h as u64).max(1) as u32, ih)
    } else {
        (iw, (((iw as u64) * h as u64) / w as u64).max(1) as u32)
    };
    let cropped = image::imageops::crop_imm(img, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
    image::imageops::resize(&cropped, w, h, image::imageops::FilterType::Triangle)
}

/// Darken an image for use behind text: brightest top-right, darkest bottom-left
/// (where the game page puts its title and buttons).
fn darken_for_text(img: &mut image::RgbaImage) {
    let (w, h) = img.dimensions();
    let (fw, fh) = ((w.max(2) - 1) as f32, (h.max(2) - 1) as f32);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let k = (0.22 + 0.45 * (x as f32 / fw) + 0.33 * (1.0 - y as f32 / fh)).min(1.0);
        for c in &mut px.0[..3] {
            *c = (*c as f32 * k) as u8;
        }
        px.0[3] = 255;
    }
}

/// A wide game-page banner from a hero image (sharp) or a cover (blurred).
fn hero_from(img: &image::RgbaImage, blurred: bool) -> image::RgbaImage {
    let mut out = if blurred {
        image::imageops::blur(&cover_fit(img, 640, 210), 5.0)
    } else {
        cover_fit(img, 1280, 420)
    };
    darken_for_text(&mut out);
    out
}

/// Two pleasant dark colours derived from a string, for icon-less placeholders.
fn seed_colors(seed: &str) -> ([u8; 3], [u8; 3]) {
    let h = seed
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    let hue = (h % 360) as f32;
    let rgb = |hue: f32, l: f32| {
        // HSL with fixed saturation, enough for muted backgrounds.
        let c = (1.0 - (2.0 * l - 1.0).abs()) * 0.45;
        let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let (r, g, b) = match (hue / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
    };
    (rgb(hue, 0.26), rgb((hue + 40.0) % 360.0, 0.12))
}

/// A stand-in 300×450 poster for games without a cover: the icon over a
/// blurred wash of its own colours (or a colour gradient if there's no icon).
fn generated_poster(icon: Option<&image::RgbaImage>, seed: &str) -> image::RgbaImage {
    let (w, h) = (300u32, 450u32);
    let (top, bottom) = seed_colors(seed);
    let mut img = image::RgbaImage::from_fn(w, h, |_, y| {
        let t = y as f32 / (h - 1) as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        image::Rgba([mix(top[0], bottom[0]), mix(top[1], bottom[1]), mix(top[2], bottom[2]), 255])
    });
    if let Some(ic) = icon {
        let wash = image::imageops::blur(
            &image::imageops::resize(ic, w, h, image::imageops::FilterType::Triangle),
            22.0,
        );
        image::imageops::overlay(&mut img, &wash, 0, 0);
        for px in img.pixels_mut() {
            for c in &mut px.0[..3] {
                *c = (*c as f32 * 0.5) as u8;
            }
            px.0[3] = 255;
        }
        let big = square_icon(ic, 128);
        image::imageops::overlay(&mut img, &big, i64::from((w - 128) / 2), 110);
    }
    img
}

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read an IMAGE_RESOURCE_DATA_ENTRY and return the bytes it points at.
fn res_data<'a>(data: &'a [u8], entry: usize, foff: &dyn Fn(usize) -> Option<usize>) -> Option<&'a [u8]> {
    if entry + 8 > data.len() {
        return None;
    }
    let rva = le32(data, entry) as usize;
    let size = le32(data, entry + 4) as usize;
    let fo = foff(rva)?;
    if fo + size <= data.len() {
        Some(&data[fo..fo + size])
    } else {
        None
    }
}

/// Walk one resource-type directory down to its leaf data (id → bytes).
fn res_leaves<'a>(
    data: &'a [u8],
    res_off: usize,
    dir_off: usize,
    foff: &dyn Fn(usize) -> Option<usize>,
) -> Vec<(u16, &'a [u8])> {
    let mut out = Vec::new();
    const MASK: u32 = 0x7fff_ffff;
    if dir_off + 16 > data.len() {
        return out;
    }
    let n = le16(data, dir_off + 12) as usize + le16(data, dir_off + 14) as usize;
    for i in 0..n {
        let e = dir_off + 16 + i * 8;
        if e + 8 > data.len() {
            break;
        }
        let name = le32(data, e);
        let off = le32(data, e + 4);
        if name & 0x8000_0000 != 0 || off & 0x8000_0000 == 0 {
            continue;
        }
        let id = name as u16;
        let level2 = res_off + (off & MASK) as usize;
        if level2 + 16 > data.len() {
            continue;
        }
        let n2 = le16(data, level2 + 12) as usize + le16(data, level2 + 14) as usize;
        for j in 0..n2 {
            let e2 = level2 + 16 + j * 8;
            if e2 + 8 > data.len() {
                break;
            }
            let off2 = le32(data, e2 + 4);
            if off2 & 0x8000_0000 == 0 {
                if let Some(b) = res_data(data, res_off + off2 as usize, foff) {
                    out.push((id, b));
                }
            } else {
                let lang = res_off + (off2 & MASK) as usize;
                if lang + 16 > data.len() {
                    continue;
                }
                let n3 = le16(data, lang + 12) as usize + le16(data, lang + 14) as usize;
                for k in 0..n3 {
                    let e3 = lang + 16 + k * 8;
                    if e3 + 8 > data.len() {
                        break;
                    }
                    let off3 = le32(data, e3 + 4);
                    if off3 & 0x8000_0000 == 0
                        && let Some(b) = res_data(data, res_off + off3 as usize, foff)
                    {
                        out.push((id, b));
                    }
                }
            }
        }
    }
    out
}

/// Rebuild a valid `.ico` file from a Windows PE's icon resources.
///
/// Reads RT_GROUP_ICON (the directory) and RT_ICON (the raw images) from the
/// resource section. The result can be decoded by the `image` crate.
fn pe_extract_ico(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 0x40 || &data[0..2] != b"MZ" {
        return None;
    }
    let pe = le32(data, 0x3C) as usize;
    if pe + 24 > data.len() || &data[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let num_sections = le16(data, coff + 2) as usize;
    let size_opt = le16(data, coff + 16) as usize;
    let opt = coff + 20;
    if opt + 2 > data.len() {
        return None;
    }
    let magic = le16(data, opt);
    let (dd_off, num_rva_off) = match magic {
        0x10b => (opt + 96, opt + 92),  // PE32
        0x20b => (opt + 112, opt + 108), // PE32+
        _ => return None,
    };
    if le32(data, num_rva_off) < 3 || dd_off + 3 * 8 > data.len() {
        return None;
    }
    let res_rva = le32(data, dd_off + 2 * 8) as usize;
    if res_rva == 0 {
        return None;
    }
    let sec = opt + size_opt;
    let foff = |rva: usize| -> Option<usize> {
        for i in 0..num_sections {
            let s = sec + i * 40;
            if s + 40 > data.len() {
                return None;
            }
            let vsize = le32(data, s + 8) as usize;
            let va = le32(data, s + 12) as usize;
            let raw_size = le32(data, s + 16) as usize;
            let raw = le32(data, s + 20) as usize;
            if rva >= va && rva < va + vsize.max(raw_size) {
                return Some(rva - va + raw);
            }
        }
        None
    };
    let res_off = foff(res_rva)?;

    // Root directory: find RT_ICON (3) and RT_GROUP_ICON (14).
    if res_off + 16 > data.len() {
        return None;
    }
    let n = le16(data, res_off + 12) as usize + le16(data, res_off + 14) as usize;
    let (mut rt_icon, mut rt_group) = (None, None);
    for i in 0..n {
        let e = res_off + 16 + i * 8;
        if e + 8 > data.len() {
            break;
        }
        let name = le32(data, e);
        let off = le32(data, e + 4);
        if name & 0x8000_0000 != 0 || off & 0x8000_0000 == 0 {
            continue;
        }
        let sub = res_off + (off & 0x7fff_ffff) as usize;
        match name {
            3 => rt_icon = Some(sub),
            14
                if rt_group.is_none() => {
                    rt_group = Some(sub);
                }
            _ => {}
        }
        if rt_group.is_some() && rt_icon.is_some() { break; }
    }
    let icons = res_leaves(data, res_off, rt_icon?, &foff);
    let groups = res_leaves(data, res_off, rt_group?, &foff);
    let grp = groups.into_iter().next()?.1;
    if grp.len() < 6 {
        return None;
    }
    let count = le16(grp, 4) as usize;

    let mut entries = Vec::new();
    let mut image_data = Vec::new();
    let mut offset = 6 + count * 16;
    for i in 0..count {
        let ge = 6 + i * 14;
        if ge + 14 > grp.len() {
            break;
        }
        let (w, h, color) = (grp[ge], grp[ge + 1], grp[ge + 2]);
        let planes = le16(grp, ge + 4);
        let bitcount = le16(grp, ge + 6);
        let id = le16(grp, ge + 12);
        let Some((_, blob)) = icons.iter().find(|(bid, _)| *bid == id) else {
            continue;
        };
        entries.push((
            w,
            h,
            color,
            planes,
            bitcount,
            blob.len() as u32,
            offset as u32,
        ));
        image_data.extend_from_slice(blob);
        offset += blob.len();
    }
    if entries.is_empty() {
        return None;
    }

    let mut ico = Vec::with_capacity(offset);
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (w, h, color, planes, bitcount, size, off) in &entries {
        ico.push(*w);
        ico.push(*h);
        ico.push(*color);
        ico.push(0);
        ico.extend_from_slice(&planes.to_le_bytes());
        ico.extend_from_slice(&bitcount.to_le_bytes());
        ico.extend_from_slice(&size.to_le_bytes());
        ico.extend_from_slice(&off.to_le_bytes());
    }
    ico.extend_from_slice(&image_data);
    Some(ico)
}

/// Extract the icon from a Windows executable, decoded to RGBA.
fn extract_exe_icon(exe: &Path) -> Option<image::RgbaImage> {
    let data = fs::read(exe).ok()?;
    let ico = pe_extract_ico(&data)?;
    image::load_from_memory_with_format(&ico, image::ImageFormat::Ico)
        .ok()
        .map(|i| i.to_rgba8())
}

/// Locate an executable by name in PATH (plus ~/.local/bin and /usr/local/bin).
/// If `name` contains a slash it's treated as a path.
fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let c = d.join(name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    for d in [home().join(".local/bin"), PathBuf::from("/usr/local/bin")] {
        let c = d.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn find_umu() -> Option<PathBuf> {
    find_in_path("umu-run")
}

// ---------------------------------------------------------------------------
// Distribution detection + package-manager install hints
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum PkgMgr {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Unknown,
}

fn os_release_field(key: &str) -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    let needle = format!("{key}=");
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(&needle) {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn distro_name() -> String {
    os_release_field("PRETTY_NAME").unwrap_or_else(|| "your distribution".into())
}

fn detect_pkgmgr() -> PkgMgr {
    let id = os_release_field("ID").unwrap_or_default().to_lowercase();
    let like = os_release_field("ID_LIKE")
        .unwrap_or_default()
        .to_lowercase();
    let hay = format!("{id} {like}");
    let has = |s: &str| hay.split_whitespace().any(|w| w == s);
    if has("arch") || has("cachyos") || has("endeavouros") || has("manjaro") || has("garuda") {
        PkgMgr::Pacman
    } else if has("debian") || has("ubuntu") || has("linuxmint") || has("pop") || has("kali") {
        PkgMgr::Apt
    } else if has("fedora") || has("rhel") || has("centos") || has("nobara") {
        PkgMgr::Dnf
    } else if has("opensuse") || has("suse") || has("sles") {
        PkgMgr::Zypper
    } else {
        PkgMgr::Unknown
    }
}

fn install_command(pkgs: &[&str]) -> Option<String> {
    let list = pkgs.join(" ");
    Some(match detect_pkgmgr() {
        PkgMgr::Pacman => format!("sudo pacman -S --needed {list}"),
        PkgMgr::Apt => format!("sudo apt install {list}"),
        PkgMgr::Dnf => format!("sudo dnf install {list}"),
        PkgMgr::Zypper => format!("sudo zypper install {list}"),
        PkgMgr::Unknown => return None,
    })
}

// ---------------------------------------------------------------------------
// Downloads (curl/wget). Everything no-root installs into the user directory.
// ---------------------------------------------------------------------------

/// Fetch a URL. With `dest` set, writes the body there; otherwise returns stdout.
fn http_fetch(url: &str, dest: Option<&Path>) -> Result<String, String> {
    http_fetch_with(url, dest, &[])
}

/// `http_fetch` with extra request headers. curl reads them from stdin, so
/// secrets such as API keys never show up in the process list.
fn http_fetch_with(
    url: &str,
    dest: Option<&Path>,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    if find_in_path("curl").is_some() {
        let mut c = Command::new("curl");
        c.arg("-fSL")
            .arg("--retry")
            .arg("3")
            .arg("-A")
            .arg("gamelo");
        match dest {
            Some(d) => {
                c.arg("-o").arg(d);
            }
            None => {
                c.arg("-s");
            }
        }
        if !headers.is_empty() {
            c.arg("-K").arg("-");
        }
        c.arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| format!("curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let mut cfg = String::new();
            for (k, v) in headers {
                let line = format!("{k}: {v}").replace(['\r', '\n'], "");
                cfg.push_str(&format!(
                    "header = \"{}\"\n",
                    line.replace('\\', "\\\\").replace('"', "\\\"")
                ));
            }
            let _ = stdin.write_all(cfg.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("download failed: {}", err.trim()));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    if find_in_path("wget").is_some() {
        let mut c = Command::new("wget");
        c.arg("-q");
        for (k, v) in headers {
            c.arg(format!("--header={k}: {v}"));
        }
        match dest {
            Some(d) => {
                c.arg("-O").arg(d);
            }
            None => {
                c.arg("-O").arg("-");
            }
        }
        c.arg(url);
        let out = c.output().map_err(|e| format!("wget: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("download failed: {}", err.trim()));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    Err("neither curl nor wget is installed".into())
}

fn make_executable(p: &Path) -> std::io::Result<()> {
    let mut perms = fs::metadata(p)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(p, perms)
}

/// Breadth-first search for a file named `name` under `root` (shallowest wins).
fn find_file_recursive(root: &Path, name: &str) -> Option<PathBuf> {
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                queue.push_back(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(p);
            }
        }
    }
    None
}

fn github_latest_asset(
    repo: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<(String, String), String> {
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = http_fetch(&api, None)?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("bad release JSON: {e}"))?;
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or("latest release has no assets")?;
    assets
        .iter()
        .find_map(|a| {
            let name = a.get("name")?.as_str()?;
            if matches(name) {
                let url = a.get("browser_download_url")?.as_str()?.to_string();
                Some((name.to_string(), url))
            } else {
                None
            }
        })
        .ok_or_else(|| "no matching download found in the latest release".to_string())
}

/// Download and install the latest GE-Proton build into the user's copy of Steam
/// (when present) or into ~/.local/share/gamelo/proton.
fn install_ge_proton(steam_root: Option<PathBuf>) -> Result<String, String> {
    let (name, url) = github_latest_asset("GloriousEggroll/proton-ge-custom", |n| {
        n.ends_with("-x86_64.tar.gz")
    })
    .or_else(|_| {
        github_latest_asset("GloriousEggroll/proton-ge-custom", |n| {
            n.ends_with(".tar.gz")
        })
    })?;
    let base = name
        .trim_end_matches(".tar.gz")
        .trim_end_matches("-x86_64")
        .to_string();

    let dest_root = match steam_root {
        Some(sr) => sr.join("compatibilitytools.d"),
        None => data_dir().join("proton"),
    };
    fs::create_dir_all(&dest_root).map_err(|e| e.to_string())?;
    let final_dir = dest_root.join(&base);
    if final_dir.join("proton").is_file() {
        return Ok(format!("{base} is already installed"));
    }

    let dl = data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let tar_path = dl.join(&name);
    http_fetch(&url, Some(&tar_path))?;

    // Extract into a staging dir on the same filesystem, then move the tool dir.
    let staging = dest_root.join(format!(".ptmp-{base}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = fs::remove_file(&tar_path);
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err("extracting the Proton archive failed".into());
    }
    let proton = find_file_recursive(&staging, "proton")
        .ok_or("the archive did not contain a Proton build")?;
    let tool_dir = proton
        .parent()
        .ok_or("unexpected archive layout")?
        .to_path_buf();
    if final_dir.exists() {
        let _ = fs::remove_dir_all(&final_dir);
    }
    fs::rename(&tool_dir, &final_dir).map_err(|e| format!("moving Proton build: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    let _ = make_executable(&final_dir.join("proton"));
    Ok(format!("Installed {base} to {}", final_dir.display()))
}

/// Download the latest umu-launcher zipapp and install `umu-run` into ~/.local/bin.
fn install_umu() -> Result<String, String> {
    let (name, url) = github_latest_asset("Open-Wine-Components/umu-launcher", |n| {
        n.ends_with("-zipapp.tar")
    })?;

    let dl = data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let tar_path = dl.join(&name);
    http_fetch(&url, Some(&tar_path))?;

    let staging = dl.join("umu-staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = fs::remove_file(&tar_path);
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err("extracting the umu archive failed".into());
    }
    let src = find_file_recursive(&staging, "umu-run").ok_or("umu-run not found in archive")?;
    let bin_dir = home().join(".local/bin");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let dest = bin_dir.join("umu-run");
    fs::copy(&src, &dest).map_err(|e| format!("installing umu-run: {e}"))?;
    let _ = make_executable(&dest);
    let _ = fs::remove_dir_all(&staging);
    Ok(format!("Installed umu-run to {}", dest.display()))
}

// ---------------------------------------------------------------------------
// SteamGridDB artwork (optional — needs a free API key)
// ---------------------------------------------------------------------------

const SGDB_API: &str = "https://www.steamgriddb.com/api/v2";

/// Percent-encode a string for use as a single URL path segment.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn sgdb_get(path: &str, key: &str) -> Result<serde_json::Value, String> {
    let auth = format!("Bearer {key}");
    let body = http_fetch_with(
        &format!("{SGDB_API}{path}"),
        None,
        &[("Authorization", &auth)],
    )
    .map_err(|e| {
        if e.contains("401") {
            "SteamGridDB rejected the API key".to_string()
        } else {
            e
        }
    })?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("bad SteamGridDB response: {e}"))?;
    if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return Err("SteamGridDB request failed".into());
    }
    Ok(v)
}

/// A game found by a SteamGridDB search.
struct SgdbGame {
    id: u64,
    name: String,
    year: Option<i64>,
    verified: bool,
}

/// One downloadable image in a SteamGridDB listing.
struct SgdbAsset {
    url: String,
    thumb: String,
    width: u64,
    height: u64,
    style: String,
    author: String,
}

/// Which kind of artwork the browser shows.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ArtKind {
    #[default]
    Icon,
    Cover,
    Hero,
}

impl ArtKind {
    fn label(self) -> &'static str {
        match self {
            ArtKind::Icon => "Icons",
            ArtKind::Cover => "Covers",
            ArtKind::Hero => "Heroes",
        }
    }

    fn noun_plural(self) -> &'static str {
        match self {
            ArtKind::Icon => "icons",
            ArtKind::Cover => "covers",
            ArtKind::Hero => "heroes",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            ArtKind::Icon => "icon",
            ArtKind::Cover => "cover",
            ArtKind::Hero => "hero",
        }
    }

    /// Listing endpoint for a game: square icons, or static 600×900 covers.
    fn list_path(self, id: u64, page: u32) -> String {
        match self {
            ArtKind::Icon => format!(
                "/icons/game/{id}?mimes=image/png,image/vnd.microsoft.icon&page={page}"
            ),
            ArtKind::Cover => format!(
                "/grids/game/{id}?dimensions=600x900&mimes=image/png,image/jpeg&types=static&page={page}"
            ),
            ArtKind::Hero => format!(
                "/heroes/game/{id}?mimes=image/png,image/jpeg&types=static&page={page}"
            ),
        }
    }
}

/// Calendar year (UTC) of a Unix timestamp — Howard Hinnant's days-to-civil.
fn year_of(ts: i64) -> i64 {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

fn sgdb_search(query: &str, key: &str) -> Result<Vec<SgdbGame>, String> {
    let v = sgdb_get(
        &format!("/search/autocomplete/{}", url_encode(query.trim())),
        key,
    )?;
    let Some(list) = v.get("data").and_then(|d| d.as_array()) else {
        return Ok(Vec::new());
    };
    Ok(list
        .iter()
        .filter_map(|g| {
            Some(SgdbGame {
                id: g.get("id")?.as_u64()?,
                name: g.get("name")?.as_str()?.to_string(),
                year: g.get("release_date").and_then(|d| d.as_i64()).map(year_of),
                verified: g.get("verified").and_then(|b| b.as_bool()).unwrap_or(false),
            })
        })
        .collect())
}

/// One page of artwork for a SteamGridDB game, plus whether more pages exist.
fn sgdb_assets(
    kind: ArtKind,
    id: u64,
    page: u32,
    key: &str,
) -> Result<(Vec<SgdbAsset>, bool), String> {
    let v = sgdb_get(&kind.list_path(id, page), key)?;
    let limit = v.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize;
    let Some(list) = v.get("data").and_then(|d| d.as_array()) else {
        return Ok((Vec::new(), false));
    };
    let assets = list
        .iter()
        .filter_map(|a| {
            let url = a.get("url")?.as_str()?.to_string();
            let thumb = a.get("thumb").and_then(|t| t.as_str()).unwrap_or(&url).to_string();
            Some(SgdbAsset {
                thumb,
                width: a.get("width").and_then(|w| w.as_u64()).unwrap_or(0),
                height: a.get("height").and_then(|h| h.as_u64()).unwrap_or(0),
                style: a.get("style").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                author: a
                    .get("author")
                    .and_then(|au| au.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                url,
            })
        })
        .collect();
    Ok((assets, limit > 0 && list.len() >= limit))
}

/// Where SteamGridDB thumbnails are cached.
fn thumb_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".cache"))
        .join("gamelo/sgdb")
}

/// Download (or reuse a cached copy of) a thumbnail, scaled for the browser grid.
fn load_thumb(url: &str) -> Option<image::RgbaImage> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let dir = thumb_cache_dir();
    let path = dir.join(format!("{:016x}", h.finish()));
    if !path.is_file() {
        fs::create_dir_all(&dir).ok()?;
        let part = path.with_extension("part");
        if http_fetch(url, Some(&part)).is_err() {
            let _ = fs::remove_file(&part);
            return None;
        }
        fs::rename(&part, &path).ok()?;
    }
    Some(fit_within(&decode_image(&path)?, 240, 360))
}

/// Download a chosen image and store it as a PNG icon (256²), cover (600×900)
/// or hero (1920×620), named after the game's id.
fn download_art(url: &str, game_id: &str, kind: ArtKind) -> Result<PathBuf, String> {
    let dir = art_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let base = format!("{game_id}-{}", kind.noun());
    let tmp = dir.join(format!(".{base}.download"));
    // Asset URLs point at the public CDN, so no auth header here.
    if let Err(e) = http_fetch(url, Some(&tmp)) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let img = decode_image(&tmp);
    let _ = fs::remove_file(&tmp);
    let img = img.ok_or("the downloaded image is not in a supported format")?;
    let img = match kind {
        ArtKind::Icon => square_icon(&img, 256),
        ArtKind::Cover => cover_fit(&img, 600, 900),
        ArtKind::Hero => cover_fit(&img, 1920, 620),
    };
    let out = dir.join(format!("{base}.png"));
    img.save(&out).map_err(|e| format!("saving artwork: {e}"))?;
    Ok(out)
}

/// The search result that best fits `name`: an exact (case-insensitive) title,
/// else the first verified entry, else the first result.
fn best_match<'a>(name: &str, games: &'a [SgdbGame]) -> Option<&'a SgdbGame> {
    let n = name.trim().to_lowercase();
    games
        .iter()
        .find(|g| g.name.to_lowercase() == n)
        .or_else(|| games.iter().find(|g| g.verified))
        .or_else(|| games.first())
}

/// Artwork found by auto-fetch (None = SteamGridDB had none of that kind).
struct FetchedArt {
    cover: Option<PathBuf>,
    hero: Option<PathBuf>,
}

/// Look `name` up and download the top cover and/or hero for it.
fn auto_fetch_art(
    name: &str,
    game_id: &str,
    key: &str,
    want_cover: bool,
    want_hero: bool,
) -> Result<FetchedArt, String> {
    let games = sgdb_search(name, key)?;
    let Some(m) = best_match(name, &games) else {
        return Ok(FetchedArt { cover: None, hero: None });
    };
    let fetch = |kind: ArtKind| -> Option<PathBuf> {
        let (assets, _) = sgdb_assets(kind, m.id, 0, key).ok()?;
        download_art(&assets.first()?.url, game_id, kind).ok()
    };
    Ok(FetchedArt {
        cover: if want_cover { fetch(ArtKind::Cover) } else { None },
        hero: if want_hero { fetch(ArtKind::Hero) } else { None },
    })
}

/// Fetch thumbnails on a few worker threads, reporting each one as it lands.
fn spawn_thumb_workers(
    urls: Vec<String>,
    tx: std::sync::mpsc::Sender<ArtMsg>,
    ctx: egui::Context,
) {
    if urls.is_empty() {
        return;
    }
    let queue = Arc::new(Mutex::new(VecDeque::from(urls)));
    for _ in 0..4 {
        let (queue, tx, ctx) = (queue.clone(), tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            // The guard is consumed by `and_then`, so the lock isn't held while downloading.
            while let Some(url) = queue.lock().ok().and_then(|mut q| q.pop_front()) {
                let img = load_thumb(&url);
                if tx.send(ArtMsg::Thumb { url, img }).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    }
}

impl Settings {
    fn prefix_base(&self) -> PathBuf {
        if !self.prefix_base.is_empty() {
            PathBuf::from(&self.prefix_base)
        } else {
            data_dir().join("prefixes")
        }
    }

    fn mangohud_bin(&self) -> &str {
        if self.mangohud_bin.is_empty() {
            "mangohud"
        } else {
            &self.mangohud_bin
        }
    }

    fn gamemoderun_bin(&self) -> &str {
        if self.gamemoderun_bin.is_empty() {
            "gamemoderun"
        } else {
            &self.gamemoderun_bin
        }
    }

    fn umu_bin(&self) -> Option<PathBuf> {
        if !self.umu_bin.is_empty() {
            Some(PathBuf::from(&self.umu_bin))
        } else {
            find_umu()
        }
    }

    fn new_game_flags(&self) -> GameFlags {
        self.new_flags.unwrap_or(GameFlags {
            mangohud: true,
            fsr4: Fsr4Mode::Upgrade,
            ..Default::default()
        })
    }

    fn save_keep(&self) -> usize {
        if self.save_keep == 0 { 10 } else { self.save_keep as usize }
    }

    fn save_auto_backup(&self) -> bool {
        !self.save_no_auto_backup
    }

    /// UI zoom factor, clamped to a sane range (1.0 when unset).
    fn zoom(&self) -> f32 {
        if (0.5..=3.0).contains(&self.zoom) {
            self.zoom
        } else {
            1.0
        }
    }
}

fn game_prefix(g: &CustomGame, settings: &Settings) -> PathBuf {
    if !g.prefix.is_empty() {
        PathBuf::from(&g.prefix)
    } else {
        settings.prefix_base().join(slugify(&g.name))
    }
}

/// Auto prefixes are named after the game, so a rename would strand the old one
/// (and its saves). Pin the renamed game to it instead. Returns true if pinned.
fn keep_prefix_on_rename(old: &CustomGame, new: &mut CustomGame, settings: &Settings) -> bool {
    if !old.prefix.trim().is_empty()
        || !new.prefix.trim().is_empty()
        || slugify(&old.name) == slugify(&new.name)
    {
        return false;
    }
    let old_prefix = game_prefix(old, settings);
    if !old_prefix.is_dir() {
        return false;
    }
    new.prefix = old_prefix.to_string_lossy().to_string();
    true
}

/// umu GAMEID for a game: the user's ID, or a per-game placeholder.
fn game_umu_id(g: &CustomGame) -> String {
    let id = g.umu_id.trim();
    if id.is_empty() {
        format!("umu-{}", slugify(&g.name))
    } else {
        id.to_string()
    }
}

fn game_workdir(g: &CustomGame) -> Option<PathBuf> {
    if !g.workdir.is_empty() {
        Some(PathBuf::from(&g.workdir))
    } else {
        Path::new(&g.exe).parent().map(|p| p.to_path_buf())
    }
}

/// A fully resolved launch: program + args + environment + working dir.
struct Launch {
    program: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    cwd: Option<PathBuf>,
}

fn build_launch(g: &CustomGame, settings: &Settings) -> Result<Launch, String> {
    if !Path::new(&g.exe).is_file() {
        return Err(format!("executable not found: {}", g.exe));
    }
    let mut envs: Vec<(String, String)> = flag_envs(&g.flags)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    envs.extend(tuning_envs(&g.tuning, g.flags.mangohud));

    let mut wrappers: Vec<String> = Vec::new();
    // gamescope shows MangoHud itself (--mangoapp) when both are on.
    let mangoapp = g.flags.gamescope && g.flags.mangohud;
    let mut argv: Vec<String> = match g.kind {
        GameKind::Linux => {
            // Native binary: run directly, wrapping with the host mangohud/gamemode.
            if g.flags.mangohud && !mangoapp {
                wrappers.push(settings.mangohud_bin().to_string());
            }
            if g.flags.gamemode {
                wrappers.push(settings.gamemoderun_bin().to_string());
            }
            let mut a = vec![g.exe.clone()];
            a.extend(tokenize(&g.args));
            a
        }
        GameKind::Windows => {
            // Standalone Windows game: umu-run drives Proton + the Steam runtime
            // container, with no Steam client involved.
            let umu = settings
                .umu_bin()
                .ok_or_else(|| "umu-run not found (install umu-launcher)".to_string())?;
            umu_env(g, settings, &mut envs)?;
            // OptiScaler is loaded in place of a system DLL, which Wine must allow;
            // the OpenXR bridge stays off unless VR is allowed (see Settings).
            let overrides: Vec<String> = optiscaler::dll_override(&g.id)
                .into_iter()
                .chain((!settings.allow_openxr).then(|| "wineopenxr=d".to_string()))
                .collect();
            if !overrides.is_empty() {
                envs.push(("WINEDLLOVERRIDES".into(), overrides.join(";")));
            }
            // MangoHud ships as a Vulkan layer inside the Steam runtime container,
            // so it is enabled by env rather than the host mangohud wrapper.
            if g.flags.mangohud && !mangoapp {
                envs.push(("MANGOHUD".into(), "1".into()));
            }
            if g.flags.gamemode {
                wrappers.push(settings.gamemoderun_bin().to_string());
            }
            let mut a = vec![umu.to_string_lossy().into(), g.exe.clone()];
            a.extend(tokenize(&g.args));
            a
        }
    };

    if g.flags.gamescope {
        // gamemoderun gamescope <options> -- <game>
        let mut gs = vec!["gamescope".to_string()];
        gs.extend(g.gamescope_opts.args(mangoapp));
        gs.push("--".into());
        wrappers.extend(gs);
    }
    // wrappers go outermost: gamemoderun [gamescope … --] mangohud <program> <args...>
    for w in wrappers.into_iter().rev() {
        argv.insert(0, w);
    }
    // The user's own variables win; DLL overrides are combined instead.
    for (k, v) in parse_env(&g.env) {
        match envs.iter_mut().find(|(ek, _)| *ek == k) {
            Some(e) if k == "WINEDLLOVERRIDES" => e.1 = format!("{};{v}", e.1),
            Some(e) => e.1 = v,
            None => envs.push((k, v)),
        }
    }

    let program = argv.remove(0);
    Ok(Launch {
        program,
        args: argv,
        envs,
        cwd: game_workdir(g).filter(|p| p.is_dir()),
    })
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@,%+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Ensure the parent of a game's prefix directory exists (umu creates the prefix itself).
fn ensure_prefix(g: &CustomGame, settings: &Settings) -> Result<PathBuf, String> {
    let prefix = game_prefix(g, settings);
    if let Some(parent) = prefix.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(prefix)
}

/// Build the common umu-run environment (GAMEID/PROTONPATH/WINEPREFIX) for a Windows game.
fn umu_env(
    g: &CustomGame,
    settings: &Settings,
    envs: &mut Vec<(String, String)>,
) -> Result<PathBuf, String> {
    if g.proton_dir.is_empty() {
        return Err("no Proton layer selected for this game".into());
    }
    let prefix = game_prefix(g, settings);
    envs.push(("GAMEID".into(), game_umu_id(g)));
    envs.push(("PROTONPATH".into(), g.proton_dir.clone()));
    envs.push(("WINEPREFIX".into(), prefix.to_string_lossy().into()));
    Ok(prefix)
}

/// Spawn a local game in its own session with output captured to a per-game log.
/// Returns the child handle and the log path so the UI can report real status.
fn launch_game(
    g: &CustomGame,
    settings: &Settings,
) -> Result<(Child, PathBuf), String> {
    let launch = build_launch(g, settings)?;
    let log_dir = data_dir().join("logs");
    fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let log_path = log_dir.join(format!("{}.log", slugify(&g.name)));
    let log = fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args);
    for (k, v) in &launch.envs {
        cmd.env(k, v);
    }
    if let Some(cwd) = &launch.cwd {
        cmd.current_dir(cwd);
    }
    // Detach from the GUI: no stdin, output to the log file, own session.
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(log));
    cmd.stderr(Stdio::from(log_err));
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    Ok((child, log_path))
}

/// Create the wine prefix for a Windows game via `umu-run ""`.
fn init_prefix(g: &CustomGame, settings: &Settings) -> Result<Child, String> {
    let umu = settings
        .umu_bin()
        .ok_or_else(|| "umu-run not found (install umu-launcher)".to_string())?;
    let mut envs = Vec::new();
    umu_env(g, settings, &mut envs)?;
    ensure_prefix(g, settings)?;
    let mut cmd = Command::new(&umu);
    cmd.arg(""); // empty target => umu only sets up the prefix
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.spawn().map_err(|e| format!("spawn failed: {e}"))
}

/// Open winecfg inside the game's prefix via umu-run.
fn run_winecfg(g: &CustomGame, settings: &Settings) -> Result<Child, String> {
    let umu = settings
        .umu_bin()
        .ok_or_else(|| "umu-run not found (install umu-launcher)".to_string())?;
    let mut envs = Vec::new();
    let prefix = umu_env(g, settings, &mut envs)?;
    let winecfg = prefix.join("drive_c/windows/system32/winecfg.exe");
    if !winecfg.is_file() {
        return Err("prefix not initialized yet — run \"Init prefix\" first".into());
    }
    let mut cmd = Command::new(&umu);
    cmd.arg(&winecfg);
    for (k, v) in &envs {
        cmd.env(k, v);
    }
    cmd.spawn().map_err(|e| format!("spawn failed: {e}"))
}

/// One-line shell command equivalent of running the game.
fn launch_command_string(
    g: &CustomGame,
    settings: &Settings,
) -> Result<String, String> {
    let launch = build_launch(g, settings)?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = &launch.cwd {
        parts.push(format!("cd {} &&", shell_quote(&cwd.to_string_lossy())));
    }
    for (k, v) in &launch.envs {
        parts.push(format!("{k}={}", shell_quote(v)));
    }
    parts.push(shell_quote(&launch.program));
    for a in &launch.args {
        parts.push(shell_quote(a));
    }
    Ok(parts.join(" "))
}

/// Quote one Exec argument per the desktop-entry spec: reserved characters
/// force double quotes (escaping `"` `` ` `` `$` `\`), then the string-level
/// escaping doubles backslashes and `%` becomes `%%`.
fn desktop_exec_arg(arg: &str) -> String {
    let reserved = |c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c);
    let quoted = if arg.contains(reserved) {
        let mut q = String::from('"');
        for c in arg.chars() {
            if "\"`$\\".contains(c) {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        arg.to_string()
    };
    quoted.replace('\\', "\\\\").replace('%', "%%")
}

/// Create a .desktop entry so the game shows up in the app menu. It runs the
/// game through `gamelo --run`, so menu launches count towards playtime.
fn write_desktop_entry(g: &CustomGame, settings: &Settings) -> Result<PathBuf, String> {
    // Fail now (missing exe, no Proton…) rather than from the app menu later.
    build_launch(g, settings)?;
    let dir = home().join(".local/share/applications");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("gamelo-{}.desktop", g.id));

    let exec = format!("{} --run {}", desktop_exec_arg(&self_exe().to_string_lossy()), g.id);

    let mut s = String::new();
    s.push_str("[Desktop Entry]\n");
    s.push_str("Type=Application\n");
    s.push_str(&format!("Name={}\n", g.name.replace(['\n', '\r'], " ")));
    s.push_str("Comment=Game launched by Gamelo\n");
    s.push_str(&format!("Exec={exec}\n"));
    let cached_icon = icons_dir().join(format!("icon-{}.png", g.id));
    if !g.icon.is_empty() && Path::new(&g.icon).is_file() {
        s.push_str(&format!("Icon={}\n", g.icon));
    } else if cached_icon.is_file() {
        s.push_str(&format!("Icon={}\n", cached_icon.to_string_lossy()));
    }
    s.push_str("Terminal=false\n");
    s.push_str("Categories=Game;\n");
    fs::write(&path, s).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Guess a game's name from its executable: the nearest folder that isn't a
/// generic build/library folder (bin, Binaries, Win64, Games…), else the exe name.
fn guess_game_name(exe: &Path) -> String {
    const GENERIC: &[&str] = &[
        "bin", "bin32", "bin64", "binaries", "win32", "win64", "x64", "x86", "x86_64",
        "release", "shipping", "build", "linux", "linux64", "game", "games", "downloads",
        "desktop", "home", "windows", "program files", "program files (x86)",
    ];
    let tidy = |s: &str| {
        // Dashes separate words only in names without spaces ("Thronefall-v1-61",
        // not "Resonance - A Plague Tale").
        let seps: &[char] = if s.contains(' ') { &['_', '.', ' '] } else { &['_', '.', ' ', '-'] };
        let words: Vec<&str> = s.split(seps).filter(|w| !w.is_empty()).collect();
        // Drop a trailing version like "v2354" or "v1 61" / "v2 1 9" (dots and
        // dashes split it), but keep "Half Life 2".
        let is_v = |w: &str| w.strip_prefix(['v', 'V']).is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
        let numeric = |w: &str| w.chars().all(|c| c.is_ascii_digit());
        let end = (1..words.len())
            .find(|&i| is_v(words[i]) && words[i + 1..].iter().all(|w| numeric(w)))
            .unwrap_or(words.len());
        words[..end].join(" ")
    };
    for dir in exe.ancestors().skip(1).take(4) {
        // Never climb into a home directory's or a top-level folder's name.
        let top = dir
            .parent()
            .is_none_or(|p| p == Path::new("/") || p == Path::new("/home"));
        if top || dir == home() {
            break;
        }
        let Some(n) = dir.file_name().and_then(|n| n.to_str()) else {
            break;
        };
        if !GENERIC.contains(&n.to_lowercase().as_str()) {
            let t = tidy(n);
            if !t.is_empty() {
                return t;
            }
        }
    }
    exe.file_stem()
        .map(|s| tidy(&s.to_string_lossy()))
        .unwrap_or_else(|| "New game".into())
}

fn pick_executable() -> Option<PathBuf> {
    pick_file(lg("Select game executable"), lg("Executables"), &["*.exe", "*.EXE"])
}

fn pick_image() -> Option<PathBuf> {
    pick_file(
        "Select artwork image",
        "Images",
        &["*.png", "*.jpg", "*.jpeg", "*.ico", "*.PNG", "*.JPG", "*.JPEG", "*.ICO"],
    )
}

fn pick_any_file(title: &str) -> Option<PathBuf> {
    pick_file(title, lg("All files"), &["*"])
}

/// Open a native folder chooser and return the selected folder.
fn pick_folder(title: &str) -> Option<PathBuf> {
    run_picker(vec![
        (
            "zenity".into(),
            vec!["--file-selection".into(), "--directory".into(), format!("--title={title}")],
            true,
        ),
        (
            "kdialog".into(),
            vec![
                "--getexistingdirectory".into(),
                home().to_string_lossy().into(),
                "--title".into(),
                title.into(),
            ],
            false,
        ),
        (
            "yad".into(),
            vec!["--file-selection".into(), "--directory".into(), format!("--title={title}")],
            true,
        ),
    ])
}

/// Run the first available chooser of (program, args, force GTK's built-in dialog).
fn run_picker(candidates: Vec<(String, Vec<String>, bool)>) -> Option<PathBuf> {
    for (prog, args, no_portal) in candidates {
        let mut cmd = Command::new(&prog);
        cmd.args(&args).stderr(Stdio::null());
        if no_portal {
            // Avoid the xdg-desktop-portal FileChooser backend, which is absent on
            // some minimal window managers (e.g. mangowc).
            cmd.env("GTK_USE_PORTAL", "0");
        }
        match cmd.output() {
            Ok(out) => {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        return Some(PathBuf::from(s));
                    }
                }
                // The tool ran but the user cancelled: stop trying others.
                return None;
            }
            // Tool not installed: try the next one.
            Err(_) => continue,
        }
    }
    None
}

/// Open a native file chooser (zenity/kdialog/yad) and return the selected path.
fn pick_file(title: &str, filter_name: &str, patterns: &[&str]) -> Option<PathBuf> {
    let home_s = home().to_string_lossy().to_string();
    let pats = patterns.join(" ");
    // (program, args, force GTK built-in chooser)
    let candidates: Vec<(String, Vec<String>, bool)> = vec![
        (
            "zenity".into(),
            vec![
                "--file-selection".into(),
                format!("--title={title}"),
                format!("--file-filter={filter_name}|{pats}"),
                format!("--file-filter={}|*", lg("All files")),
            ],
            true,
        ),
        (
            "kdialog".into(),
            vec![
                "--getopenfilename".into(),
                home_s,
                format!("{pats}|{filter_name}"),
                "--title".into(),
                title.into(),
            ],
            false,
        ),
        (
            "yad".into(),
            vec![
                "--file-selection".into(),
                format!("--title={title}"),
                format!("--file-filter={filter_name} | {pats}"),
            ],
            true,
        ),
    ];
    run_picker(candidates)
}

/// Open a folder, file or URL with the desktop's default handler.
fn xdg_open(target: impl AsRef<std::ffi::OsStr>) -> Option<Child> {
    Command::new("xdg-open").arg(target).spawn().ok()
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
        let cfg = load_config();
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
        }
    }

    /// Drop a cached icon so it is rebuilt from its (possibly changed) source.
    fn forget_icon(&mut self, key: &str) {
        self.icon_tex.remove(key);
        self.icon_fail.remove(key);
        let _ = fs::remove_file(icons_dir().join(format!("{key}.png")));
    }

    /// Drop cached textures for a game's icon and cover.
    fn forget_art(&mut self, id: &str) {
        for kind in ["icon", "poster", "gen", "hero"] {
            self.forget_icon(&format!("{kind}-{id}"));
        }
    }

    /// Point a game's icon, cover or hero at `path` (empty = back to the default).
    fn set_art(&mut self, id: &str, kind: ArtKind, path: String) -> bool {
        let Some(g) = self.cfg.custom_games.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        if kind == ArtKind::Cover && !path.is_empty() {
            self.motion.art_arrived.insert(id.to_string(), self.now);
        }
        match kind {
            ArtKind::Icon => g.icon = path,
            ArtKind::Cover => g.cover = path,
            ArtKind::Hero => g.hero = path,
        }
        save_config(&self.cfg);
        self.forget_art(id);
        true
    }

    fn sgdb_key(&self) -> Option<String> {
        let k = self.cfg.settings.steamgriddb_key.trim();
        (!k.is_empty()).then(|| k.to_string())
    }

    fn art_game_id(&self) -> Option<String> {
        self.art
            .game
            .and_then(|i| self.cfg.custom_games.get(i))
            .map(|g| g.id.clone())
    }

    /// Show the Artwork page for game `i`.
    fn open_artwork(&mut self, ctx: &egui::Context, i: usize) {
        self.page = Page::Artwork;
        if self.art.game != Some(i) || self.art.matches.is_empty() {
            self.art_select_game(ctx, i);
        }
    }

    /// Switch the browser to game `i` and search SteamGridDB for its name.
    fn art_select_game(&mut self, ctx: &egui::Context, i: usize) {
        let Some(name) = self.cfg.custom_games.get(i).map(|g| g.name.clone()) else {
            return;
        };
        self.art.game = Some(i);
        self.art.query = name;
        self.art.matches.clear();
        self.art.sgdb_id = None;
        self.art.assets.clear();
        self.art.picked = None;
        self.art.more = false;
        self.art.error = None;
        self.art.loading = false;
        self.art.generation += 1;
        if self.sgdb_key().is_some() {
            self.art_search(ctx);
        }
    }

    fn art_search(&mut self, ctx: &egui::Context) {
        let Some(key) = self.sgdb_key() else {
            self.art.error = Some(lg("Add your SteamGridDB API key first").into());
            return;
        };
        let query = self.art.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.art.generation += 1;
        let generation = self.art.generation;
        self.art.matches.clear();
        self.art.sgdb_id = None;
        self.art.assets.clear();
        self.art.picked = None;
        self.art.more = false;
        self.art.error = None;
        self.art.loading = true;
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = sgdb_search(&query, &key);
            let _ = tx.send(ArtMsg::Games { generation, result });
            ctx.request_repaint();
        });
    }

    /// Load page `page` of the selected match's artwork (0 = start over).
    fn art_list(&mut self, ctx: &egui::Context, page: u32) {
        let (Some(key), Some(id)) = (self.sgdb_key(), self.art.sgdb_id) else {
            return;
        };
        if page == 0 {
            self.art.generation += 1;
            self.art.assets.clear();
            self.art.picked = None;
            self.art.more = false;
        }
        let generation = self.art.generation;
        let kind = self.art.kind;
        self.art.loading = true;
        self.art.error = None;
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = sgdb_assets(kind, id, page, &key);
            let _ = tx.send(ArtMsg::Assets {
                generation,
                page,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Download the picked image and set it on the current game.
    fn art_apply(&mut self, ctx: &egui::Context) {
        let (Some(id), Some(asset)) = (
            self.art_game_id(),
            self.art.picked.and_then(|p| self.art.assets.get(p)),
        ) else {
            return;
        };
        let (url, kind) = (asset.url.clone(), self.art.kind);
        self.art.applying = true;
        self.set_status(lfmt("Downloading {}…", &[&lg(kind.noun())]), false);
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = download_art(&url, &id, kind);
            let _ = tx.send(ArtMsg::Applied { id, kind, result });
            ctx.request_repaint();
        });
    }

    /// Handle replies from SteamGridDB worker threads.
    fn art_poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.art_rx.try_recv() {
            match msg {
                ArtMsg::Games { generation, result } => {
                    if generation != self.art.generation {
                        continue;
                    }
                    self.art.loading = false;
                    match result {
                        Ok(games) => {
                            if games.is_empty() {
                                self.art.error = Some(lfmt(
                                    "No SteamGridDB match for “{}” — try another spelling",
                                    &[&self.art.query.trim()],
                                ));
                            }
                            self.art.sgdb_id = games.first().map(|g| g.id);
                            self.art.matches = games;
                            self.art_list(ctx, 0);
                        }
                        Err(e) => self.art.error = Some(e),
                    }
                }
                ArtMsg::Assets {
                    generation,
                    page,
                    result,
                } => {
                    if generation != self.art.generation {
                        continue;
                    }
                    self.art.loading = false;
                    match result {
                        Ok((assets, more)) => {
                            if assets.is_empty() && page == 0 {
                                self.art.error = Some(lfmt(
                                    "SteamGridDB has no {} for this game yet",
                                    &[&lg(self.art.kind.noun_plural())],
                                ));
                            }
                            self.art.page = page;
                            self.art.more = more;
                            let urls: Vec<String> = assets
                                .iter()
                                .map(|a| a.thumb.clone())
                                .filter(|u| {
                                    !self.art.thumbs.contains_key(u)
                                        && !self.art.thumb_failed.contains(u)
                                        && !self.art.thumb_pending.contains(u)
                                })
                                .collect();
                            self.art.thumb_pending.extend(urls.iter().cloned());
                            self.art.assets.extend(assets);
                            spawn_thumb_workers(urls, self.art_tx.clone(), ctx.clone());
                        }
                        Err(e) => self.art.error = Some(e),
                    }
                }
                ArtMsg::Thumb { url, img } => {
                    self.art.thumb_pending.remove(&url);
                    match img {
                        Some(img) => {
                            let tex = load_rgba(ctx, format!("sgdb:{url}"), &img);
                            self.motion.thumb_loaded.insert(url.clone(), self.now);
                            self.art.thumbs.insert(url, tex);
                        }
                        None => {
                            self.art.thumb_failed.insert(url);
                        }
                    }
                }
                ArtMsg::Applied { id, kind, result } => {
                    self.art.applying = false;
                    match result {
                        Ok(path) => {
                            if self.set_art(&id, kind, path.to_string_lossy().to_string()) {
                                self.set_status(lfmt("New {} applied", &[&lg(kind.noun())]), false);
                            } else {
                                self.set_status(lg("Downloaded, but that game no longer exists").into(), true);
                            }
                        }
                        Err(e) => self.set_status(lfmt("Artwork download failed: {}", &[&e]), true),
                    }
                }
                ArtMsg::KeyChecked(result) => {
                    self.key_checking = false;
                    self.key_check = Some(result);
                }
                ArtMsg::AutoFetched { id, result } => {
                    self.auto_pending.remove(&id);
                    let Some(i) = self.index_of(&id) else {
                        continue;
                    };
                    let name = self.cfg.custom_games[i].name.clone();
                    match result {
                        Ok(art) => {
                            let (got_cover, got_hero) = (art.cover.is_some(), art.hero.is_some());
                            let g = &mut self.cfg.custom_games[i];
                            g.art_checked = true;
                            if let Some(p) = art.cover {
                                g.cover = p.to_string_lossy().into();
                                self.motion.art_arrived.insert(id.clone(), self.now);
                            }
                            if let Some(p) = art.hero {
                                g.hero = p.to_string_lossy().into();
                            }
                            save_config(&self.cfg);
                            self.forget_art(&id);
                            let msg = match (got_cover, got_hero) {
                                (true, true) => lfmt("Artwork added for {}", &[&name]),
                                (true, false) => lfmt("Cover added for {}", &[&name]),
                                (false, true) => lfmt("Banner added for {}", &[&name]),
                                (false, false) => lfmt("No artwork found for {} on SteamGridDB", &[&name]),
                            };
                            self.set_status(msg, false);
                        }
                        Err(e) => self.set_status(lfmt("Artwork for {}: {}", &[&name, &e]), true),
                    }
                }
            }
        }
    }

    fn icon_finish(&mut self, ctx: &egui::Context, key: &str, img: image::RgbaImage) -> egui::TextureHandle {
        let dir = icons_dir();
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{key}.png"));
        let _ = img.save(&path);
        let tex = load_rgba(ctx, key.to_string(), &img);
        self.icon_tex.insert(key.to_string(), tex.clone());
        tex
    }

    fn get_icon(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        source: &IconSource,
    ) -> Option<egui::TextureHandle> {
        self.get_tex(ctx, key, || match source {
            IconSource::Image(p) => decode_image(p).map(|i| square_icon(&i, 64)),
            IconSource::Exe(p) => extract_exe_icon(p).map(|i| square_icon(&i, 64)),
            IconSource::None => None,
        })
    }

    /// The icon for game `i`: its custom image, else the exe's embedded icon.
    fn game_icon(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        let key = format!("icon-{}", g.id);
        let source = if !g.icon.is_empty() && Path::new(&g.icon).is_file() {
            IconSource::Image(PathBuf::from(&g.icon))
        } else if g.kind == GameKind::Windows {
            IconSource::Exe(PathBuf::from(&g.exe))
        } else {
            IconSource::None
        };
        self.get_icon(ctx, &key, &source)
    }

    /// The cover poster for game `i` (300×450), if one is set.
    fn game_poster(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        if g.cover.is_empty() || !Path::new(&g.cover).is_file() {
            return None;
        }
        let key = format!("poster-{}", g.id);
        let path = PathBuf::from(&g.cover);
        self.get_tex(ctx, &key, || decode_image(&path).map(|i| cover_fit(&i, 300, 450)))
    }

    /// The game page banner: the hero image, else a blurred cover, else the
    /// generated poster's colours.
    fn game_hero(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        let key = format!("hero-{}", g.id);
        let pick = |p: &str| (!p.is_empty() && Path::new(p).is_file()).then(|| PathBuf::from(p));
        let (hero, cover) = (pick(&g.hero), pick(&g.cover));
        let (seed, exe, windows) = (g.name.clone(), g.exe.clone(), g.kind == GameKind::Windows);
        self.get_tex(ctx, &key, || {
            if let Some(img) = hero.as_deref().and_then(decode_image) {
                return Some(hero_from(&img, false));
            }
            if let Some(img) = cover.as_deref().and_then(decode_image) {
                return Some(hero_from(&img, true));
            }
            let icon = windows.then(|| extract_exe_icon(Path::new(&exe))).flatten();
            Some(hero_from(&generated_poster(icon.as_ref(), &seed), true))
        })
    }

    /// A cached texture: from memory, else the on-disk PNG cache, else `make`.
    fn get_tex(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        make: impl FnOnce() -> Option<image::RgbaImage>,
    ) -> Option<egui::TextureHandle> {
        if let Some(t) = self.icon_tex.get(key) {
            return Some(t.clone());
        }
        if self.icon_fail.contains(key) {
            return None;
        }
        let dir = icons_dir();
        let cache = dir.join(format!("{key}.png"));
        if cache.is_file()
            && let Some(img) = decode_image(&cache)
        {
            let tex = load_rgba(ctx, key.to_string(), &img);
            self.icon_tex.insert(key.to_string(), tex.clone());
            return Some(tex);
        }
        match make() {
            Some(img) => Some(self.icon_finish(ctx, key, img)),
            None => {
                self.icon_fail.insert(key.to_string());
                None
            }
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

    /// Poll running markers (about once a second) and report games that ended.
    fn poll_running(&mut self) {
        self.children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        if self.last_scan.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_scan = Instant::now();
        let now = scan_running();
        let ended: Vec<RunMarker> = self
            .running
            .values()
            .filter(|m| !now.contains_key(&m.id))
            .cloned()
            .collect();
        let started: Vec<String> = now
            .values()
            .filter(|m| !self.running.contains_key(&m.id))
            .map(|m| m.name.clone())
            .collect();
        self.running = now;
        for name in started {
            self.set_status(lfmt("{} is running", &[&name]), false);
        }
        // Started games stop "starting"; a runner that exited (and was reaped
        // above) without ever writing its marker means the launch failed.
        let failed: Vec<String> = self
            .motion
            .launching
            .iter()
            .filter(|(id, (_, pid))| !self.running.contains_key(*id) && !pid_alive(*pid))
            .map(|(id, _)| id.clone())
            .collect();
        self.motion.launching.retain(|id, _| !self.running.contains_key(id));
        for id in failed {
            self.motion.launching.remove(&id);
            if let Some(i) = self.index_of(&id) {
                let name = self.cfg.custom_games[i].name.clone();
                self.set_status(lfmt("{} failed to start — see its log", &[&name]), true);
            }
        }
        if ended.is_empty() {
            return;
        }
        let before = std::mem::replace(&mut self.stats, load_stats());
        self.sessions = sessions::load();
        for m in ended {
            self.forget_disk_use(&m.id);
            let s = self.stats.get(&m.id).cloned().unwrap_or_default();
            let old = before.get(&m.id).map_or(0, |b| b.playtime_secs);
            if s.playtime_secs > old {
                self.motion.played.insert(m.id.clone(), (old, s.playtime_secs, None));
            }
            if self.stopping.remove(&m.id) {
                self.set_status(lfmt("{} stopped", &[&m.name]), false);
            } else if s.last_exit != Some(0) && s.last_session_secs < 30 {
                self.set_status(
                    lfmt("{} exited after {}s — see {}", &[&m.name, &s.last_session_secs, &m.log]),
                    true,
                );
            } else {
                self.set_status(
                    lfmt("{} closed · played {}", &[&m.name, &format_duration(s.last_session_secs)]),
                    false,
                );
            }
        }
    }

    fn play(&mut self, i: usize) {
        let g = &self.cfg.custom_games[i];
        if self.running.contains_key(&g.id) {
            return;
        }
        // Surface problems (missing exe, no Proton…) here; the runner has no window.
        if let Err(e) = build_launch(g, &self.cfg.settings) {
            let id = g.id.clone();
            self.set_status(e, true);
            self.motion.shake.insert(id, self.now);
            return;
        }
        let (id, name) = (g.id.clone(), g.name.clone());
        match spawn_runner(&id) {
            Ok(child) => {
                // Launching an unsorted or backlog game makes it Playing.
                if matches!(self.cfg.custom_games[i].status, GameStatus::None | GameStatus::Backlog) {
                    self.cfg.custom_games[i].status = GameStatus::Playing;
                    save_config(&self.cfg);
                }
                self.motion.launching.insert(id, (self.now, child.id()));
                self.children.push(child);
                self.set_status(lfmt("Launching {}…", &[&name]), false);
            }
            Err(e) => {
                self.set_status(e, true);
                self.motion.shake.insert(id, self.now);
            }
        }
    }

    fn stop(&mut self, id: &str) {
        let Some(m) = self.running.get(id).cloned() else {
            return;
        };
        let g = self.index_of(id).map(|i| self.cfg.custom_games[i].clone());
        self.children.extend(stop_game(&m, g.as_ref(), &self.cfg.settings));
        self.stopping.insert(id.to_string());
        self.set_status(lfmt("Stopping {}…", &[&m.name]), false);
    }

    /// Games that still lack a cover or hero and haven't been auto-fetched.
    fn needs_art(&self) -> Vec<String> {
        self.cfg
            .custom_games
            .iter()
            .filter(|g| !g.art_checked && (g.cover.is_empty() || g.hero.is_empty()))
            .map(|g| g.id.clone())
            .collect()
    }

    /// Fetch missing covers/heroes for these games in the background, one at a time.
    fn auto_fetch(&mut self, ctx: &egui::Context, ids: Vec<String>) {
        let Some(key) = self.sgdb_key() else {
            return;
        };
        let jobs: Vec<(String, String, bool, bool)> = ids
            .iter()
            .filter(|id| !self.auto_pending.contains(*id))
            .filter_map(|id| {
                let g = self.cfg.custom_games.iter().find(|g| &g.id == id)?;
                let (want_cover, want_hero) = (g.cover.is_empty(), g.hero.is_empty());
                (want_cover || want_hero)
                    .then(|| (g.id.clone(), g.name.clone(), want_cover, want_hero))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        for j in &jobs {
            self.auto_pending.insert(j.0.clone());
        }
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for (id, name, want_cover, want_hero) in jobs {
                let result = auto_fetch_art(&name, &id, &key, want_cover, want_hero);
                let _ = tx.send(ArtMsg::AutoFetched { id, result });
                ctx.request_repaint();
            }
        });
    }

    /// Load the game page's text buffers when a different game is shown.
    fn sync_edit(&mut self, i: usize) {
        let g = &self.cfg.custom_games[i];
        if self.edit.id == g.id {
            return;
        }
        self.edit = EditBuf {
            id: g.id.clone(),
            name: g.name.clone(),
            args: g.args.clone(),
            workdir: g.workdir.clone(),
            prefix: g.prefix.clone(),
            umu_id: g.umu_id.clone(),
            exe: g.exe.clone(),
            env: g.env.clone(),
        };
    }

    /// Save one edited text field of game `i`.
    fn commit_field(&mut self, i: usize, field: Field) {
        let old = self.cfg.custom_games[i].clone();
        let mut g = old.clone();
        let e = self.edit.clone();
        let (slot, value, current) = match field {
            Field::Name => (&mut g.name, e.name.trim(), &old.name),
            Field::Args => (&mut g.args, e.args.trim(), &old.args),
            Field::Workdir => (&mut g.workdir, e.workdir.trim(), &old.workdir),
            Field::Prefix => (&mut g.prefix, e.prefix.trim(), &old.prefix),
            Field::UmuId => (&mut g.umu_id, e.umu_id.trim(), &old.umu_id),
            Field::Exe => (&mut g.exe, e.exe.trim(), &old.exe),
            Field::Env => (&mut g.env, e.env.trim(), &old.env),
        };
        if value == current.as_str() || (matches!(field, Field::Name) && value.is_empty()) {
            // Unchanged (or an empty name): put the saved value back in the box.
            let saved = current.clone();
            self.edit_slot(field).clone_from(&saved);
            return;
        }
        *slot = value.to_string();
        let mut msg = lg("Saved").to_string();
        if matches!(field, Field::Name) && keep_prefix_on_rename(&old, &mut g, &self.cfg.settings) {
            msg = lfmt("Renamed — still using its prefix {}", &[&g.prefix]);
            self.edit.prefix = g.prefix.clone();
        }
        if matches!(field, Field::Exe) {
            self.forget_art(&g.id);
        }
        let saved = value.to_string();
        *self.edit_slot(field) = saved;
        self.cfg.custom_games[i] = g;
        save_config(&self.cfg);
        self.set_status(msg, false);
    }

    fn edit_slot(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.edit.name,
            Field::Args => &mut self.edit.args,
            Field::Workdir => &mut self.edit.workdir,
            Field::Prefix => &mut self.edit.prefix,
            Field::UmuId => &mut self.edit.umu_id,
            Field::Exe => &mut self.edit.exe,
            Field::Env => &mut self.edit.env,
        }
    }

    fn remove_game(&mut self, i: usize) {
        let g = self.cfg.custom_games.remove(i);
        // Leave its poster behind to fade out while the grid closes the gap.
        if let Some(rel) = self.motion.tile_rel.remove(&g.id) {
            let tex = self
                .icon_tex
                .get(&format!("poster-{}", g.id))
                .or_else(|| self.icon_tex.get(&format!("gen-{}", g.id)))
                .cloned();
            self.motion.ghosts.push(Ghost { rel, tex, since: self.now });
        }
        // Keep the artwork browser pointing at the same game.
        match self.art.game {
            Some(a) if a == i => {
                self.art.game = None;
                self.art.generation += 1;
                self.art.matches.clear();
                self.art.assets.clear();
                self.art.picked = None;
            }
            Some(a) if a > i => self.art.game = Some(a - 1),
            _ => {}
        }
        if self.selected.as_deref() == Some(g.id.as_str()) {
            self.selected = None;
            if matches!(self.page, Page::Game | Page::Artwork) {
                self.page = Page::Library;
            }
        }
        save_config(&self.cfg);
        self.set_status(lfmt("Removed {} from the library — its files are kept", &[&g.name]), false);
    }

    fn new_add_modal(&self, exe: PathBuf) -> AddModal {
        let is_exe = exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        // The Proton build chosen in Settings, else the one the last Windows game uses.
        let tool = |dir: &str| self.proton_tools.iter().position(|t| t.dir.to_string_lossy() == dir);
        let proton_idx = tool(&self.cfg.settings.default_proton)
            .or_else(|| {
                self.cfg
                    .custom_games
                    .iter()
                    .rev()
                    .find(|g| g.kind == GameKind::Windows)
                    .and_then(|g| tool(&g.proton_dir))
            })
            .unwrap_or(0);
        AddModal {
            name: guess_game_name(&exe),
            kind: if is_exe { GameKind::Windows } else { GameKind::Linux },
            exe: exe.to_string_lossy().into(),
            proton_idx,
        }
    }

    fn add_game(&mut self, ctx: &egui::Context, m: AddModal) {
        let (id, name) = self.insert_game(m);
        save_config(&self.cfg);
        self.selected = Some(id.clone());
        self.page = Page::Game;
        if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
            self.auto_fetch(ctx, vec![id]);
            self.set_status(lfmt("Added {} — fetching artwork…", &[&name]), false);
        } else {
            self.set_status(lfmt("Added {}", &[&name]), false);
        }
    }

    /// Add several games at once (from a folder scan), staying on the library.
    fn add_games(&mut self, ctx: &egui::Context, list: Vec<AddModal>) {
        let n = list.len();
        let ids: Vec<String> = list.into_iter().map(|m| self.insert_game(m).0).collect();
        save_config(&self.cfg);
        self.page = Page::Library;
        if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
            self.auto_fetch(ctx, ids);
        }
        self.set_status(lfmt("Added {} games", &[&n]), false);
    }

    /// Put a new game in the library (unsaved). Returns its id and name.
    fn insert_game(&mut self, m: AddModal) -> (String, String) {
        let proton_dir = match m.kind {
            GameKind::Windows => self
                .proton_tools
                .get(m.proton_idx)
                .map(|t| t.dir.to_string_lossy().to_string())
                .unwrap_or_default(),
            GameKind::Linux => String::new(),
        };
        // A known umu-database entry turns protonfixes on from the first launch.
        let umu_id = match m.kind {
            GameKind::Windows => self.umu_match(m.name.trim()).map(|e| e.umu_id).unwrap_or_default(),
            GameKind::Linux => String::new(),
        };
        let g = CustomGame {
            id: new_game_id(),
            name: m.name.trim().to_string(),
            kind: m.kind,
            exe: m.exe,
            proton_dir,
            flags: self.cfg.settings.new_game_flags(),
            umu_id,
            added: unix_now(),
            ..Default::default()
        };
        let (id, name) = (g.id.clone(), g.name.clone());
        self.cfg.custom_games.push(g);
        self.motion.fresh.insert(id.clone(), None);
        (id, name)
    }

    fn game_act(&mut self, ctx: &egui::Context, id: &str, act: GameAct) {
        let Some(i) = self.index_of(id) else {
            return;
        };
        let g = self.cfg.custom_games[i].clone();
        let settings = self.cfg.settings.clone();
        let report = |app: &mut App, r: Result<String, String>| match r {
            Ok(msg) => app.set_status(msg, false),
            Err(e) => app.set_status(e, true),
        };
        match act {
            GameAct::Play => {
                self.motion.ripple.insert(id.to_string(), self.now);
                self.play(i);
            }
            GameAct::Stop => self.stop(id),
            GameAct::Open => {
                self.selected = Some(g.id);
                self.page = Page::Game;
            }
            GameAct::Artwork => {
                self.selected = Some(g.id);
                self.open_artwork(ctx, i);
            }
            GameAct::Saves => {
                self.selected = Some(g.id);
                self.saves.view = None;
                self.page = Page::Saves;
            }
            GameAct::Shortcut => {
                let r = write_desktop_entry(&g, &settings)
                    .map(|p| lfmt("Added to the app menu: {}", &[&p.display()]));
                report(self, r);
            }
            GameAct::CopyCommand => match launch_command_string(&g, &settings) {
                Ok(c) => self.copy_text(&c),
                Err(e) => self.set_status(e, true),
            },
            GameAct::InitPrefix | GameAct::Winecfg => {
                let (r, msg) = match act {
                    GameAct::InitPrefix => (init_prefix(&g, &settings), lfmt("Initializing prefix for {}…", &[&g.name])),
                    _ => (run_winecfg(&g, &settings), lg("Opening winecfg…").to_string()),
                };
                let r = r.map(|child| {
                    self.children.push(child);
                    msg
                });
                report(self, r);
            }
            GameAct::OpenPrefix => {
                self.children.extend(xdg_open(game_prefix(&g, &settings)));
                self.set_status(lg("Opened prefix folder").into(), false);
            }
            GameAct::OpenGameFolder => {
                let dir = game_workdir(&g).filter(|d| d.is_dir());
                match dir {
                    Some(d) => {
                        self.children.extend(xdg_open(&d));
                        self.set_status(lfmt("Opened {}", &[&d.display()]), false);
                    }
                    None => self.set_status(lfmt("{}’s folder can’t be found", &[&g.name]), true),
                }
            }
            GameAct::OpenLog => {
                let log = data_dir().join("logs").join(format!("{}.log", slugify(&g.name)));
                if log.is_file() {
                    self.children.extend(xdg_open(&log));
                } else {
                    self.set_status(lfmt("No log yet for {}", &[&g.name]), true);
                }
            }
            GameAct::AskRemove => self.confirm_remove = Some(g.id),
            GameAct::Remove => self.remove_game(i),
            GameAct::SetFlags(f) => {
                self.cfg.custom_games[i].flags = f;
                save_config(&self.cfg);
            }
            GameAct::SetKind(k) => {
                self.cfg.custom_games[i].kind = k;
                save_config(&self.cfg);
                self.forget_art(id);
            }
            GameAct::SetProton(n) => {
                if let Some(t) = self.proton_tools.get(n) {
                    self.cfg.custom_games[i].proton_dir = t.dir.to_string_lossy().into();
                    let label = t.label.clone();
                    save_config(&self.cfg);
                    self.set_status(lfmt("{} now uses {}", &[&g.name, &label]), false);
                }
            }
            GameAct::BrowseExe => {
                if let Some(p) = pick_executable() {
                    self.edit.exe = p.to_string_lossy().into();
                    self.commit_field(i, Field::Exe);
                }
            }
            GameAct::Commit(f) => self.commit_field(i, f),
            GameAct::SetGamescope(o) => {
                self.cfg.custom_games[i].gamescope_opts = o;
                save_config(&self.cfg);
            }
            GameAct::SetTuning(t) => {
                self.cfg.custom_games[i].tuning = t;
                save_config(&self.cfg);
            }
            GameAct::SetStatus(st) => {
                self.cfg.custom_games[i].status = st;
                save_config(&self.cfg);
            }
            GameAct::ToggleCollection(c) => {
                let list = &mut self.cfg.custom_games[i].collections;
                if let Some(at) = list.iter().position(|x| *x == c) {
                    list.remove(at);
                } else {
                    list.push(c);
                }
                save_config(&self.cfg);
            }
            GameAct::Favorite => {
                let fav = !g.favorite;
                self.cfg.custom_games[i].favorite = fav;
                save_config(&self.cfg);
                let msg = if fav { "Added {} to favorites" } else { "Removed {} from favorites" };
                self.set_status(lfmt(msg, &[&g.name]), false);
            }
            GameAct::SetNoFixes(off) => {
                self.cfg.custom_games[i].flags.no_protonfixes = off;
                save_config(&self.cfg);
            }
            GameAct::SetUmuId(umu) => {
                self.cfg.custom_games[i].umu_id = umu.clone();
                if self.edit.id == id {
                    self.edit.umu_id = umu.clone();
                }
                save_config(&self.cfg);
                self.set_status(lfmt("{} now uses {}", &[&g.name, &umu]), false);
            }
        }
    }

    fn apply_art_cmds(&mut self, ctx: &egui::Context, cmds: Vec<ArtCmd>) {
        for cmd in cmds {
            match cmd {
                ArtCmd::Search => self.art_search(ctx),
                ArtCmd::SelectMatch(id) => {
                    if self.art.sgdb_id != Some(id) {
                        self.art.sgdb_id = Some(id);
                        self.art_list(ctx, 0);
                    }
                }
                ArtCmd::Kind(k) => {
                    if self.art.kind != k {
                        self.art.kind = k;
                        self.art_list(ctx, 0);
                    }
                }
                ArtCmd::More => {
                    let next = self.art.page + 1;
                    self.art_list(ctx, next);
                }
                ArtCmd::Pick(n) => self.art.picked = Some(n),
                ArtCmd::Apply => self.art_apply(ctx),
                ArtCmd::Local(kind) => {
                    if let Some(id) = self.art_game_id()
                        && let Some(p) = pick_image()
                    {
                        self.set_art(&id, kind, p.to_string_lossy().to_string());
                        self.set_status(lfmt("Custom {} set", &[&lg(kind.noun())]), false);
                    }
                }
                ArtCmd::Reset(kind) => {
                    if let Some(id) = self.art_game_id() {
                        self.set_art(&id, kind, String::new());
                        self.set_status(lfmt("{} reset", &[&lg(kind.noun())]), false);
                    }
                }
                ArtCmd::SaveKey => {
                    let key = std::mem::take(&mut self.art.key_input);
                    if key.trim().is_empty() {
                        continue;
                    }
                    self.cfg.settings.steamgriddb_key = key.trim().to_string();
                    save_config(&self.cfg);
                    self.set_status(lg("SteamGridDB key saved").into(), false);
                    if let Some(i) = self.art.game {
                        self.art_select_game(ctx, i);
                    }
                }
            }
        }
    }

    /// Name a new collection (adding the game it was opened from), or rename one.
    fn collection_modal(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.collection_edit.take() else { return };
        let mut done = false;
        let mut cancel = false;
        let title = if edit.renaming.is_some() { "Rename collection" } else { "New collection" };
        let resp = egui::Modal::new(egui::Id::unique("collection-edit")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new(tr(title)).size(18.0).strong().color(pal().text));
            ui.add_space(10.0);
            let (r, _) = text_field(ui, &mut edit.name, "e.g. Co-op, Soulslikes…", f32::INFINITY);
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = !edit.name.trim().is_empty();
                ui.add_enabled_ui(ok, |ui| {
                    if primary_button(ui, if edit.renaming.is_some() { "Rename" } else { "Create" }).clicked() {
                        done = true;
                    }
                });
                done |= ok && enter;
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        }));
        if done {
            let name = edit.name.trim().to_string();
            let settings = &mut self.cfg.settings;
            match &edit.renaming {
                Some(old) => {
                    for c in settings.collections.iter_mut().filter(|c| *c == old) {
                        c.clone_from(&name);
                    }
                    for g in &mut self.cfg.custom_games {
                        for c in g.collections.iter_mut().filter(|c| *c == old) {
                            c.clone_from(&name);
                        }
                    }
                    if self.lib_filter == LibFilter::Collection(old.clone()) {
                        self.lib_filter = LibFilter::Collection(name.clone());
                    }
                }
                None => {
                    if !settings.collections.contains(&name) {
                        settings.collections.push(name.clone());
                    }
                    if let Some(i) = edit.game.as_deref().and_then(|id| self.index_of(id))
                        && !self.cfg.custom_games[i].collections.contains(&name)
                    {
                        self.cfg.custom_games[i].collections.push(name.clone());
                    }
                }
            }
            save_config(&self.cfg);
            return;
        }
        if !(cancel || resp.should_close()) {
            self.collection_edit = Some(edit);
        }
    }

    fn apply_acts(&mut self, ctx: &egui::Context, acts: Acts) {
        if let Some(t) = acts.copy {
            self.copy_text(&t);
        }
        if let Some(target) = acts.open {
            // Folders we own may not exist yet (no logs before the first launch…).
            if target.starts_with('/') && !Path::new(&target).exists() {
                let _ = fs::create_dir_all(&target);
            }
            self.children.extend(xdg_open(&target));
        }
        if acts.rescan {
            self.refresh();
        }
        if acts.save_settings {
            save_config(&self.cfg);
            // Tool paths may have changed.
            self.deps = self.dep_rows();
            self.set_status(lg("Settings saved").into(), false);
        }
        if acts.test_key
            && let Some(key) = self.sgdb_key()
        {
            self.key_checking = true;
            self.key_check = None;
            let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let result = sgdb_search("Portal", &key).map(|_| ());
                let _ = tx.send(ArtMsg::KeyChecked(result));
                ctx.request_repaint();
            });
        }
        if acts.clear_cache {
            let freed = self.cache_bytes.unwrap_or(0);
            // Only derived images: downloaded artwork (art_dir) is referenced by the config.
            let _ = fs::remove_dir_all(thumb_cache_dir());
            let _ = fs::remove_dir_all(icons_dir());
            self.icon_tex.clear();
            self.icon_fail.clear();
            self.art.thumbs.clear();
            self.art.thumb_failed.clear();
            self.cache_bytes = None;
            self.set_status(lfmt("Cleared {} of cached images", &[&format_bytes(freed)]), false);
        }
        if let Some(action) = acts.dep {
            let steam_root = self.steam_root.clone();
            match action {
                DepAction::Proton => self.start_download(lg("Downloading GE-Proton"), move || {
                    install_ge_proton(steam_root)
                }),
                DepAction::Umu => self.start_download(lg("Downloading umu-run"), install_umu),
            }
        }
        if acts.back {
            self.page = match self.page {
                Page::Artwork | Page::Saves if self.selected_index().is_some() => Page::Game,
                _ => Page::Library,
            };
        }
        if let Some(p) = acts.goto {
            self.page = p;
        }
        if acts.add
            && let Some(p) = pick_executable()
        {
            self.add_modal = Some(self.new_add_modal(p));
        }
        if acts.scan
            && let Some(dir) = pick_folder(lg("Folder with your games"))
        {
            self.start_scan(ctx, dir);
        }
        if acts.steam_import {
            self.open_steam_import();
        }
        if acts.lutris_import {
            self.open_lutris_import();
        }
        if let Some(edit) = acts.collection {
            self.collection_edit = Some(edit);
        }
        if let Some(name) = acts.delete_collection {
            self.cfg.settings.collections.retain(|c| *c != name);
            for g in &mut self.cfg.custom_games {
                g.collections.retain(|c| *c != name);
            }
            if self.lib_filter == LibFilter::Collection(name.clone()) {
                self.lib_filter = LibFilter::All;
            }
            save_config(&self.cfg);
            self.set_status(lfmt("Deleted the collection “{}”", &[&name]), false);
        }
        if acts.fill_art {
            let ids = self.needs_art();
            self.auto_fetch(ctx, ids);
        }
        self.apply_art_cmds(ctx, acts.art);
        // Edits first, so a Play in the same frame launches with them saved.
        let (plays, rest): (Vec<_>, Vec<_>) = acts
            .game
            .into_iter()
            .partition(|(_, a)| matches!(a, GameAct::Play | GameAct::Stop));
        for (id, act) in rest.into_iter().chain(plays) {
            self.game_act(ctx, &id, act);
        }
        // Left the game page (sidebar, back, remove…): save any field still being
        // edited, since its widget won't be drawn again to report the focus loss.
        if self.page != Page::Game && !self.edit.id.is_empty() {
            if let Some(i) = self.index_of(&self.edit.id) {
                for f in [Field::Name, Field::Args, Field::Workdir, Field::Prefix, Field::UmuId, Field::Exe, Field::Env] {
                    self.commit_field(i, f);
                }
            }
            self.edit = EditBuf::default();
        }
    }

    /// Library order: search filter, then the chosen sort.
    fn library_order(&self) -> Vec<usize> {
        self.library_order_for(&self.search)
    }

    /// Does game `i` pass filter `f`?
    fn filter_matches(&self, f: &LibFilter, i: usize) -> bool {
        let g = &self.cfg.custom_games[i];
        match f {
            LibFilter::All => true,
            LibFilter::Favorites => g.favorite,
            LibFilter::Unplayed => self.stats.get(&g.id).is_none_or(|s| s.last_played == 0),
            LibFilter::Status(s) => g.status == *s,
            LibFilter::Collection(c) => g.collections.contains(c),
        }
    }

    /// The filters worth offering: All and Not played yet always; Favorites,
    /// each status and each collection once at least one game is in it.
    /// (filter, logical label)
    fn filter_options(&self) -> Vec<(LibFilter, String)> {
        let mut out = vec![(LibFilter::All, lg("All").to_string())];
        let n = self.cfg.custom_games.len();
        let any = |f: &LibFilter| (0..n).any(|i| self.filter_matches(f, i));
        if any(&LibFilter::Favorites) {
            out.push((LibFilter::Favorites, lg("Favorites").to_string()));
        }
        out.push((LibFilter::Unplayed, lg("Not played yet").to_string()));
        for s in GameStatus::ALL {
            if any(&LibFilter::Status(s)) {
                out.push((LibFilter::Status(s), lg(s.label()).to_string()));
            }
        }
        for c in &self.cfg.settings.collections {
            let f = LibFilter::Collection(c.clone());
            if any(&f) {
                out.push((f, c.clone()));
            }
        }
        out
    }

    /// `library_order` with search text `query`.
    fn library_order_for(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let games = &self.cfg.custom_games;
        let mut v: Vec<usize> = (0..games.len())
            .filter(|&i| q.is_empty() || games[i].name.to_lowercase().contains(&q))
            .filter(|&i| self.filter_matches(&self.lib_filter, i))
            .collect();
        let stat = |i: usize| self.stats.get(&games[i].id).cloned().unwrap_or_default();
        let name = |i: usize| games[i].name.to_lowercase();
        match self.sort {
            SortMode::Name => v.sort_by_key(|&i| name(i)),
            SortMode::Recent => v.sort_by(|&a, &b| {
                stat(b)
                    .last_played
                    .cmp(&stat(a).last_played)
                    .then_with(|| name(a).cmp(&name(b)))
            }),
            SortMode::Playtime => v.sort_by(|&a, &b| {
                stat(b)
                    .playtime_secs
                    .cmp(&stat(a).playtime_secs)
                    .then_with(|| name(a).cmp(&name(b)))
            }),
        }
        // Favourites lead whatever the sort (a stable sort keeps their order).
        v.sort_by_key(|&i| !games[i].favorite);
        v
    }

    fn page_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let sort_before = self.sort;
        // Right-click on empty space: library-wide actions. Registered first, so
        // posters and controls drawn on top keep their own clicks and menus.
        let bg = ui.interact(ui.clip_rect(), ui.scope_id().with("library-bg"), egui::Sense::click());
        let can_fill = self.sgdb_key().is_some() && self.auto_pending.is_empty() && !self.needs_art().is_empty();
        bg.context_menu(|ui| menu_ui(ui, |ui| {
            if ui.button(tr("Add game…")).clicked() {
                acts.add = true;
                ui.close();
            }
            if ui.button(tr("Scan a folder for games…")).clicked() {
                acts.scan = true;
                ui.close();
            }
            if ui.button(tr("Non-Steam games from Steam…")).clicked() {
                acts.steam_import = true;
                ui.close();
            }
            ui.menu_button(tr("Sort by"), |ui| menu_ui(ui, |ui| {
                for m in [SortMode::Recent, SortMode::Name, SortMode::Playtime] {
                    if ui.selectable_label(self.sort == m, tr(m.label())).clicked() {
                        self.sort = m;
                        ui.close();
                    }
                }
            }));
            ui.menu_button(tr("Tile size"), |ui| menu_ui(ui, |ui| {
                for (key, label, _) in TILE_SIZES {
                    if ui.selectable_label(self.cfg.settings.tile_size_key() == key, tr(label)).clicked() {
                        self.cfg.settings.tile_size = key.into();
                        save_config(&self.cfg);
                        ui.close();
                    }
                }
            }));
            if can_fill && ui.button(tr("Fill missing artwork")).clicked() {
                acts.fill_art = true;
                ui.close();
            }
            ui.separator();
            if ui.button(tr("Refresh")).clicked() {
                acts.rescan = true;
                ui.close();
            }
        }));
        ui.horizontal(|ui| {
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(tr("Library")).size(24.0).strong().color(pal().text));
                let n = self.cfg.custom_games.len();
                let playing = self.running.len();
                let games = if n == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&n]) };
                let sub = match playing {
                    0 => disp(&games),
                    p => disp(&format!("{games} · {}", lfmt("{} playing", &[&p]))),
                };
                ui.label(egui::RichText::new(sub).color(pal().subtext).small());
            });
            ui.with_layout(far_end(), |ui| {
                if primary_button(ui, "+  Add game").clicked() {
                    acts.add = true;
                }
                ui.menu_button(tr("Import"), |ui| menu_ui(ui, |ui| {
                    if ui.button(tr("Scan a folder for games…")).on_hover_text(tr("Find every game in a folder and add them together")).clicked() {
                        acts.scan = true;
                        ui.close();
                    }
                    if ui.button(tr("Non-Steam games from Steam…")).on_hover_text(tr("Bring over games you added to Steam, with their prefixes and artwork")).clicked() {
                        acts.steam_import = true;
                        ui.close();
                    }
                    if ui.button(tr("Games from Lutris…")).on_hover_text(tr("Bring over games you installed with Lutris, with their prefixes")).clicked() {
                        acts.lutris_import = true;
                        ui.close();
                    }
                }));
                if !self.auto_pending.is_empty() {
                    ui.label(egui::RichText::new(tr("Fetching artwork…")).color(pal().subtext).small());
                    ui.spinner();
                } else if self.sgdb_key().is_some()
                    && !self.needs_art().is_empty()
                    && ui.button(tr("Fill missing artwork")).clicked()
                {
                    acts.fill_art = true;
                }
            });
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let (search, _) = text_field(ui, &mut self.search, "Search games…", 280.0);
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                search.request_focus();
            }
            ui.with_layout(far_end(), |ui| {
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(tr(self.sort.label()))
                    .show_ui(ui, |ui| menu_ui(ui, |ui| {
                        for m in [SortMode::Recent, SortMode::Name, SortMode::Playtime] {
                            ui.selectable_value(&mut self.sort, m, tr(m.label()));
                        }
                    }));
                ui.label(egui::RichText::new(tr("Sort")).color(pal().subtext).small());
                ui.add_space(10.0);
                if let Some(k) = tile_size_toggle(ui, &self.cfg.settings) {
                    self.set_layout(k);
                }
            });
        });
        // Filters: the fixed ones, then statuses and collections that have games.
        let options = self.filter_options();
        if !options.iter().any(|(f, _)| *f == self.lib_filter) {
            self.lib_filter = LibFilter::All;
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            for (f, label) in options {
                let r = choice_chip(ui, self.lib_filter == f, &label);
                if r.clicked() {
                    self.lib_filter = f.clone();
                }
                if let LibFilter::Collection(name) = &f {
                    r.context_menu(|ui| menu_ui(ui, |ui| {
                        if ui.button(tr("Rename collection…")).clicked() {
                            acts.collection = Some(CollectionEdit { game: None, renaming: Some(name.clone()), name: name.clone() });
                            ui.close();
                        }
                        if ui.button(egui::RichText::new(tr("Delete collection")).color(pal().danger)).clicked() {
                            acts.delete_collection = Some(name.clone());
                            ui.close();
                        }
                    }));
                }
            }
        });
        ui.add_space(18.0);

        if self.cfg.custom_games.is_empty() {
            empty_hint(ui, "Your library is empty — click “+ Add game” and pick a game’s executable.");
            return;
        }
        if self.sort != sort_before {
            self.cfg.settings.library_sort = self.sort.key().into();
            save_config(&self.cfg);
        }
        let browsing = self.search.trim().is_empty() && self.lib_filter == LibFilter::All;
        if browsing {
            self.jump_back_in(ui, ctx, acts);
        }
        let order = self.library_order();
        if order.is_empty() {
            let why = match self.lib_filter {
                LibFilter::Unplayed if self.search.trim().is_empty() => "You’ve played every game in your library.",
                _ => "No games match your search.",
            };
            empty_hint(ui, why);
            return;
        }
        let avail = ui.available_width() - 1.0;
        let cols = grid_cols(avail);
        let gap = grid_gap(avail);
        if self.cfg.settings.library_view == "list" {
            self.library_keys(ctx, &order, 1, acts);
            self.library_list(ui, ctx, &order, acts);
            return;
        }
        self.library_keys(ctx, &order, cols as isize, acts);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, 22.0);
            let origin = ui.max_rect().min;
            self.motion.grid_origin = origin;
            for i in order {
                self.library_tile(ui, ctx, i, origin, acts);
            }
            // Removed posters shrink and fade where they stood.
            let (now, secs) = (self.now, self.secs(0.3));
            self.motion.ghosts.retain(|g| now - g.since < f64::from(secs));
            self.motion.busy |= !self.motion.ghosts.is_empty();
            for g in &self.motion.ghosts {
                let t = ease_since(now, g.since, secs);
                let r = g.rel.translate(origin.to_vec2());
                let r = egui::Rect::from_center_size(r.center(), r.size() * (1.0 - 0.15 * t));
                if let Some(tex) = &g.tex {
                    ui.painter().add(cover_fill_shape(tex, r, 10, 1.0 - t));
                }
            }
            if browsing {
                let (slot, resp) =
                    ui.allocate_exact_size(egui::vec2(tile_w(), tile_h() + 46.0), egui::Sense::click());
                let rect = self.glide("+add", slot, origin);
                add_tile(ui, rect, resp, acts);
            }
        });
    }

    /// A pick from the layout buttons: "list", or a grid tile size.
    fn set_layout(&mut self, k: &str) {
        if k == "list" {
            self.cfg.settings.library_view = "list".into();
        } else {
            self.cfg.settings.library_view = "grid".into();
            self.cfg.settings.tile_size = k.into();
        }
        save_config(&self.cfg);
    }

    /// Arrow keys move a focus ring over the grid; Enter opens the game's page
    /// and Space plays it. Off while a text field has focus.
    fn library_keys(&mut self, ctx: &egui::Context, order: &[usize], cols: isize, acts: &mut Acts) {
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            self.lib_keys = false;
        }
        if ctx.memory(|m| m.focused().is_some()) || overlay_open(ctx) {
            return;
        }
        let (step, enter, space) = ctx.input(|i| {
            let k = |key| i.key_pressed(key) as isize;
            let side = k(egui::Key::ArrowRight) - k(egui::Key::ArrowLeft);
            let side = if rtl() { -side } else { side };
            let step = side + cols * (k(egui::Key::ArrowDown) - k(egui::Key::ArrowUp));
            (step, i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Space))
        });
        let games = &self.cfg.custom_games;
        let cur = self.lib_cursor.as_ref().and_then(|id| order.iter().position(|&i| &games[i].id == id));
        if step != 0 {
            let next = match cur {
                Some(c) if self.lib_keys => (c as isize + step).clamp(0, order.len() as isize - 1) as usize,
                // The first press shows the ring where it was (or on the first game).
                Some(c) => c,
                None => 0,
            };
            self.lib_cursor = Some(games[order[next]].id.clone());
            self.lib_keys = true;
            self.lib_scroll = true;
        } else if self.lib_keys
            && let Some(c) = cur
        {
            let id = games[order[c]].id.clone();
            if enter {
                acts.game.push((id, GameAct::Open));
            } else if space {
                let act = if self.running.contains_key(&id) { GameAct::Stop } else { GameAct::Play };
                acts.game.push((id, act));
            }
        }
    }

    fn library_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        i: usize,
        origin: egui::Pos2,
        acts: &mut Acts,
    ) {
        let (slot, resp) =
            ui.allocate_exact_size(egui::vec2(tile_w(), tile_h() + 46.0), egui::Sense::click());
        let g = &self.cfg.custom_games[i];
        let (id, name) = (g.id.clone(), g.name.clone());
        let fav = g.favorite;
        let missing = game_missing_files(g);
        let rect = self.glide(&id, slot, origin);
        let rect = rect.translate(egui::vec2(self.shake_dx(&id), 0.0));
        let stat = self.stats.get(&id).cloned().unwrap_or_default();
        // A game added this session pops into the grid the first time it's seen.
        let pop = match self.motion.fresh.get(&id).copied() {
            None => 1.0,
            Some(born) => {
                let born = born.unwrap_or(self.now);
                self.motion.fresh.insert(id.clone(), Some(born));
                let secs = self.secs(0.5);
                let raw = if secs > 0.0 { ((self.now - born) as f32 / secs).clamp(0.0, 1.0) } else { 1.0 };
                if raw >= 1.0 {
                    self.motion.fresh.remove(&id);
                } else {
                    self.motion.busy = true;
                }
                egui::emath::easing::back_out(raw)
            }
        };
        let running = self.running.contains_key(&id);
        let fetching = self.auto_pending.contains(&id);
        let cover = self.game_poster(ctx, i);
        let generated = cover.is_none();
        let tex = cover.or_else(|| self.game_generated_poster(ctx, i));

        let launching = !running && self.motion.launching.contains_key(&id);
        let focused = self.lib_keys && self.lib_cursor.as_deref() == Some(id.as_str());
        if focused && std::mem::take(&mut self.lib_scroll) {
            ui.scroll_to_rect(rect.expand(16.0), None);
        }
        let hovered = resp.hovered() || focused;
        // `h` eases the shading in; `spring` overshoots a little, for the lift
        // and the pop; `press` squashes the poster while the button is held.
        let h = ctx.animate_bool_with_time_and_easing(
            egui::Id::unique(("tile-hover", &id)),
            hovered,
            self.secs(0.16),
            egui::emath::easing::cubic_out,
        );
        let spring = ctx.animate_bool_with_time_and_easing(
            egui::Id::unique(("tile-spring", &id)),
            hovered,
            self.secs(0.34),
            egui::emath::easing::back_out,
        );
        let press = ctx.animate_bool_with_time(
            egui::Id::unique(("tile-press", &id)),
            resp.is_pointer_button_down_on(),
            self.secs(0.07),
        );
        let poster = egui::Rect::from_min_size(rect.min, egui::vec2(tile_w(), tile_h()));
        self.motion.tile_rel.insert(id.clone(), poster.translate(-origin.to_vec2()));
        let poster = egui::Rect::from_center_size(
            poster.center() + egui::vec2(0.0, 14.0 * (1.0 - pop) - 7.0 * spring + 3.0 * press),
            poster.size() * (0.86 + 0.14 * pop) * (1.0 + 0.04 * spring - 0.05 * press),
        );
        // Where the pointer is over the poster (-1..1 each way), eased, so the
        // art can drift toward it and a light can follow it.
        let pointer = resp.hover_pos().filter(|_| resp.hovered());
        let aim = pointer.map_or(egui::Vec2::ZERO, |p| {
            ((p - poster.center()) / (poster.size() / 2.0)).clamp(egui::vec2(-1.0, -1.0), egui::vec2(1.0, 1.0))
        });
        let aim = egui::vec2(
            ctx.animate_value_with_time(egui::Id::unique(("tile-aim-x", &id)), aim.x, self.secs(0.18)),
            ctx.animate_value_with_time(egui::Id::unique(("tile-aim-y", &id)), aim.y, self.secs(0.18)),
        );
        let art_pan = if self.cfg.settings.reduce_motion {
            egui::vec2(0.5, 0.5)
        } else {
            egui::vec2(0.5 + 0.5 * aim.x, 0.5 + 0.5 * aim.y)
        };
        let art_zoom = 1.0 + 0.07 * h;
        let radius = rad(10);
        // Coming back from its page, the poster is still flying home: leave its
        // slot empty (the title and playtime below still show).
        let mut painter = ui.painter().clone();
        if self.fly(ctx, &id, true, poster) {
            painter.set_invisible();
        }
        if h > 0.0 {
            // A few widening layers make a soft shadow that deepens as it lifts.
            for (grow, alpha) in [(2.0, 70.0), (6.0, 40.0), (11.0, 22.0)] {
                painter.rect_filled(
                    poster.translate(egui::vec2(0.0, 5.0 + 7.0 * spring)).expand(grow * spring.max(0.0)),
                    rad(10 + grow as u8),
                    theme::shadow(alpha * h),
                );
            }
        }
        // New cover art dissolves in over the generated poster it replaces.
        let fade = self.motion.art_arrived.get(&id).copied().map(|t| self.progress(t, 0.7));
        if fade == Some(1.0) {
            self.motion.art_arrived.remove(&id);
        }
        match &tex {
            Some(t) => {
                let alpha = pop.min(1.0);
                if let Some(f) = fade.filter(|&f| f < 1.0 && !generated) {
                    if let Some(old) = self.game_generated_poster(ctx, i) {
                        painter.add(cover_fill_shape_pan(&old, poster, 10, alpha, art_zoom, art_pan));
                    }
                    painter.add(cover_fill_shape_pan(t, poster, 10, f * alpha, art_zoom, art_pan));
                } else {
                    painter.add(cover_fill_shape_pan(t, poster, 10, alpha, art_zoom, art_pan));
                }
            }
            None => {
                painter.rect_filled(poster, radius, pal().card2);
            }
        }
        // Text and badges over the poster use a dark palette in every theme.
        let over = theme::OverImage::new();
        if generated {
            // Generated posters carry the name; real covers already show the title.
            let galley = painter.layout(
                disp(&name),
                egui::FontId::proportional(15.0),
                pal().text,
                poster.width() - 24.0,
            );
            let pos = egui::pos2(poster.center().x - galley.size().x / 2.0, poster.top() + poster.height() * 0.62);
            painter.galley(pos, galley, pal().text);
        }

        // While artwork is being fetched, a band of light sweeps the poster.
        if fetching {
            if self.cfg.settings.reduce_motion {
                poster_badge(&painter, poster.left_bottom() + egui::vec2(8.0, -28.0), "Fetching art", pal().accent, 1.0);
            } else {
                let t = self.sweep(1.6);
                sheen(&painter.with_clip_rect(poster), poster, t);
            }
        }

        // Badges, top-left. A running game's dot breathes.
        let pulse = if running { self.pulse(2.4) } else { 1.0 };
        let mut y = poster.top() + 8.0;
        if running {
            y += poster_badge(&painter, egui::pos2(poster.left() + 8.0, y), "Running", pal().ok, pulse);
        }
        if missing {
            poster_badge(&painter, egui::pos2(poster.left() + 8.0, y), "Missing files", pal().warn, 1.0);
        }
        if fav {
            let c = poster.right_top() + egui::vec2(-18.0, 18.0);
            painter.circle_filled(c, 12.0, egui::Color32::from_black_alpha(170));
            star(&painter, c, 7.5, true, GOLD);
        }
        let status = self.cfg.custom_games[i].status;
        if status != GameStatus::None {
            poster_badge(&painter, poster.left_bottom() + egui::vec2(8.0, -28.0), status.label(), status.color(), 1.0);
        }

        let center = poster.center();
        let play_r = 27.0;
        let over_play = hovered
            && !launching
            && !(running && self.stopping.contains(&id))
            && resp
                .hover_pos()
                .is_some_and(|p| p.distance(center) <= play_r + 4.0);
        let stopping = running && self.stopping.contains(&id);
        let working = if launching {
            Some(("Starting…", pal().accent))
        } else if stopping {
            Some(("Stopping…", pal().danger))
        } else {
            None
        };
        if let Some((label, color)) = working {
            // Between a click and the game window opening or closing: a quiet ring.
            painter.rect_filled(poster, radius, egui::Color32::from_black_alpha(130));
            let angle = self.sweep(1.1) * std::f32::consts::TAU;
            spinner_arc(&painter, center, 22.0, angle, color);
            painter.text(
                center + egui::vec2(0.0, 36.0),
                egui::Align2::CENTER_TOP,
                tr(label),
                egui::FontId::proportional(13.0),
                pal().text,
            );
        } else if h > 0.0 {
            // Hover: dim the poster a little, let a soft light follow the
            // pointer, and spring a round play/stop button in.
            painter.rect_filled(poster, radius, egui::Color32::from_black_alpha((70.0 * h) as u8));
            if pointer.is_some() {
                let light = poster.center() + aim * poster.size() / 2.0;
                glow_spot(&painter.with_clip_rect(poster.shrink(1.0)), light, poster.width() * 0.95, egui::Color32::from_white_alpha((80.0 * h) as u8));
            }
            let fill = match (running, over_play) {
                (true, true) => pal().danger,
                (true, false) => pal().danger.gamma_multiply(0.8),
                (false, true) => pal().accent,
                (false, false) => pal().accent.gamma_multiply(0.85),
            };
            // Over the button itself, it swells and gets a halo.
            let swell = ctx.animate_bool_with_time_and_easing(
                egui::Id::unique(("tile-over-play", &id)),
                over_play,
                self.secs(0.22),
                egui::emath::easing::back_out,
            );
            let r = play_r * (0.55 + 0.45 * spring) * (1.0 + 0.12 * swell - 0.08 * press);
            if swell > 0.0 {
                let halo = egui::Stroke::new(2.0, fill.gamma_multiply(0.45 * swell.clamp(0.0, 1.0)));
                if pal().square {
                    painter.rect_stroke(egui::Rect::from_center_size(center, egui::vec2(2.0, 2.0) * (r + 6.0 * swell)), 0, halo, egui::StrokeKind::Outside);
                } else {
                    painter.circle_stroke(center, r + 6.0 * swell, halo);
                }
            }
            if pal().square {
                painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r)), 0, fill.gamma_multiply(h));
            } else {
                painter.circle_filled(center, r, fill.gamma_multiply(h));
            }
            let icon = pal().on_accent.gamma_multiply(h);
            if running {
                painter.rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(16.0, 16.0) * (r / play_r)),
                    rad(2),
                    icon,
                );
            } else {
                play_triangle(&painter, center, 11.0 * (r / play_r), icon);
            }
        }
        drop(over);
        let border = if running && h < 0.5 {
            egui::Stroke::new(1.5, pal().ok.gamma_multiply(0.3 + 0.5 * pulse))
        } else {
            egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h))
        };
        painter.rect_stroke(poster, radius, border, egui::StrokeKind::Inside);
        // A faint accent glow around a lifted poster.
        if h > 0.0 && !(running && h < 0.5) {
            painter.rect_stroke(poster.expand(2.0), rad(12), egui::Stroke::new(3.0, pal().accent.gamma_multiply(0.18 * h)), egui::StrokeKind::Outside);
        }
        // Play was pressed: two rings burst out from the button.
        if let Some(&since) = self.motion.ripple.get(&id) {
            let secs = self.secs(0.7);
            let t = if secs > 0.0 { ((self.now - since) as f32 / secs).clamp(0.0, 1.0) } else { 1.0 };
            if t >= 1.0 {
                self.motion.ripple.remove(&id);
            } else {
                self.motion.busy = true;
                let ring = painter.with_clip_rect(poster.expand(12.0));
                for delay in [0.0, 0.18] {
                    let k = ((t - delay) / (1.0 - delay)).clamp(0.0, 1.0);
                    if k > 0.0 && k < 1.0 {
                        let e = egui::emath::easing::cubic_out(k);
                        let color = pal().accent.gamma_multiply(1.0 - k);
                        ring.circle_stroke(center, play_r * (0.9 + 2.4 * e), egui::Stroke::new(3.0 * (1.0 - k) + 0.5, color));
                    }
                }
            }
        }

        // Title + playtime under the poster; the title warms to the accent as
        // its poster lifts.
        let mut job = egui::text::LayoutJob::simple_singleline(
            disp(&name),
            egui::FontId::proportional(14.5),
            pal().text.lerp_to_gamma(pal().accent, 0.55 * h),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: tile_w(),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let labels = ui.painter();
        let title = labels.layout_job(job);
        let tx = if rtl() { rect.right() - title.size().x } else { rect.left() };
        labels.galley(egui::pos2(tx, rect.top() + tile_h() + 8.0), title, pal().text);
        // After a session the total counts up to its new value, with a fading "+25m".
        let (playtime, added) = self.shown_playtime(&id, stat.playtime_secs);
        let glow = added.as_ref().map_or(0.0, |(_, a)| *a);
        let (sub, sub_color) = if running {
            (tr("Playing now"), pal().ok)
        } else if stat.last_played == 0 {
            (tr("Never played"), pal().faint)
        } else {
            (
                disp(&format!("{} · {}", format_duration(playtime), format_ago(stat.last_played, unix_now()))),
                pal().subtext.lerp_to_gamma(pal().ok, glow),
            )
        };
        let (sx, salign) = if rtl() { (rect.right(), egui::Align2::RIGHT_TOP) } else { (rect.left(), egui::Align2::LEFT_TOP) };
        let sub_rect = labels.text(
            egui::pos2(sx, rect.top() + tile_h() + 30.0),
            salign,
            sub,
            egui::FontId::proportional(12.0),
            sub_color,
        );
        if let Some((delta, alpha)) = added.filter(|_| !running) {
            let (dpos, dalign) = if rtl() {
                (sub_rect.left_top() - egui::vec2(6.0, 0.0), egui::Align2::RIGHT_TOP)
            } else {
                (sub_rect.right_top() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_TOP)
            };
            labels.text(
                dpos,
                dalign,
                disp(&delta),
                egui::FontId::proportional(12.0),
                pal().ok.gamma_multiply(alpha),
            );
        }

        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            let act = match (over_play, running) {
                (true, true) => GameAct::Stop,
                (true, false) => GameAct::Play,
                (false, _) => GameAct::Open,
            };
            acts.game.push((id.clone(), act));
        }
        let game = self.cfg.custom_games[i].clone();
        resp.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &game, running, false, &self.cfg.settings.collections, acts)));
    }

    fn page_game(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let Some(i) = self.selected_index() else {
            acts.back = true;
            return;
        };
        self.sync_edit(i);
        let g = self.cfg.custom_games[i].clone();
        let id = g.id.clone();
        let run = self.running.get(&id).cloned();
        let stat = self.stats.get(&id).cloned().unwrap_or_default();

        acts.back |= back_button(ui, ctx, "Library");
        ui.add_space(12.0);

        // Hero banner, with the poster, title and actions laid over it.
        let w = ui.available_width();
        let hero_h = (w * 0.32).clamp(250.0, 340.0);
        let (hero, hero_resp) = ui.allocate_exact_size(egui::vec2(w, hero_h), egui::Sense::click());
        let painter = ui.painter().clone();
        let radius = rad(14);
        // Parallax: the banner image drifts slower than the page as it scrolls.
        let drift = (hero.top() - ui.clip_rect().top()) / hero.height();
        let pan = if self.cfg.settings.reduce_motion { 0.5 } else { (0.5 + 0.6 * drift).clamp(0.0, 1.0) };
        // The banner settles from slightly zoomed in when the page opens.
        let settle = self.progress(self.motion.page_since, 0.8);
        let zoom = 1.12 + 0.08 * (1.0 - settle);
        match self.game_hero(ctx, i) {
            Some(t) => {
                painter.add(cover_fill_shape_ex(&t, hero, 14, 1.0, zoom, pan));
            }
            None => {
                painter.rect_filled(hero, radius, pal().card);
            }
        }
        painter.rect_stroke(hero, radius, egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);

        let pad = 24.0;
        let ph = (hero_h - 2.0 * pad).min(220.0);
        let pw = ph * 2.0 / 3.0;
        let px = if rtl() { hero.right() - pad - pw } else { hero.left() + pad };
        let poster = egui::Rect::from_min_size(egui::pos2(px, hero.bottom() - pad - ph), egui::vec2(pw, ph));
        self.motion.hero_poster = Some(poster);
        // Arriving from the grid, the poster is still flying in: draw it once it lands.
        let flying = self.fly(ctx, &id, false, poster);
        if !flying
            && let Some(t) = self
                .game_poster(ctx, i)
                .or_else(|| self.game_generated_poster(ctx, i))
        {
            painter.add(cover_fill_shape(&t, poster, 10, 1.0));
        }
        painter.rect_stroke(
            poster,
            rad(10),
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(45)),
            egui::StrokeKind::Inside,
        );
        let poster_resp = ui
            .interact(poster, ui.scope_id().with("hero-poster"), egui::Sense::click())
            .on_hover_text(tr("Change artwork"))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if poster_resp.clicked() {
            acts.game.push((id.clone(), GameAct::Artwork));
        }
        for r in [&hero_resp, &poster_resp] {
            r.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, run.is_some(), true, &self.cfg.settings.collections, acts)));
        }

        let (play_state, phase) = self.play_state(&id);
        // Enter (a controller's A) plays or stops, unless something else has the keys.
        if ctx.memory(|m| m.focused().is_none())
            && !overlay_open(ctx)
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && matches!(play_state, PlayState::Play | PlayState::Stop)
        {
            let act = if run.is_some() { GameAct::Stop } else { GameAct::Play };
            acts.game.push((id.clone(), act));
        }
        let dx = self.shake_dx(&id);
        let (playtime, added) = self.shown_playtime(&id, stat.playtime_secs);
        let text_area = if rtl() {
            egui::Rect::from_min_max(egui::pos2(hero.left() + pad, hero.top() + pad), egui::pos2(poster.left() - 24.0, hero.bottom() - pad))
        } else {
            egui::Rect::from_min_max(egui::pos2(poster.right() + 24.0, hero.top() + pad), egui::pos2(hero.right() - pad, hero.bottom() - pad))
        };
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(text_area)
                .layout(egui::Layout::bottom_up(if rtl() { egui::Align::Max } else { egui::Align::Min })),
            |ui| {
                let _over = theme::OverImage::new();
                // Bottom-up: the action row goes in first, the title last.
                ui.horizontal(|ui| {
                    match &run {
                        Some(m) => {
                            if play_button(ui, play_state, phase, dx).clicked() {
                                acts.game.push((id.clone(), GameAct::Stop));
                            }
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(trf(
                                    "Running · {}",
                                    &[&format_duration(unix_now().saturating_sub(m.started))],
                                ))
                                .color(pal().ok),
                            );
                        }
                        None => {
                            if play_button(ui, play_state, phase, dx).clicked() {
                                acts.game.push((id.clone(), GameAct::Play));
                            }
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button(tr("Change artwork")).clicked() {
                        acts.game.push((id.clone(), GameAct::Artwork));
                    }
                    ui.menu_button(tr("More"), |ui| menu_ui(ui, |ui| {
                        for (label, act) in [
                            ("Add to app menu", GameAct::Shortcut),
                            ("Copy launch command", GameAct::CopyCommand),
                            ("Open log", GameAct::OpenLog),
                        ] {
                            if ui.button(tr(label)).clicked() {
                                acts.game.push((id.clone(), act));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui
                            .button(egui::RichText::new(tr("Remove from library…")).color(pal().danger))
                            .clicked()
                        {
                            acts.game.push((id.clone(), GameAct::AskRemove));
                            ui.close();
                        }
                    }));
                    let tip = if g.favorite { "Remove from favorites" } else { "Add to favorites" };
                    if star_button(ui, g.favorite).on_hover_text(tr(tip)).clicked() {
                        acts.game.push((id.clone(), GameAct::Favorite));
                    }
                    let shown = if g.status == GameStatus::None { tr("Set status") } else { tr(g.status.label()) };
                    egui::ComboBox::from_id_salt("game-status")
                        .selected_text(egui::RichText::new(shown).color(g.status.color().lerp_to_gamma(pal().text, 0.3)))
                        .show_ui(ui, |ui| menu_ui(ui, |ui| {
                            for st in std::iter::once(GameStatus::None).chain(GameStatus::ALL) {
                                if ui.selectable_label(g.status == st, tr(st.label())).clicked() {
                                    acts.game.push((id.clone(), GameAct::SetStatus(st)));
                                }
                            }
                        }));
                });
                ui.add_space(12.0);
                let played = if stat.last_played == 0 {
                    tr("Never played")
                } else {
                    trf("{} played · last played {}", &[&format_duration(playtime), &format_ago(stat.last_played, unix_now())])
                };
                ui.horizontal(|ui| {
                    let glow = added.as_ref().map_or(0.0, |(_, a)| *a);
                    ui.label(egui::RichText::new(played).color(pal().subtext.lerp_to_gamma(pal().ok, glow)));
                    if let Some((delta, alpha)) = &added {
                        ui.label(egui::RichText::new(disp(delta)).color(pal().ok.gamma_multiply(*alpha)));
                    }
                });
                let mut kind = match g.kind {
                    GameKind::Windows => trf("Windows · {}", &[&self.proton_label(&g.proton_dir)]),
                    GameKind::Linux => tr("Linux native"),
                };
                if let Some(link) = &g.store {
                    kind = format!("{kind} · {}", link.store.name());
                }
                ui.label(egui::RichText::new(kind).color(pal().subtext).small());
                ui.add(
                    egui::Label::new(egui::RichText::new(disp(&g.name)).size(30.0).strong().color(pal().text))
                        .truncate(),
                );
            },
        );
        ui.add_space(18.0);

        if game_missing_files(&g) {
            let what = if !Path::new(&g.exe).is_file() {
                "The game’s executable can’t be found — fix the path under Advanced."
            } else {
                "The selected Proton build is missing — pick another one below."
            };
            card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    status_dot(ui, pal().warn, true);
                    ui.label(egui::RichText::new(tr(what)).color(pal().warn));
                });
            });
            ui.add_space(12.0);
        }

        let tab = self.game_tab_bar(ui, g.kind == GameKind::Windows);
        ui.add_space(14.0);
        match tab {
            GameTab::Overview => {
                self.about_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.glance_card(ui, ctx, i, &stat, acts);
                ui.add_space(12.0);
                self.hltb_card(ui, ctx, i, stat.playtime_secs);
                ui.add_space(12.0);
                self.saves_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.last_session_card(ui, i, &stat, acts);
                return;
            }
            GameTab::Mods => {
                self.opti_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.components_card(ui, i);
                return;
            }
            GameTab::Settings => {}
        }
        section(ui, "Performance", |ui| {
            let mut f = g.flags;
            if flags_ui(ui, &mut f) {
                acts.game.push((id.clone(), GameAct::SetFlags(f)));
            }
            if g.flags.gamescope {
                ui.add_space(10.0);
                let mut o = g.gamescope_opts.clone();
                if gamescope_ui(ui, &mut o) {
                    acts.game.push((id.clone(), GameAct::SetGamescope(o)));
                }
            }
            ui.add_space(10.0);
            let mut t = g.tuning.clone();
            if tuning_ui(ui, &mut t, g.flags.mangohud, g.kind == GameKind::Windows) {
                acts.game.push((id.clone(), GameAct::SetTuning(t)));
            }
        });
        ui.add_space(12.0);
        section(ui, "Launch", |ui| {
            if edit_row(ui, "Name", &mut self.edit.name, "") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Name)));
            }
            if edit_row(ui, "Arguments", &mut self.edit.args, "e.g. -windowed") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Args)));
            }
            if edit_row(ui, "Working dir", &mut self.edit.workdir, "the exe’s folder") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Workdir)));
            }
            modal_row(ui, "Environment", |ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.edit.env)
                        .hint_text("DXVK_HUD=fps\nPROTON_LOG=1")
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if r.lost_focus() {
                    acts.game.push((id.clone(), GameAct::Commit(Field::Env)));
                }
            });
            note(ui, 128.0, "One KEY=value per line. They're added to the launch; WINEDLLOVERRIDES entries are combined with the app's own.");
        });
        ui.add_space(12.0);
        if g.kind == GameKind::Windows {
            let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
            let current = self
                .proton_tools
                .iter()
                .position(|t| t.dir.to_string_lossy() == g.proton_dir);
            let auto_prefix = self
                .cfg
                .settings
                .prefix_base()
                .join(slugify(&g.name))
                .to_string_lossy()
                .to_string();
            let default_id = format!("umu-{}", slugify(&g.name));
            if self.proton.umu_db.is_none() {
                self.load_fix_data(ctx, false);
            }
            let umu_hint = self.umu_match(&g.name);
            let fix = match &self.proton.fixes {
                Some(Ok(f)) => Some(proton::fix_for(f, &game_umu_id(&g))),
                _ => None,
            };
            section(ui, "Proton & prefix", |ui| {
                modal_row(ui, "Proton", |ui| {
                    if tool_labels.is_empty() {
                        pill(ui, "no Proton builds found — see Dependencies", pal().warn);
                    } else {
                        let shown = current.map_or(tr("Choose a Proton build…"), |n| tool_labels[n].clone());
                        egui::ComboBox::from_id_salt("game-proton")
                            .selected_text(shown)
                            .width(260.0)
                            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                for (n, t) in tool_labels.iter().enumerate() {
                                    if ui.selectable_label(current == Some(n), t).clicked() {
                                        acts.game.push((id.clone(), GameAct::SetProton(n)));
                                    }
                                }
                            }));
                    }
                });
                if edit_row(ui, "umu ID", &mut self.edit.umu_id, &default_id) {
                    acts.game.push((id.clone(), GameAct::Commit(Field::UmuId)));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(128.0);
                    let gameid = game_umu_id(&g);
                    match (&umu_hint, &fix) {
                        (Some(e), _) if e.umu_id != gameid => {
                            status_dot(ui, pal().accent, true);
                            ui.label(
                                egui::RichText::new(trf("The umu database lists this game as {}", &[&e.umu_id]))
                                    .color(pal().accent)
                                    .small(),
                            );
                            if ui.small_button(tr("Use it")).clicked() {
                                acts.game.push((id.clone(), GameAct::SetUmuId(e.umu_id.clone())));
                            }
                        }
                        (_, Some(Some(path))) => {
                            status_dot(ui, pal().ok, true);
                            ui.label(egui::RichText::new(tr("A protonfix exists for this ID")).color(pal().ok).small());
                            if ui.small_button(tr("View")).clicked() {
                                acts.open = Some(format!("{}{path}", proton::FIXES_REPO_URL));
                            }
                        }
                        _ => {
                            ui.label(
                                egui::RichText::new(tr(
                                    "A umu-database ID (e.g. umu-1091500) turns on protonfixes for this game.",
                                ))
                                .color(pal().subtext)
                                .small(),
                            );
                        }
                    }
                });
                ui.add_space(6.0);
                if edit_row(ui, "Prefix", &mut self.edit.prefix, &auto_prefix) {
                    acts.game.push((id.clone(), GameAct::Commit(Field::Prefix)));
                }
                ui.horizontal(|ui| {
                    ui.add_space(128.0);
                    for (label, act) in [
                        ("Open prefix folder", GameAct::OpenPrefix),
                        ("Initialize prefix", GameAct::InitPrefix),
                        ("Winecfg", GameAct::Winecfg),
                    ] {
                        if ui.button(tr(label)).clicked() {
                            acts.game.push((id.clone(), act));
                        }
                    }
                });
            });
            ui.add_space(12.0);
        }
        let settings = &self.cfg.settings;
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id_adv = ui.make_persistent_id("game-advanced");
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id_adv, false);
            // egui's collapsing header always puts its arrow on the left: build
            // the row by hand so it starts on the reading side.
            let dir = if rtl() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
            let clicked = ui
                .with_layout(dir, |ui| {
                    let arrow = state.show_toggle_button(ui, disclosure_icon);
                    let label = ui.add(
                        egui::Label::new(egui::RichText::new(tr("Advanced")).strong().color(pal().text)).sense(egui::Sense::click()),
                    );
                    !arrow.clicked() && label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                })
                .inner;
            if clicked {
                state.toggle(ui);
            }
            state.show_body_unindented(ui, |ui| {
                    ui.add_space(6.0);
                    modal_row(ui, "Type", |ui| {
                        for (k, label) in [(GameKind::Windows, "Windows (Proton)"), (GameKind::Linux, "Linux native")] {
                            if choice_chip(ui, g.kind == k, label).clicked() && g.kind != k {
                                acts.game.push((id.clone(), GameAct::SetKind(k)));
                            }
                        }
                    });
                    modal_row(ui, "Executable", |ui| {
                        let w = (ui.available_width() - button_width(ui, "Browse…")).max(120.0);
                        let (r, by_menu) = text_field(ui, &mut self.edit.exe, "", w);
                        if r.lost_focus() || by_menu {
                            acts.game.push((id.clone(), GameAct::Commit(Field::Exe)));
                        }
                        if ui.button(tr("Browse…")).clicked() {
                            acts.game.push((id.clone(), GameAct::BrowseExe));
                        }
                    });
                    ui.add_space(4.0);
                    match &launch_command_string(&g, settings) {
                        Ok(cmd) => {
                            command_preview(ui, cmd);
                            ui.add_space(6.0);
                            if ui.button(tr("Copy launch command")).clicked() {
                                acts.copy = Some(cmd.clone());
                            }
                        }
                        Err(e) => {
                            ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
                        }
                    }
                });
        });
    }

    fn proton_label(&self, dir: &str) -> String {
        self.proton_tools
            .iter()
            .find(|t| t.dir.to_string_lossy() == dir)
            .map(|t| t.label.clone())
            .or_else(|| Path::new(dir).file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| lg("no Proton build").into())
    }

    /// The "add game" dialog. Returns the details once the user confirms.
    fn add_modal_ui(&mut self, ctx: &egui::Context) -> Option<AddModal> {
        let mut m = self.add_modal.take()?;
        let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
        let has_key = self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch;
        let (mut submit, mut cancel) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("add-game")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(470.0);
            ui.label(egui::RichText::new(tr("Add game")).size(18.0).strong().color(pal().text));
            ui.add_space(12.0);
            modal_row(ui, "Name", |ui| {
                text_field(ui, &mut m.name, "", f32::INFINITY);
            });
            modal_row(ui, "Executable", |ui| {
                let shown = Path::new(&m.exe)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| m.exe.clone());
                let w = (ui.available_width() - button_width(ui, "Change…")).max(60.0);
                ui.scope(|ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(egui::RichText::new(disp(&shown)).color(pal().subtext)).truncate())
                        .on_hover_text(&m.exe);
                });
                if ui.button(tr("Change…")).clicked()
                    && let Some(p) = pick_executable()
                {
                    m = self.new_add_modal(p);
                }
            });
            modal_row(ui, "Type", |ui| {
                for (k, label) in [(GameKind::Windows, "Windows (Proton)"), (GameKind::Linux, "Linux native")] {
                    if choice_chip(ui, m.kind == k, label).clicked() {
                        m.kind = k;
                    }
                }
            });
            if m.kind == GameKind::Windows {
                modal_row(ui, "Proton", |ui| {
                    if tool_labels.is_empty() {
                        pill(ui, "no Proton builds found — see Dependencies", pal().warn);
                    } else {
                        let idx = m.proton_idx.min(tool_labels.len() - 1);
                        egui::ComboBox::from_id_salt("add-proton")
                            .selected_text(tool_labels[idx].as_str())
                            .width(260.0)
                            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                for (n, t) in tool_labels.iter().enumerate() {
                                    ui.selectable_value(&mut m.proton_idx, n, t);
                                }
                            }));
                    }
                });
            }
            para(
                ui,
                if has_key {
                    "Its cover and banner are fetched from SteamGridDB automatically."
                } else {
                    "Tip: with a SteamGridDB key (Settings) its artwork is fetched automatically."
                },
                pal().subtext,
                true,
            );
            ui.add_space(14.0);
            let can_add = !m.name.trim().is_empty()
                && (m.kind == GameKind::Linux || !tool_labels.is_empty());
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can_add, |ui| {
                    if primary_button(ui, "Add game").clicked() {
                        submit = true;
                    }
                });
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        }));
        if resp.should_close() {
            cancel = true;
        }
        if submit {
            return Some(m);
        }
        if !cancel {
            self.add_modal = Some(m);
        }
        None
    }

    fn remove_modal_ui(&mut self, ctx: &egui::Context, acts: &mut Acts) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let Some(i) = self.index_of(&id) else {
            self.confirm_remove = None;
            return;
        };
        let g = &self.cfg.custom_games[i];
        let name = g.name.clone();
        let keeps = match g.kind {
            GameKind::Windows => lfmt(
                "It leaves your library. Its prefix and saves stay on disk:\n{}",
                &[&game_prefix(g, &self.cfg.settings).display()],
            ),
            GameKind::Linux => lg("It leaves your library. The game’s files stay on disk.").to_string(),
        };
        let (mut yes, mut no) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("confirm-remove")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(440.0);
            ui.label(
                egui::RichText::new(trf("Remove “{}”?", &[&name]))
                    .size(18.0)
                    .strong()
                    .color(pal().text),
            );
            ui.add_space(8.0);
            para(ui, &keeps, pal().subtext, false);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(tr("Remove")).color(pal().on_accent).strong())
                            .fill(pal().danger)
                            .stroke(egui::Stroke::new(1.0, pal().danger))
                            .corner_radius(rad(9)),
                    )
                    .clicked()
                {
                    yes = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    no = true;
                }
            });
        }));
        if yes {
            acts.game.push((id, GameAct::Remove));
            self.confirm_remove = None;
        } else if no || resp.should_close() {
            self.confirm_remove = None;
        }
    }

    fn page_artwork(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let Some(gi) = self.art.game.filter(|&i| i < self.cfg.custom_games.len()) else {
            acts.back = true;
            return;
        };
        let name = self.cfg.custom_games[gi].name.clone();
        acts.back |= back_button(ui, ctx, &name);
        ui.add_space(10.0);
        page_header(
            ui,
            "Artwork",
            &lfmt("Icon, cover and banner for {} — from SteamGridDB or your own files", &[&name]),
        );

        // Current artwork, each with local-file and reset options.
        let icon = self.game_icon(ctx, gi);
        let cover = self.game_poster(ctx, gi);
        let hero = self.game_hero(ctx, gi);
        let (has_icon, has_cover, has_hero) = {
            let g = &self.cfg.custom_games[gi];
            (!g.icon.is_empty(), !g.cover.is_empty(), !g.hero.is_empty())
        };
        let initial = name.chars().next().unwrap_or('?');
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let note = if has_icon { "custom image" } else { "from the game’s exe" };
                art_current(ui, ArtKind::Icon, note, has_icon, acts, |ui| {
                    icon_slot(ui, icon.as_ref(), pal().accent, initial);
                });
                ui.add_space(18.0);
                let note = if has_cover { "shown in the library" } else { "not set" };
                art_current(ui, ArtKind::Cover, note, has_cover, acts, |ui| match &cover {
                    Some(c) => {
                        cover_slot(ui, c, egui::vec2(44.0, 66.0));
                    }
                    None => cover_placeholder(ui, egui::vec2(44.0, 66.0)),
                });
                ui.add_space(18.0);
                let note = if has_hero { "game page banner" } else { "uses the blurred cover" };
                art_current(ui, ArtKind::Hero, note, has_hero, acts, |ui| match &hero {
                    Some(h) => {
                        cover_slot(ui, h, egui::vec2(132.0, 44.0));
                    }
                    None => cover_placeholder(ui, egui::vec2(132.0, 44.0)),
                });
            });
        });
        ui.add_space(14.0);

        if self.sgdb_key().is_none() {
            card_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr("SteamGridDB API key needed"))
                        .strong()
                        .color(pal().text),
                );
                para(ui, "Browsing artwork needs a free API key. Paste it here (it’s also in Settings).", pal().subtext, true);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.art.key_input)
                        .password(true)
                        .hint_text(tr("API key"))
                        .desired_width(300.0),
                    );
                    if primary_button(ui, "Save key").clicked() {
                        acts.art.push(ArtCmd::SaveKey);
                    }
                    ui.hyperlink_to(tr("Get a key"),
                        "https://www.steamgriddb.com/profile/preferences/api",
                    );
                });
            });
            return;
        }

        // Search + matching SteamGridDB entries.
        ui.horizontal(|ui| {
            let w = (ui.available_width() - button_width(ui, "Search") - 8.0).max(120.0);
            let (resp, _) = text_field(ui, &mut self.art.query, "Search SteamGridDB…", w);
            let enter = resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, "Search").clicked() || enter {
                acts.art.push(ArtCmd::Search);
            }
        });
        if !self.art.matches.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(tr("Match")).color(pal().subtext).small());
                for m in &self.art.matches {
                    let label = match m.year {
                        Some(y) => disp(&format!("{} ({y})", m.name)),
                        None => disp(&m.name),
                    };
                    let on = self.art.sgdb_id == Some(m.id);
                    let tip = if m.verified {
                        "Verified SteamGridDB entry"
                    } else {
                        "Unverified SteamGridDB entry"
                    };
                    if choice_chip(ui, on, &label).on_hover_text(tr(tip)).clicked() {
                        acts.art.push(ArtCmd::SelectMatch(m.id));
                    }
                }
            });
        }
        ui.add_space(10.0);

        // Icons / Covers tabs + apply.
        let kind = self.art.kind;
        ui.horizontal(|ui| {
            for k in [ArtKind::Icon, ArtKind::Cover, ArtKind::Hero] {
                if choice_chip(ui, kind == k, k.label()).clicked() {
                    acts.art.push(ArtCmd::Kind(k));
                }
            }
            if self.art.loading {
                ui.spinner();
            }
            ui.with_layout(
                far_end(),
                |ui| {
                    let can = self.art.picked.is_some() && !self.art.applying;
                    ui.add_enabled_ui(can, |ui| {
                        let label = lfmt("Apply as {}", &[&lg(kind.noun())]);
                        if primary_button(ui, &label).clicked() {
                            acts.art.push(ArtCmd::Apply);
                        }
                    });
                    if self.art.applying {
                        ui.spinner();
                    }
                },
            );
        });
        if let Some(e) = &self.art.error {
            ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
        }
        ui.add_space(10.0);

        // Thumbnail grid.
        let tile = match kind {
            ArtKind::Icon => egui::vec2(104.0, 104.0),
            ArtKind::Cover => egui::vec2(112.0, 168.0),
            ArtKind::Hero => egui::vec2(240.0, 78.0),
        };
        // Thumbnails fade (and settle) in as they arrive instead of popping.
        let (now, fade_secs) = (self.now, self.secs(0.3));
        let fades: Vec<f32> = self
            .art
            .assets
            .iter()
            .map(|a| self.motion.thumb_loaded.get(&a.thumb).map_or(1.0, |&t| ease_since(now, t, fade_secs)))
            .collect();
        self.motion.busy |= fades.iter().any(|&f| f < 1.0);
        let pick_secs = self.secs(0.15);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            for (n, a) in self.art.assets.iter().enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(tile, egui::Sense::click());
                let radius = rad(10);
                ui.painter().rect_filled(rect, radius, pal().card2);
                match self.art.thumbs.get(&a.thumb) {
                    Some(t) => {
                        let inner = match kind {
                            ArtKind::Icon => rect.shrink(12.0),
                            ArtKind::Cover | ArtKind::Hero => rect,
                        };
                        let sz = t.size_vec2();
                        let f = fades[n];
                        let s = (inner.width() / sz.x).min(inner.height() / sz.y) * (0.96 + 0.04 * f);
                        egui::Image::new(egui::load::SizedTexture::from_handle(t))
                            .corner_radius(radius)
                            .tint(egui::Color32::WHITE.gamma_multiply(f))
                            .paint_at(
                                ui,
                                egui::Rect::from_center_size(inner.center(), sz * s),
                            );
                    }
                    None if self.art.thumb_failed.contains(&a.thumb) => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            tr("no preview"),
                            egui::FontId::proportional(11.0),
                            pal().faint,
                        );
                    }
                    None => {
                        ui.put(
                            egui::Rect::from_center_size(
                                rect.center(),
                                egui::vec2(18.0, 18.0),
                            ),
                            egui::Spinner::new(),
                        );
                    }
                }
                let picked = ctx.animate_bool_with_time(
                    egui::Id::unique(("thumb-pick", n)),
                    self.art.picked == Some(n),
                    pick_secs,
                );
                let rest = if resp.hovered() { pal().subtext } else { pal().border };
                let stroke = egui::Stroke::new(1.0 + 1.5 * picked, rest.lerp_to_gamma(pal().accent, picked));
                ui.painter().rect_stroke(
                    rect,
                    radius,
                    stroke,
                    egui::StrokeKind::Inside,
                );
                let dims = if a.width == 0 {
                    "ICO".to_string()
                } else {
                    format!("{}×{}", a.width, a.height)
                };
                let by = if a.author.is_empty() {
                    String::new()
                } else {
                    lfmt(" · by {}", &[&a.author])
                };
                let resp = resp
                    .on_hover_text(disp(&format!("{dims} · {}{by}", a.style)))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    acts.art.push(ArtCmd::Pick(n));
                }
                if resp.double_clicked() {
                    acts.art.push(ArtCmd::Pick(n));
                    acts.art.push(ArtCmd::Apply);
                }
                resp.context_menu(|ui| menu_ui(ui, |ui| {
                    if ui.button(trf("Apply as {}", &[&lg(kind.noun())])).clicked() {
                        acts.art.push(ArtCmd::Pick(n));
                        acts.art.push(ArtCmd::Apply);
                        ui.close();
                    }
                    if ui.button(tr("Open full image")).clicked() {
                        acts.open = Some(a.url.clone());
                        ui.close();
                    }
                    if ui.button(tr("Copy image link")).clicked() {
                        acts.copy = Some(a.url.clone());
                        ui.close();
                    }
                }));
            }
        });
        if self.art.more && !self.art.loading {
            ui.add_space(12.0);
            if ui.button(tr("Load more")).clicked() {
                acts.art.push(ArtCmd::More);
            }
        }
        if !self.art.assets.is_empty() {
            ui.add_space(10.0);
            para(ui, "Click to select, then Apply — or double-click a tile to apply it straight away.", pal().subtext, true);
        }
    }

    fn page_settings(&mut self, ui: &mut egui::Ui, acts: &mut Acts) {
        page_header(ui, "Settings", "Changes are saved as you make them");
        ui.set_max_width(820.0);
        let mut save = false;
        let indent = 128.0;

        // ---- Appearance
        let zoom = self.cfg.settings.zoom();
        let dark_now = pal().dark;
        section_sub(ui, "Appearance", "Language, colours, shape, size and motion", |ui| {
            modal_row(ui, "Language", |ui| {
                for (key, label) in [("", tr("System")), ("en", "English".to_string()), ("ar", disp("العربية"))] {
                    if choice_chip(ui, self.cfg.settings.language == key, &label).clicked() {
                        self.cfg.settings.language = key.into();
                        save = true;
                    }
                }
            });
            modal_row(ui, "Mode", |ui| {
                for (key, label) in [("dark", tr("Dark")), ("light", tr("Light")), ("system", tr("Match system"))] {
                    let on = match self.cfg.settings.theme_mode.as_str() {
                        "" => key == "dark",
                        m => m == key,
                    };
                    if choice_chip(ui, on, &label).clicked() {
                        self.cfg.settings.theme_mode = key.into();
                        save = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(120.0, 22.0),
                    egui::Label::new(egui::RichText::new(tr("Theme")).color(pal().subtext)),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                    let current = theme::theme_by_key(
                        if dark_now { &self.cfg.settings.dark_theme } else { &self.cfg.settings.light_theme },
                        dark_now,
                    )
                    .key;
                    for t in theme::THEMES.iter().filter(|t| t.dark == dark_now) {
                        if theme_tile(ui, t, t.key == current).clicked() {
                            let slot = if dark_now {
                                &mut self.cfg.settings.dark_theme
                            } else {
                                &mut self.cfg.settings.light_theme
                            };
                            *slot = t.key.into();
                            save = true;
                        }
                    }
                });
            });
            note(
                ui,
                indent,
                if dark_now {
                    "Dark themes. Switch the mode to Light to pick a light one; each mode remembers its own."
                } else {
                    "Light themes. Switch the mode to Dark to pick a dark one; each mode remembers its own."
                },
            );
            ui.add_space(4.0);
            modal_row(ui, "Accent", |ui| {
                let theme_accent = theme::build(
                    if dark_now { &self.cfg.settings.dark_theme } else { &self.cfg.settings.light_theme },
                    dark_now,
                    "",
                    false,
                )
                .accent;
                if accent_swatch(ui, theme_accent, self.cfg.settings.accent.is_empty())
                    .on_hover_text(tr("The theme's own"))
                    .clicked()
                {
                    self.cfg.settings.accent.clear();
                    save = true;
                }
                ui.add_space(6.0);
                for (key, name, _, _) in theme::ACCENTS {
                    let c = theme::accent_color(key, dark_now).unwrap_or(theme_accent);
                    if accent_swatch(ui, c, self.cfg.settings.accent == *key)
                        .on_hover_text(tr(name))
                        .clicked()
                    {
                        self.cfg.settings.accent = (*key).into();
                        save = true;
                    }
                }
            });
            modal_row(ui, "Corners", |ui| {
                for (square, label) in [(false, tr("Rounded")), (true, tr("Square"))] {
                    if choice_chip(ui, self.cfg.settings.square_corners == square, &label).clicked() {
                        self.cfg.settings.square_corners = square;
                        save = true;
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "Library tiles", |ui| {
                for (key, label, _) in TILE_SIZES {
                    if choice_chip(ui, self.cfg.settings.tile_size_key() == key, label).clicked() {
                        self.cfg.settings.tile_size = key.into();
                        save = true;
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "UI scale", |ui| {
                // Steps, not a slider: dragging a slider that rescales the UI under
                // the pointer makes the value jump around.
                let smaller = ZOOM_STEPS.iter().rev().find(|&&z| z < zoom - 0.005).copied();
                let larger = ZOOM_STEPS.iter().find(|&&z| z > zoom + 0.005).copied();
                let step = |ui: &mut egui::Ui, label: &str, to: Option<f32>, tip: &str| {
                    ui.add_enabled(to.is_some(), egui::Button::new(label).min_size(egui::vec2(32.0, 28.0)))
                        .on_hover_text(tip)
                        .clicked()
                        .then_some(to)
                        .flatten()
                };
                let mut pick = step(ui, "−", smaller, &tr("Smaller (Ctrl −)"));
                ui.add_sized(
                    [64.0, 28.0],
                    egui::Label::new(
                        egui::RichText::new(format!("{:.0}%", zoom * 100.0))
                            .size(16.0)
                            .strong()
                            .color(pal().text),
                    ),
                );
                pick = pick.or(step(ui, "+", larger, &tr("Larger (Ctrl +)")));
                if (zoom - 1.0).abs() > 0.005 {
                    ui.add_space(6.0);
                    if ui.button(tr("Reset")).on_hover_text(tr("Back to 100% (Ctrl 0)")).clicked() {
                        pick = Some(1.0);
                    }
                }
                if let Some(z) = pick {
                    self.cfg.settings.zoom = z;
                    save = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.spacing_mut().item_spacing.x = 6.0;
                for z in ZOOM_STEPS {
                    let label = format!("{:.0}%", z * 100.0);
                    if choice_chip(ui, (z - zoom).abs() < 0.005, &label).clicked() {
                        self.cfg.settings.zoom = z;
                        save = true;
                    }
                }
            });
            note(ui, indent, "Applies on click. Ctrl + / Ctrl − / Ctrl 0 and Ctrl + scroll work anywhere and are remembered too.");
            ui.add_space(8.0);
            modal_row(ui, "VR", |ui| {
                if check(ui, &mut self.cfg.settings.allow_openxr, tr("Let games use VR headsets (OpenXR)")).changed() {
                    save = true;
                }
            });
            note(ui, indent, "Leave off unless you play VR games: on some systems Proton's VR support crashes games as they start.");
            ui.add_space(8.0);
            modal_row(ui, "Controller", |ui| {
                let mut on = !self.cfg.settings.no_gamepad;
                if check(ui, &mut on, tr("Navigate with a game controller")).changed() {
                    self.cfg.settings.no_gamepad = !on;
                    save = true;
                }
            });
            let names = self.pad.as_ref().map(|p| p.connected.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default();
            let line = match names.first() {
                _ if self.cfg.settings.no_gamepad => lg("Off.").to_string(),
                Some(n) => lfmt("Connected: {}. D-pad or left stick moves, A opens or plays, B goes back, LB/RB switch tabs, right stick scrolls.", &[n]),
                None => lg("No controller connected. Plug one in any time.").to_string(),
            };
            note(ui, indent, &line);
            ui.add_space(8.0);
            modal_row(ui, "Motion", |ui| {
                save |= check(ui, &mut self.cfg.settings.reduce_motion, tr("Reduce motion"))
                    .changed();
            });
            note(ui, indent, "Turns off transitions, glides and pulses.");
        });
        ui.add_space(12.0);

        // ---- Artwork
        let key_set = self.sgdb_key().is_some();
        let missing_art = self.needs_art().len();
        section_sub(ui, "Artwork", "Covers, banners and icons from SteamGridDB", |ui| {
            let (mut toggle_show, mut key_edited) = (false, false);
            let show_key = self.show_key;
            let can_test = key_set && !self.key_checking;
            row_with_buttons(
                ui,
                "API key",
                |ui| {
                    if ui.add_enabled(can_test, egui::Button::new(tr("Test"))).clicked() {
                        acts.test_key = true;
                    }
                    toggle_show = ui.button(tr(if show_key { "Hide" } else { "Show" })).clicked();
                },
                |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.cfg.settings.steamgriddb_key)
                            .password(!show_key)
                            .hint_text(tr("paste your key"))
                            .desired_width(ui.available_width()),
                    );
                    key_edited = r.changed();
                    save |= r.lost_focus();
                },
            );
            if toggle_show {
                self.show_key = !self.show_key;
            }
            if key_edited {
                self.key_check = None;
            }
            ui.horizontal(|ui| {
                ui.add_space(indent);
                match (&self.key_check, self.key_checking) {
                    (_, true) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Checking the key…")).color(pal().subtext).small());
                    }
                    (Some(Ok(())), _) => {
                        status_dot(ui, pal().ok, true);
                        ui.label(egui::RichText::new(tr("The key works")).color(pal().ok).small());
                    }
                    (Some(Err(e)), _) => {
                        status_dot(ui, pal().warn, true);
                        ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
                    }
                    (None, _) => {
                        ui.label(egui::RichText::new(tr("Free with a SteamGridDB account:")).color(pal().subtext).small());
                        ui.hyperlink_to(
                            egui::RichText::new(tr("get a key")).small(),
                            "https://www.steamgriddb.com/profile/preferences/api",
                        );
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "New games", |ui| {
                let mut on = !self.cfg.settings.skip_auto_fetch;
                if ui.add_enabled_ui(key_set, |ui| check(ui, &mut on, tr("Fetch a cover and banner when a game is added"))).inner
                    .changed()
                {
                    self.cfg.settings.skip_auto_fetch = !on;
                    save = true;
                }
            });
            modal_row(ui, "Game info", |ui| {
                let mut on = !self.cfg.settings.no_hltb;
                if check(ui, &mut on, tr("Show HowLongToBeat times on game pages")).changed() {
                    self.cfg.settings.no_hltb = !on;
                    save = true;
                }
            });
            modal_row(ui, "", |ui| {
                let mut on = !self.cfg.settings.no_store;
                if check(ui, &mut on, tr("Show Steam store info: description, screenshots, ProtonDB rating")).changed() {
                    self.cfg.settings.no_store = !on;
                    save = true;
                }
            });
            if key_set && missing_art > 0 {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let label = if missing_art == 1 {
                        tr("Fill missing artwork (1 game)")
                    } else {
                        trf("Fill missing artwork ({} games)", &[&missing_art])
                    };
                    if ui.add_enabled(self.auto_pending.is_empty(), egui::Button::new(label)).clicked() {
                        acts.fill_art = true;
                    }
                });
            }
        });
        ui.add_space(12.0);

        // ---- Saves
        let db = self.saves.index.as_ref().map(|ix| ix.len());
        let db_age = saves::index_age();
        let db_busy = self.tasks.iter().any(|t| t.key == "save-db");
        let mut update_db = false;
        section_sub(ui, "Saves", "Backups after every session, and a folder to keep saves in sync", |ui| {
            let (mut browse, mut open) = (false, false);
            let has_sync = !self.cfg.settings.save_sync_dir.is_empty();
            row_with_buttons(
                ui,
                "Sync folder",
                |ui| {
                    open = ui.add_enabled(has_sync, egui::Button::new(tr("Open"))).clicked();
                    browse = ui.button(tr("Browse…")).clicked();
                },
                |ui| {
                    let w = ui.available_width();
                    let (r, by_menu) = text_field(ui, &mut self.cfg.settings.save_sync_dir, &tr("off"), w);
                    save |= r.lost_focus() || by_menu;
                },
            );
            if browse && let Some(p) = pick_folder(lg("Folder to keep game saves in sync")) {
                self.cfg.settings.save_sync_dir = p.to_string_lossy().into();
                save = true;
            }
            if open {
                acts.open = Some(self.cfg.settings.save_sync_dir.clone());
            }
            note(
                ui,
                indent,
                "Use a Syncthing, Nextcloud or Dropbox folder, a USB drive, or a partition Windows can see. Saves are pulled in before a game starts and copied out after it closes.",
            );
            ui.add_space(6.0);
            modal_row(ui, "Backups", |ui| {
                let mut on = self.cfg.settings.save_auto_backup();
                if check(ui, &mut on, tr("Back up saves after every session")).changed() {
                    self.cfg.settings.save_no_auto_backup = !on;
                    save = true;
                }
            });
            modal_row(ui, "Keep", |ui| {
                let keep = self.cfg.settings.save_keep();
                for n in [5usize, 10, 20, 50] {
                    if choice_chip(ui, keep == n, &lfmt("{} per game", &[&n])).clicked() {
                        self.cfg.settings.save_keep = n as u32;
                        save = true;
                    }
                }
            });
            row_with_buttons(
                ui,
                "Save database",
                |ui| {
                    update_db = ui.add_enabled(!db_busy, egui::Button::new(tr("Update"))).clicked();
                },
                |ui| {
                    let text = match (db, db_age) {
                        _ if db_busy => tr("Downloading…"),
                        (Some(n), Some(t)) => trf("{} games · updated {}", &[&n, &format_ago(t, unix_now())]),
                        (None, Some(t)) => trf("downloaded {}", &[&format_ago(t, unix_now())]),
                        _ => tr("not downloaded yet — it's fetched the first time you open a Save Vault"),
                    };
                    ui.label(egui::RichText::new(text).color(pal().subtext).small());
                },
            );
            note(ui, indent, "From the Ludusavi project: where thousands of games keep their saves.");
        });
        if update_db {
            self.update_save_db();
        }
        ui.add_space(12.0);

        // ---- Tools
        section_sub(ui, "Tools", "Leave a field empty to find the tool on your PATH", |ui| {
            let s = &mut self.cfg.settings;
            for (label, field, hint, dep) in [
                ("umu-run", &mut s.umu_bin, "umu-run", "umu-run"),
                ("MangoHud", &mut s.mangohud_bin, "mangohud", "MangoHud"),
                ("GameMode", &mut s.gamemoderun_bin, "gamemoderun", "GameMode"),
            ] {
                let mut browse = false;
                row_with_buttons(
                    ui,
                    label,
                    |ui| browse = ui.button(tr("Browse…")).clicked(),
                    |ui| {
                        let (r, by_menu) = text_field(ui, field, hint, ui.available_width());
                        save |= r.lost_focus() || by_menu;
                    },
                );
                if browse && let Some(p) = pick_any_file(&lfmt("Select {}", &[&label])) {
                    *field = p.to_string_lossy().into();
                    save = true;
                }
                // What will actually be used, so a typo shows up straight away.
                let row = self.deps.iter().find(|r| r.name == dep);
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    match row {
                        Some(r) if r.found => {
                            status_dot(ui, pal().ok, true);
                            ui.label(egui::RichText::new(&r.detail).color(pal().subtext).small().monospace());
                        }
                        Some(r) => {
                            status_dot(ui, pal().warn, true);
                            ui.label(egui::RichText::new(tr(&r.detail)).color(pal().warn).small());
                        }
                        None => {}
                    }
                });
                ui.add_space(4.0);
            }
        });
        ui.add_space(12.0);

        // ---- New games
        let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
        let current_proton = self
            .proton_tools
            .iter()
            .position(|t| t.dir.to_string_lossy() == self.cfg.settings.default_proton);
        section_sub(ui, "New games", "What a game starts with when you add it", |ui| {
            modal_row(ui, "Proton", |ui| {
                let shown = current_proton.map_or(tr("Same as the last game added"), |n| tool_labels[n].clone());
                egui::ComboBox::from_id_salt("default-proton")
                    .selected_text(shown)
                    .width(280.0)
                    .show_ui(ui, |ui| menu_ui(ui, |ui| {
                        if ui
                            .selectable_label(current_proton.is_none(), tr("Same as the last game added"))
                            .clicked()
                        {
                            self.cfg.settings.default_proton.clear();
                            save = true;
                        }
                        for (n, t) in tool_labels.iter().enumerate() {
                            if ui.selectable_label(current_proton == Some(n), t).clicked() {
                                self.cfg.settings.default_proton = self.proton_tools[n].dir.to_string_lossy().into();
                                save = true;
                            }
                        }
                    }));
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(120.0, 22.0),
                    egui::Label::new(egui::RichText::new(tr("Performance")).color(pal().subtext)),
                );
                vcol(ui, |ui| {
                    let mut f = self.cfg.settings.new_game_flags();
                    if flags_ui(ui, &mut f) {
                        self.cfg.settings.new_flags = Some(f);
                        save = true;
                    }
                });
            });
        });
        ui.add_space(12.0);

        // ---- Keyboard shortcuts
        section_sub(ui, "Keyboard shortcuts", "Work anywhere in Gamelo; Meta+G works from anywhere on your desktop", |ui| {
            for (keys, what) in [
                ("Ctrl+K", "Find a game and play it"),
                ("Ctrl+F", "Search the library"),
                ("F11", "Couch mode (a controller's Guide button too)"),
                ("Meta+G", "Show Gamelo — change it in your desktop's shortcut settings"),
            ] {
                modal_row(ui, keys, |ui| {
                    ui.label(egui::RichText::new(tr(what)).color(pal().subtext));
                });
            }
        });
        ui.add_space(12.0);

        // ---- Storage
        if self.cache_bytes.is_none() {
            self.cache_bytes = Some(dir_size(&thumb_cache_dir()) + dir_size(&icons_dir()));
        }
        let cache = self.cache_bytes.unwrap_or(0);
        section_sub(ui, "Storage", "Where Gamelo keeps things", |ui| {
            let auto = data_dir().join("prefixes").to_string_lossy().to_string();
            let (mut browse, mut open) = (false, false);
            row_with_buttons(
                ui,
                "Prefixes",
                |ui| {
                    open = ui.button(tr("Open")).clicked();
                    browse = ui.button(tr("Browse…")).clicked();
                },
                |ui| {
                    let w = ui.available_width();
                    let (r, by_menu) = text_field(ui, &mut self.cfg.settings.prefix_base, &auto, w);
                    save |= r.lost_focus() || by_menu;
                },
            );
            if browse && let Some(p) = pick_folder(lg("Folder for new game prefixes")) {
                self.cfg.settings.prefix_base = p.to_string_lossy().into();
                save = true;
            }
            if open {
                acts.open = Some(self.cfg.settings.prefix_base().to_string_lossy().into());
            }
            note(ui, indent, "Only affects games without their own prefix; existing prefixes aren’t moved.");
            ui.add_space(6.0);
            for (label, dir) in [
                ("Settings", config_path().parent().map(Path::to_path_buf).unwrap_or_default()),
                ("Artwork", art_dir()),
                ("Logs", data_dir().join("logs")),
            ] {
                row_with_buttons(
                    ui,
                    label,
                    |ui| {
                        if ui.button(tr("Open")).clicked() {
                            acts.open = Some(dir.to_string_lossy().into());
                        }
                    },
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(dir.to_string_lossy()).color(pal().subtext).small().monospace(),
                            )
                            .truncate(),
                        );
                    },
                );
            }
            row_with_buttons(
                ui,
                "Image cache",
                |ui| {
                    if ui.add_enabled(cache > 0, egui::Button::new(tr("Clear"))).clicked() {
                        acts.clear_cache = true;
                    }
                },
                |ui| {
                    ui.label(egui::RichText::new(format_bytes(cache)).color(pal().text));
                    ui.label(
                        egui::RichText::new(tr("thumbnails and resized artwork, rebuilt when needed"))
                            .color(pal().subtext)
                            .small(),
                    );
                },
            );
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            brand_mark(ui, rect);
            ui.label(
                egui::RichText::new(format!("Gamelo {}", env!("CARGO_PKG_VERSION")))
                    .color(pal().faint)
                    .small(),
            );
            ui.with_layout(far_end(), |ui| {
                if ui.button(tr("Run first-time setup again")).clicked() {
                    self.setup = Some(setup::Setup::default());
                }
            });
        });
        if save {
            acts.save_settings = true;
        }
    }

    fn page_dependencies(&mut self, ui: &mut egui::Ui, acts: &mut Acts) {
        page_header(ui, "Dependencies", "Install missing tools and detect what’s available");
        let busy = self.download.is_some();
        let rows = &self.deps;
        let distro = distro_name();
        ui.label(
            egui::RichText::new(trf("Detected: {}", &[&distro]))
                .color(pal().subtext)
                .small(),
        );
        ui.add_space(10.0);
        for row in rows {
            card_frame().show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        let col =
                            if row.found { pal().ok } else { pal().warn };
                        status_dot(ui, col, row.found);
                        ui.label(
                            egui::RichText::new(tr(row.name))
                                .strong()
                                .color(pal().text),
                        );
                        ui.with_layout(
                            far_end(),
                            |ui| {
                                if let Some((action, label)) = row.action {
                                    ui.add_enabled_ui(!busy, |ui| {
                                        if primary_button(ui, label)
                                            .clicked()
                                        {
                                            acts.dep = Some(action);
                                        }
                                    });
                                }
                            },
                        );
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(tr(&row.detail))
                                .color(pal().subtext)
                                .small()
                                .monospace(),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .context_menu(|ui| menu_ui(ui, |ui| {
                        if ui.button(tr("Copy")).clicked() {
                            acts.copy = Some(row.detail.clone());
                            ui.close();
                        }
                    }));
                    if let Some(cmd) = &row.install_cmd {
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(cmd)
                                    .color(pal().info())
                                    .small()
                                    .monospace(),
                            );
                            if ui.small_button(tr("Copy")).clicked() {
                                acts.copy = Some(cmd.clone());
                            }
                        });
                    }
                },
            );
            ui.add_space(8.0);
        }
        ui.add_space(2.0);
        para(ui, "Downloads install into your home directory and need no root.", pal().subtext, true);
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                let label = self
                    .download
                    .as_ref()
                    .map(|d| d.label.clone())
                    .unwrap_or_default();
                ui.label(egui::RichText::new(tr(&label)).color(pal().subtext).small());
            }
            if ui.button(tr("Re-scan")).clicked() {
                acts.rescan = true;
            }
        });
    }
}

/// Gamescope settings. Returns true if anything changed.
fn gamescope_ui(ui: &mut egui::Ui, o: &mut GamescopeOpts) -> bool {
    let before = o.clone();
    const SIZES: &[(&str, u32, u32)] = &[("Native", 0, 0), ("1280×720", 1280, 720), ("1920×1080", 1920, 1080), ("2560×1440", 2560, 1440), ("3840×2160", 3840, 2160)];
    let label = |w: u32, h: u32| {
        SIZES.iter().find(|s| s.1 == w && s.2 == h).map_or_else(|| format!("{w}×{h}"), |s| tr(s.0))
    };
    if find_in_path("gamescope").is_none() {
        ui.horizontal(|ui| {
            status_dot(ui, pal().warn, true);
            let cmd = install_command(&["gamescope"]).unwrap_or_default();
            ui.label(egui::RichText::new(trf("gamescope isn't installed — {}", &[&cmd])).color(pal().warn).small());
        });
    }
    modal_row(ui, "Output", |ui| {
        egui::ComboBox::from_id_salt("gs-out").selected_text(label(o.width, o.height)).show_ui(ui, |ui| menu_ui(ui, |ui| {
            for (n, w, h) in SIZES {
                if ui.selectable_label(o.width == *w && o.height == *h, tr(n)).clicked() {
                    (o.width, o.height) = (*w, *h);
                }
            }
        }));
        ui.label(egui::RichText::new(tr("game renders at")).color(pal().subtext).small());
        egui::ComboBox::from_id_salt("gs-game").selected_text(if o.game_width == 0 { tr("same") } else { label(o.game_width, o.game_height) }).show_ui(ui, |ui| menu_ui(ui, |ui| {
            for (n, w, h) in SIZES {
                let text = if *w == 0 { tr("same") } else { tr(n) };
                if ui.selectable_label(o.game_width == *w && o.game_height == *h, text).clicked() {
                    (o.game_width, o.game_height) = (*w, *h);
                }
            }
        }));
    });
    modal_row(ui, "Upscaling", |ui| {
        for (key, name) in [("", "None"), ("fsr", "FSR"), ("nis", "NIS"), ("pixel", "Pixel"), ("linear", "Linear")] {
            if choice_chip(ui, o.filter == key, name).clicked() {
                o.filter = key.into();
            }
        }
    });
    modal_row(ui, "Options", |ui| {
        flag_chip(ui, &mut o.fullscreen, "Fullscreen");
        flag_chip(ui, &mut o.hdr, "HDR");
        ui.label(egui::RichText::new(tr("FPS limit")).color(pal().subtext).small());
        let mut fps = if o.refresh == 0 { String::new() } else { o.refresh.to_string() };
        if ui.add(egui::TextEdit::singleline(&mut fps).hint_text("∞").desired_width(48.0)).changed() {
            o.refresh = fps.trim().parse().unwrap_or(0);
        }
    });
    modal_row(ui, "Extra options", |ui| {
        let (r, by_menu) = text_field(ui, &mut o.extra, "e.g. --adaptive-sync", f32::INFINITY);
        let _ = (r, by_menu);
    });
    *o != before
}

/// FPS limit and MangoHud overlay rows. Returns true if anything changed.
fn tuning_ui(ui: &mut egui::Ui, t: &mut Tuning, mangohud: bool, windows: bool) -> bool {
    let before = t.clone();
    const CAPS: [u32; 7] = [0, 30, 40, 60, 90, 120, 144];
    modal_row(ui, "FPS limit", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for cap in CAPS {
                let label = if cap == 0 { tr("Off") } else { cap.to_string() };
                if choice_chip(ui, t.fps_limit == cap, &label).clicked() {
                    t.fps_limit = cap;
                }
            }
            let custom = !CAPS.contains(&t.fps_limit);
            if choice_chip(ui, custom, "Other").clicked() && !custom {
                t.fps_limit = 75;
            }
            if custom {
                ui.add(egui::DragValue::new(&mut t.fps_limit).range(10..=500).suffix(" fps"));
            }
        });
    });
    let how = match (t.fps_limit, mangohud && !t.overlay.is_empty(), windows) {
        (0, _, _) => None,
        (_, true, _) => Some("Capped by MangoHud."),
        (_, false, true) => Some("Capped by DXVK/VKD3D (DirectX games). Pick an overlay style below to use MangoHud's limiter instead."),
        (_, false, false) => Some("Native Linux games need MangoHud on (with an overlay style) for the cap."),
    };
    if let Some(how) = how {
        note(ui, 128.0, how);
    }
    if mangohud {
        ui.add_space(4.0);
        modal_row(ui, "Overlay", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (key, label) in [("", "Your MangoHud settings"), ("minimal", "Minimal"), ("standard", "Standard"), ("full", "Full"), ("custom", "Custom")] {
                    if choice_chip(ui, t.overlay == key, label).clicked() {
                        t.overlay = key.into();
                    }
                }
            });
        });
        if !t.overlay.is_empty() {
            modal_row(ui, "Position", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for (key, label) in [("", "Default"), ("top-left", "Top left"), ("top-right", "Top right"), ("bottom-left", "Bottom left"), ("bottom-right", "Bottom right")] {
                        if choice_chip(ui, t.position == key, label).clicked() {
                            t.position = key.into();
                        }
                    }
                });
            });
        }
        if t.overlay == "custom" {
            modal_row(ui, "Options", |ui| {
                text_field(ui, &mut t.custom, "e.g. fps,gpu_temp,cpu_temp,vram", f32::INFINITY);
            });
        }
    }
    *t != before
}

/// The performance toggles. Returns true if any of them changed.
fn flags_ui(ui: &mut egui::Ui, f: &mut GameFlags) -> bool {
    let before = *f;
    ui.horizontal_wrapped(|ui| {
        // Chips move to the next row rather than squeezing their text.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        flag_chip(ui, &mut f.mangohud, "MangoHud");
        flag_chip(ui, &mut f.ntsync, "NTSync");
        flag_chip(ui, &mut f.wayland, "Wayland");
        flag_chip(ui, &mut f.gamemode, "GameMode");
        flag_chip(ui, &mut f.gamescope, "Gamescope");
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("fsr4")
            .selected_text(tr(f.fsr4.label()))
            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Off, tr("Off"));
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Upgrade, tr("Upgrade"));
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Rrna3, tr("RDNA3 upgrade"));
            }));
        ui.add_enabled_ui(f.fsr4 != Fsr4Mode::Off, |ui| {
            flag_chip(ui, &mut f.watermark, "FSR4 HUD");
        });
    });
    *f != before
}

/// The arrow of a collapsible section: pointing along the reading direction
/// when closed, down when open.
fn disclosure_icon(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let c = response.rect.center();
    let r = 4.5;
    let dir = if rtl() { -1.0 } else { 1.0 };
    let angle = openness * std::f32::consts::FRAC_PI_2 * dir;
    let rot = egui::emath::Rot2::from_angle(angle);
    let pts = [egui::vec2(-r * 0.6 * dir, -r), egui::vec2(-r * 0.6 * dir, r), egui::vec2(r * dir, 0.0)]
        .map(|v| c + rot * v)
        .to_vec();
    let color = ui.style().interact(response).fg_stroke.color;
    ui.painter().add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// A rounded card container. The accent is drawn implicitly via icons and
/// status pills so cards stay calm instead of every card shouting with a
/// coloured border.
fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 14))
        .corner_radius(rad(12))
        .fill(pal().card)
        .stroke(egui::Stroke::new(1.0, pal().border))
}

fn icon_slot(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    accent: egui::Color32,
    initial: char,
) -> egui::Response {
    let size = 44.0;
    let resp = match tex {
        Some(t) => ui.add(
            egui::Image::new(egui::load::SizedTexture::from_handle(t))
                .fit_to_exact_size(egui::vec2(size, size))
                .corner_radius(rad(10))
                .sense(egui::Sense::click()),
        ),
        None => {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, rad(10), pal().card2);
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial.to_ascii_uppercase().to_string(),
                egui::FontId::proportional(20.0),
                accent,
            );
            resp
        }
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn load_rgba(ctx: &egui::Context, name: String, img: &image::RgbaImage) -> egui::TextureHandle {
    let (w, h) = img.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

/// A rounded rect filled with `tex`, cropped (not stretched) to cover `rect`.
fn cover_fill_shape(tex: &egui::TextureHandle, rect: egui::Rect, radius: u8, opacity: f32) -> egui::Shape {
    cover_fill_shape_ex(tex, rect, radius, opacity, 1.0, 0.5)
}

/// `cover_fill_shape`, zoomed in by `zoom` and panned vertically within the
/// spare image (`pan` 0 = top, 1 = bottom) — the banner parallax uses this.
fn cover_fill_shape_ex(
    tex: &egui::TextureHandle,
    rect: egui::Rect,
    radius: u8,
    opacity: f32,
    zoom: f32,
    pan: f32,
) -> egui::Shape {
    cover_fill_shape_pan(tex, rect, radius, opacity, zoom, egui::vec2(0.5, pan))
}

/// `cover_fill_shape_ex`, panned both ways (0..1 each; 0.5 = centred).
fn cover_fill_shape_pan(
    tex: &egui::TextureHandle,
    rect: egui::Rect,
    radius: u8,
    opacity: f32,
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Shape {
    let [tw, th] = tex.size();
    let ta = tw as f32 / th.max(1) as f32;
    let ra = rect.width() / rect.height().max(1.0);
    let (w, h) = if ra > ta { (1.0, ta / ra) } else { (ra / ta, 1.0) };
    let (w, h) = (w / zoom, h / zoom);
    let x = (1.0 - w) * pan.x;
    let y = (1.0 - h) * pan.y;
    let uv = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    egui::epaint::RectShape::filled(
        rect,
        rad(radius),
        egui::Color32::WHITE.gamma_multiply(opacity),
    )
    .with_texture(tex.id(), uv)
    .into()
}

/// A soft diagonal band of light at `t` (0..1) across `rect`: a "loading" sweep.
fn sheen(painter: &egui::Painter, rect: egui::Rect, t: f32) {
    let w = rect.width() * 0.5;
    let x = rect.left() - w + t * (rect.width() + 2.0 * w);
    let skew = rect.height() * 0.3;
    // Nested bands give the light a soft falloff.
    for (half, alpha) in [(w * 0.5, 9u8), (w * 0.28, 12), (w * 0.1, 16)] {
        let pts = vec![
            egui::pos2(x - half + skew, rect.top()),
            egui::pos2(x + half + skew, rect.top()),
            egui::pos2(x + half - skew, rect.bottom()),
            egui::pos2(x - half - skew, rect.bottom()),
        ];
        painter.add(egui::Shape::convex_polygon(pts, egui::Color32::from_white_alpha(alpha), egui::Stroke::NONE));
    }
}

/// A partial ring rotated to `angle`, over a faint full ring: a quiet "working" mark.
fn spinner_arc(painter: &egui::Painter, c: egui::Pos2, r: f32, angle: f32, color: egui::Color32) {
    painter.circle_stroke(c, r, egui::Stroke::new(3.0, color.gamma_multiply(0.2)));
    let pts: Vec<egui::Pos2> = (0..=24)
        .map(|k| {
            let a = angle + k as f32 / 24.0 * 1.8;
            c + r * egui::vec2(a.cos(), a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, egui::Stroke::new(3.0, color)));
}

/// A clickable cover poster.
fn cover_slot(ui: &mut egui::Ui, tex: &egui::TextureHandle, size: egui::Vec2) -> egui::Response {
    ui.add(
        egui::Image::new(egui::load::SizedTexture::from_handle(tex))
            .fit_to_exact_size(size)
            .corner_radius(rad(8))
            .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// An empty dashed-looking slot where a cover would go.
fn cover_placeholder(ui: &mut egui::Ui, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, rad(8), pal().card2);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        tr("none"),
        egui::FontId::proportional(11.0),
        pal().faint,
    );
}

/// Centered, muted placeholder shown when a list is empty.
fn empty_hint(ui: &mut egui::Ui, text: &str) {
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 30))
        .corner_radius(rad(12))
        .fill(pal().card.gamma_multiply(0.45))
        .stroke(egui::Stroke::new(1.0, pal().border));
    frame.show(ui, |ui| {
        ui.vertical_centered(|ui| {
            para(ui, text, pal().subtext, false);
        });
    });
}

/// The composed launch command, shown like a code block.
fn command_preview(ui: &mut egui::Ui, cmd: &str) {
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 9))
        .corner_radius(rad(9))
        .fill(pal().field)
        .stroke(egui::Stroke::new(1.0, pal().border));
    frame.show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(cmd).monospace().small().color(pal().subtext))
                .wrap()
                .sense(egui::Sense::click()),
        )
        .context_menu(|ui| menu_ui(ui, |ui| {
            if ui.button(tr("Copy")).clicked() {
                ui.ctx().copy_text(cmd.to_string());
                ui.close();
            }
        }));
    });
}

/// A small rounded outline badge. Painted at its own size, so it never
/// stretches to the height of the row it sits in.
fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(tr(text), font, color);
    let size = galley.size() + egui::vec2(18.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, rad(255), color.gamma_multiply(0.12));
    p.rect_stroke(rect, rad(255), egui::Stroke::new(1.0, color.gamma_multiply(0.40)), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// A small painted status dot (filled when ok, hollow otherwise).
/// Drawn with the painter so it never depends on font glyph coverage.
fn status_dot(ui: &mut egui::Ui, color: egui::Color32, filled: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let c = rect.center();
    if filled {
        ui.painter().circle_filled(c, 5.0, color);
    } else {
        ui.painter()
            .circle_stroke(c, 5.0, egui::Stroke::new(1.6, color));
    }
}

/// A toggle chip used for launch flags.
fn flag_chip(ui: &mut egui::Ui, on: &mut bool, label: &str) {
    if choice_chip(ui, *on, label).clicked() {
        *on = !*on;
    }
}

/// A pill-shaped button drawn filled when `on`.
fn choice_chip(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    // Colours cross-fade when toggled.
    let id = ui.make_persistent_id(("chip", label));
    let t = ui.ctx().animate_bool_with_time(id, on, ui.style().animation_time * 1.5);
    let (fill, fg, stroke) = (
        pal().card2.lerp_to_gamma(pal().accent, t),
        pal().subtext.lerp_to_gamma(pal().on_accent, t),
        pal().border.lerp_to_gamma(pal().accent, t),
    );
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(fg).small().strong())
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(rad(255))
            .min_size(egui::vec2(0.0, 26.0)),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quieter card nested inside a section card.
fn card_inner() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 10))
        .corner_radius(rad(10))
        .fill(pal().card2.gamma_multiply(if pal().dark { 0.6 } else { 0.7 }))
        .stroke(egui::Stroke::new(1.0, pal().border.gamma_multiply(0.7)))
}

/// A filled red button for destructive confirmations.
fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(pal().on_accent).strong())
            .fill(pal().danger)
            .stroke(egui::Stroke::new(1.0, pal().danger))
            .corner_radius(rad(9)),
    )
}

/// A theme preview tile for Settings: a tiny window in the theme's colours.
fn theme_tile(ui: &mut egui::Ui, t: &theme::ThemeDef, selected: bool) -> egui::Response {
    let size = egui::vec2(100.0, 76.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let [bg, card, accent, text] = t.swatch();
    let p = ui.painter();
    let r = rad(8);
    let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 54.0));
    p.rect_filled(preview, r, bg);
    // A sidebar strip, a card with two text lines and an accent button.
    let side = egui::Rect::from_min_size(preview.min + egui::vec2(6.0, 6.0), egui::vec2(18.0, 42.0));
    p.rect_filled(side, rad(4), card);
    let cardr = egui::Rect::from_min_max(preview.min + egui::vec2(30.0, 6.0), preview.max - egui::vec2(6.0, 6.0));
    p.rect_filled(cardr, rad(4), card);
    p.rect_filled(
        egui::Rect::from_min_size(cardr.min + egui::vec2(6.0, 7.0), egui::vec2(40.0, 4.0)),
        rad(2),
        text.gamma_multiply(0.85),
    );
    p.rect_filled(
        egui::Rect::from_min_size(cardr.min + egui::vec2(6.0, 15.0), egui::vec2(28.0, 3.0)),
        rad(2),
        text.gamma_multiply(0.45),
    );
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(cardr.right() - 26.0, cardr.bottom() - 13.0), egui::vec2(20.0, 8.0)),
        rad(3),
        accent,
    );
    let hovered = resp.hovered();
    let stroke = if selected {
        egui::Stroke::new(2.0, pal().accent)
    } else if hovered {
        egui::Stroke::new(1.0, pal().subtext)
    } else {
        egui::Stroke::new(1.0, pal().border)
    };
    p.rect_stroke(preview, r, stroke, egui::StrokeKind::Outside);
    p.text(
        egui::pos2(rect.center().x, preview.bottom() + 6.0),
        egui::Align2::CENTER_TOP,
        tr(t.name),
        egui::FontId::proportional(12.5),
        if selected { pal().text } else { pal().subtext },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A round (or, with square corners, square) colour swatch.
fn accent_swatch(ui: &mut egui::Ui, color: egui::Color32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let inner = egui::Rect::from_center_size(c, egui::vec2(20.0, 20.0));
    if pal().square {
        p.rect_filled(inner, 0, color);
    } else {
        p.circle_filled(c, 10.0, color);
    }
    if selected || resp.hovered() {
        let stroke = egui::Stroke::new(2.0, if selected { pal().text } else { pal().subtext });
        if pal().square {
            p.rect_stroke(inner.expand(3.0), 0, stroke, egui::StrokeKind::Outside);
        } else {
            p.circle_stroke(c, 13.0, stroke);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent-filled primary action button.
fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(pal().on_accent).strong())
            .fill(pal().accent)
            .stroke(egui::Stroke::new(1.0, pal().accent))
            .corner_radius(rad(9)),
    )
}

/// A full-width sidebar navigation entry. Returns the click response.
fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    count: Option<usize>,
) -> egui::Response {
    let height = 36.0;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    // The selected highlight is drawn (and slid) by the sidebar itself.
    if hovered && !selected {
        p.rect_filled(rect, rad(9), pal().card);
    }
    let fg = if selected || hovered { pal().text } else { pal().subtext };
    let (start, start_align, end, end_align) = if rtl() {
        (rect.right() - 16.0, egui::Align2::RIGHT_CENTER, rect.left() + 12.0, egui::Align2::LEFT_CENTER)
    } else {
        (rect.left() + 16.0, egui::Align2::LEFT_CENTER, rect.right() - 12.0, egui::Align2::RIGHT_CENTER)
    };
    p.text(egui::pos2(start, rect.center().y), start_align, tr(label), egui::FontId::proportional(15.0), fg);
    if let Some(c) = count {
        p.text(
            egui::pos2(end, rect.center().y),
            end_align,
            c.to_string(),
            egui::FontId::proportional(12.0),
            if selected { pal().accent } else { pal().faint },
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A compact status line for the sidebar footer: dot + label + value.
fn status_line(ui: &mut egui::Ui, warn: bool, label: &str, value: &str) {
    ui.horizontal(|ui| {
        status_dot(ui, if warn { pal().warn } else { pal().ok }, true);
        ui.label(egui::RichText::new(tr(label)).color(pal().subtext).small());
        ui.with_layout(far_end(), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(tr(value)).color(pal().text).small()).truncate());
        });
    });
}

/// A labelled form row used inside the modal dialogs.
fn modal_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(120.0, 22.0),
            egui::Label::new(egui::RichText::new(tr(label)).color(pal().subtext))
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        add(ui);
    });
    ui.add_space(6.0);
}

/// Room a button labelled `label` takes in a row, spacing included (labels
/// differ in length between languages).
fn button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = ui.painter().layout_no_wrap(tr(label), font, egui::Color32::WHITE).size().x;
    text + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x + 2.0
}

/// A menu, dropdown or dialog is open: Escape, Enter and the arrow keys
/// belong to it, not to the page underneath.
fn overlay_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some())
}

/// "‹ label" button; also fires on Escape when no text field has focus.
fn back_button(ui: &mut egui::Ui, ctx: &egui::Context, label: &str) -> bool {
    let no_focus = ctx.memory(|m| m.focused().is_none()) && !overlay_open(ctx);
    let text = if rtl() { format!("{}  ›", tr(label)) } else { format!("‹  {}", tr(label)) };
    ui.button(text).clicked()
        || (no_focus && ctx.input(|i| i.key_pressed(egui::Key::Escape)))
}

/// A page title with a muted subtitle underneath.
fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(tr(title)).size(24.0).strong().color(pal().text));
    para(ui, subtitle, pal().subtext, true);
    ui.add_space(14.0);
}

/// A titled settings card on the game page.
fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tr(title)).strong().color(pal().text));
        ui.add_space(10.0);
        add(ui);
    });
}

/// A titled settings card with a one-line description.
fn section_sub(ui: &mut egui::Ui, title: &str, sub: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tr(title)).size(16.0).strong().color(pal().text));
        para(ui, sub, pal().subtext, true);
        ui.add_space(12.0);
        add(ui);
    });
}

/// A form row with buttons on the right: `buttons` is laid out right-to-left
/// first, then `field` fills exactly the width that's left.
fn row_with_buttons(
    ui: &mut egui::Ui,
    label: &str,
    buttons: impl FnOnce(&mut egui::Ui),
    field: impl FnOnce(&mut egui::Ui),
) {
    modal_row(ui, label, |ui| {
        ui.with_layout(far_end(), |ui| {
            buttons(ui);
            ui.with_layout(near_start(), field);
        });
    });
}

/// A small muted note lined up under a form row's field.
fn note(ui: &mut egui::Ui, indent: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        vcol(ui, |ui| para(ui, text, pal().subtext, true));
    });
}

/// Wrapped text that also reads right-to-left: Arabic is broken into lines
/// in reading order first, then each line is shaped (egui would otherwise
/// wrap the already-reversed text and the lines would read bottom-up).
fn para(ui: &mut egui::Ui, text: &str, color: egui::Color32, small: bool) -> egui::Response {
    let rich = |t: String| {
        let r = egui::RichText::new(t).color(color);
        if small { r.small() } else { r }
    };
    if let Some(logical) = i18n::for_wrap(text).filter(|l| shaping::needs_shaping(l)) {
        let style = if small { egui::TextStyle::Small } else { egui::TextStyle::Body };
        let font = style.resolve(ui.style());
        let width = ui.available_width().max(60.0);
        let wrapped = ui.ctx().fonts_mut(|f| {
            shaping::wrap_visual(&logical, width, rtl(), |s| f.layout_no_wrap(s.to_string(), font.clone(), color).size().x)
        });
        ui.add(egui::Label::new(rich(wrapped)).wrap_mode(egui::TextWrapMode::Extend))
    } else {
        ui.add(egui::Label::new(rich(tr(text))).wrap())
    }
}

/// A checkbox; in right-to-left layouts the box sits to the right of its text.
fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    if !rtl() {
        return ui.checkbox(on, text);
    }
    ui.horizontal(|ui| {
        let mut boxed = ui.add(egui::Checkbox::without_text(on));
        let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
        if label.clicked() {
            *on = !*on;
            boxed.mark_changed();
        }
        boxed.union(label)
    })
    .inner
}

/// `ui.vertical`, right-aligned when the interface is right-to-left.
fn vcol<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if rtl() {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), add)
    } else {
        ui.vertical(add)
    }
}

/// Layout for things pushed to the far end of a row (right, or left in RTL).
fn far_end() -> egui::Layout {
    if rtl() {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::right_to_left(egui::Align::Center)
    }
}

/// Layout that flows from the start of a row (left, or right in RTL).
fn near_start() -> egui::Layout {
    if rtl() {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    }
}

/// Run `add` in a top-down layout that runs right-to-left for Arabic, so the
/// rows inside mirror automatically.
fn dir_ui<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // A modal's first (sizing) pass measures left-aligned, as `menu_ui` explains.
    if rtl() && !ui.is_sizing_pass() {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), add).inner
    } else {
        add(ui)
    }
}

/// Menu and dropdown contents, right-aligned in a right-to-left interface
/// (egui lays popups out left to right whatever the language). Popups
/// measure their content in a first "sizing pass", where right-aligned items
/// would claim the whole screen width, so that pass stays left-aligned.
fn menu_ui<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if rtl() && !ui.is_sizing_pass() {
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Max), add).inner
    } else {
        add(ui)
    }
}

/// A labelled text field. Returns true when editing ends (Enter or click away).
fn edit_row(ui: &mut egui::Ui, label: &str, buf: &mut String, hint: &str) -> bool {
    let mut done = false;
    modal_row(ui, label, |ui| {
        let (r, by_menu) = text_field(ui, buf, hint, f32::INFINITY);
        done = r.lost_focus() || by_menu;
    });
    done
}

/// A single-line text field with a Cut / Copy / Paste / Select all / Clear menu
/// on right-click. Returns its response and whether the menu changed the text.
fn text_field(ui: &mut egui::Ui, buf: &mut String, hint: &str, width: f32) -> (egui::Response, bool) {
    // Arabic reads from the right: lay it out in reading order (see rtl_edit).
    let style = rtl_edit::FieldStyle {
        font: egui::TextStyle::Body.resolve(ui.style()),
        color: ui.visuals().text_color(),
        line_height: ui.fonts_mut(|f| f.row_height(&egui::TextStyle::Body.resolve(ui.style()))) + ui.spacing().extra_text_line_spacing,
    };
    let rtl_text = rtl_edit::rtl_base(buf);
    let has_rtl = shaping::needs_shaping(buf);
    let align = if rtl_text { egui::Align::Max } else { egui::Align::Min };
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| rtl_edit::layout(ui, text.as_str(), &style, align);
    let out = egui::TextEdit::singleline(buf)
        .hint_text(tr(hint))
        .desired_width(width)
        .font(style.font.clone())
        .text_color(style.color)
        .align(egui::Align2([align, egui::Align::Center]))
        .layouter(&mut layouter)
        .show(ui);
    let id = out.response.id;
    let ctx = ui.ctx().clone();
    if has_rtl {
        // egui's click-to-caret and selection highlight assume left-to-right text.
        if out.response.response.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed())
            && let Some(p) = out.response.response.interact_pointer_pos()
        {
            let at = rtl_edit::cursor_at(&out.galley, buf, p.x - out.galley_pos.x);
            let mut state = out.state.clone();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            state.store(&ctx, id);
        }
        if let Some(r) = out.cursor_range.filter(|r| !r.is_empty()) {
            let r = r.as_sorted_char_range();
            rtl_edit::paint_selection(ui, &out.galley, out.galley_pos, r.start.0..r.end.0, out.text_clip_rect);
        }
    }
    // The cursor survives losing focus, so a selection made before the
    // right-click is still known.
    let range = out
        .cursor_range
        .or_else(|| egui::TextEdit::load_state(&ctx, id).and_then(|s| s.cursor.char_range()));
    let resp = out.response.response;
    let mut changed = false;
    resp.context_menu(|ui| menu_ui(ui, |ui| {
        let byte = |text: &str, ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
        let sel = range.map(|r| {
            let r = r.as_sorted_char_range();
            (byte(buf, r.start.0), byte(buf, r.end.0))
        });
        let picked = sel.filter(|(a, b)| a < b);
        if ui.add_enabled(picked.is_some(), egui::Button::new(tr("Cut"))).clicked() {
            if let Some((a, b)) = picked {
                ctx.copy_text(buf[a..b].to_string());
                buf.replace_range(a..b, "");
                changed = true;
            }
            ui.close();
        }
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Copy"))).clicked() {
            let (a, b) = picked.unwrap_or((0, buf.len()));
            ctx.copy_text(buf[a..b].to_string());
            ui.close();
        }
        if ui.button(tr("Paste")).clicked() {
            if let Some(text) = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()) {
                let text = text.replace(['\n', '\r'], " ");
                let (a, b) = sel.unwrap_or((buf.len(), buf.len()));
                buf.replace_range(a..b, &text);
                changed = true;
            }
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Select all"))).clicked() {
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::select_all_str(buf)));
            state.store(&ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
            ui.close();
        }
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Clear"))).clicked() {
            buf.clear();
            changed = true;
            ui.close();
        }
    }));
    (resp, changed)
}

/// The right-click menu for a game, shared by its poster and its page.
fn game_menu(ui: &mut egui::Ui, g: &CustomGame, running: bool, on_page: bool, collections: &[String], acts: &mut Acts) {
    let (id, windows, fav) = (g.id.as_str(), g.kind == GameKind::Windows, g.favorite);
    let item = |ui: &mut egui::Ui, acts: &mut Acts, label: &str, act: GameAct| {
        if ui.button(tr(label)).clicked() {
            acts.game.push((id.to_string(), act));
            ui.close();
        }
    };
    if running {
        item(ui, acts, "Stop", GameAct::Stop);
    } else {
        item(ui, acts, "Play", GameAct::Play);
    }
    if !on_page {
        item(ui, acts, "Open game page", GameAct::Open);
    }
    item(ui, acts, "Change artwork", GameAct::Artwork);
    item(ui, acts, "Saves…", GameAct::Saves);
    item(ui, acts, if fav { "Remove from favorites" } else { "Add to favorites" }, GameAct::Favorite);
    ui.menu_button(tr("Status"), |ui| menu_ui(ui, |ui| {
        for st in std::iter::once(GameStatus::None).chain(GameStatus::ALL) {
            if ui.selectable_label(g.status == st, tr(st.label())).clicked() {
                acts.game.push((id.to_string(), GameAct::SetStatus(st)));
                ui.close();
            }
        }
    }));
    ui.menu_button(tr("Collections"), |ui| menu_ui(ui, |ui| {
        for c in collections {
            let on = g.collections.contains(c);
            let mut checked = on;
            if check(ui, &mut checked, disp(c)).changed() {
                acts.game.push((id.to_string(), GameAct::ToggleCollection(c.clone())));
            }
        }
        if !collections.is_empty() {
            ui.separator();
        }
        if ui.button(tr("New collection…")).clicked() {
            acts.collection = Some(CollectionEdit { game: Some(id.to_string()), ..Default::default() });
            ui.close();
        }
    }));
    ui.separator();
    item(ui, acts, "Open game folder", GameAct::OpenGameFolder);
    if windows {
        item(ui, acts, "Open prefix folder", GameAct::OpenPrefix);
    }
    item(ui, acts, "Open log", GameAct::OpenLog);
    item(ui, acts, "Copy launch command", GameAct::CopyCommand);
    item(ui, acts, "Add to app menu", GameAct::Shortcut);
    ui.separator();
    if ui
        .button(egui::RichText::new(tr("Remove from library…")).color(pal().danger))
        .clicked()
    {
        acts.game.push((id.to_string(), GameAct::AskRemove));
        ui.close();
    }
}

/// A small pill drawn straight onto a poster. Returns the height used.
/// `glow` (0..1) brightens the dot, so it can breathe for live states.
fn poster_badge(painter: &egui::Painter, pos: egui::Pos2, text: &str, color: egui::Color32, glow: f32) -> f32 {
    let galley = painter.layout_no_wrap(tr(text), egui::FontId::proportional(11.5), color);
    let size = galley.size() + egui::vec2(20.0, 6.0);
    let rect = egui::Rect::from_min_size(pos, size);
    painter.rect_filled(rect, rad(255), egui::Color32::from_black_alpha(190));
    painter.rect_stroke(
        rect,
        rad(255),
        egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let dot = rect.left_center() + egui::vec2(8.0, 0.0);
    painter.circle_filled(dot, 5.0 * glow, color.gamma_multiply(0.25 * glow));
    painter.circle_filled(dot, 3.0, color.gamma_multiply(0.55 + 0.45 * glow));
    painter.galley(rect.min + egui::vec2(14.0, 3.0), galley, color);
    size.y + 4.0
}

/// A soft round light: `color` at the centre fading to nothing at `radius`.
fn glow_spot(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    let n = 40;
    for k in 0..=n {
        // Offset half a step: no spoke lies exactly horizontal.
        let a = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + radius * egui::vec2(a.cos(), a.sin()), egui::Color32::TRANSPARENT);
    }
    for k in 1..=n as u32 {
        mesh.add_triangle(0, k, k + 1);
    }
    painter.add(mesh);
}

/// A right-pointing play triangle centred on `c`.
fn play_triangle(painter: &egui::Painter, c: egui::Pos2, size: f32, color: egui::Color32) {
    let pts = vec![
        c + egui::vec2(-size * 0.45, -size * 0.6),
        c + egui::vec2(-size * 0.45, size * 0.6),
        c + egui::vec2(size * 0.65, 0.0),
    ];
    painter.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

impl Settings {
    /// The install folder for a store's games.
    fn store_dir(&self, store: stores::Store) -> PathBuf {
        let (set, default) = match store {
            stores::Store::Epic => (&self.epic_dir, "Games/Epic"),
            stores::Store::Gog => (&self.gog_dir, "Games/GOG"),
        };
        if set.is_empty() {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(default)
        } else {
            PathBuf::from(set)
        }
    }

    /// The tile size key in effect ("medium" when unset).
    fn tile_size_key(&self) -> &str {
        if TILE_SIZES.iter().any(|(k, ..)| *k == self.tile_size) { &self.tile_size } else { "medium" }
    }
}

/// The library layout buttons: three grid sizes (many small squares … one big
/// one) and a list. Returns "small", "medium", "large" or "list" when clicked.
fn tile_size_toggle(ui: &mut egui::Ui, settings: &Settings) -> Option<&'static str> {
    let current = if settings.library_view == "list" { "list" } else { settings.tile_size_key() };
    let mut picked = None;
    ui.spacing_mut().item_spacing.x = 4.0;
    let options = [("small", "Small tiles"), ("medium", "Medium tiles"), ("large", "Large tiles"), ("list", "List")];
    // Laid out from the far end, so add them in reverse to read small → list.
    let order: Vec<_> = if rtl() { options.to_vec() } else { options.iter().rev().copied().collect() };
    for (key, tip) in order {
        let on = key == current;
        let resp = ui
            .add(egui::Button::new("").min_size(egui::vec2(30.0, 30.0)).selected(on))
            .on_hover_text(tr(tip))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = if on { pal().accent } else if resp.hovered() { pal().text } else { pal().subtext };
        let area = egui::Rect::from_center_size(resp.rect.center(), egui::vec2(14.0, 14.0));
        if key == "list" {
            for r in 0..3 {
                let y = area.top() + 1.0 + r as f32 * 5.0;
                ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(area.left(), y), egui::vec2(3.0, 3.0)), rad(1), color);
                ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(area.left() + 5.0, y), egui::vec2(9.0, 3.0)), rad(1), color);
            }
        } else {
            let n = match key {
                "small" => 3,
                "medium" => 2,
                _ => 1,
            };
            let gap = 2.0;
            let cell = (area.width() - gap * (n as f32 - 1.0)) / n as f32;
            for r in 0..n {
                for c in 0..n {
                    let min = area.min + egui::vec2(c as f32 * (cell + gap), r as f32 * (cell + gap));
                    ui.painter().rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell, cell)), rad(1), color);
                }
            }
        }
        if resp.clicked() {
            picked = Some(key);
        }
    }
    picked
}

/// Favourite gold, the same in every theme.
const GOLD: egui::Color32 = egui::Color32::from_rgb(245, 196, 81);

/// A five-pointed star centred on `c` (painted: egui only fills convex shapes,
/// so it's a pentagon plus five tips).
fn star(painter: &egui::Painter, c: egui::Pos2, r: f32, filled: bool, color: egui::Color32) {
    let at = |k: usize, rr: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
        c + rr * egui::vec2(a.cos(), a.sin())
    };
    let inner = r * 0.45;
    if filled {
        let pent: Vec<_> = (0..5).map(|k| at(2 * k + 1, inner)).collect();
        painter.add(egui::Shape::convex_polygon(pent, color, egui::Stroke::NONE));
        for k in 0..5 {
            let tip = vec![at((2 * k + 9) % 10, inner), at(2 * k, r), at(2 * k + 1, inner)];
            painter.add(egui::Shape::convex_polygon(tip, color, egui::Stroke::NONE));
        }
    } else {
        let mut pts: Vec<_> = (0..10).map(|k| at(k, if k % 2 == 0 { r } else { inner })).collect();
        pts.push(pts[0]);
        painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, color)));
    }
}

/// A square button with a star: filled gold when on.
fn star_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let h = ui.spacing().interact_size.y.max(30.0) + 4.0;
    let resp = ui.add(egui::Button::new("").min_size(egui::vec2(h, h)));
    let color = if on { GOLD } else if resp.hovered() { pal().text } else { pal().subtext };
    star(ui.painter(), resp.rect.center(), 8.0, on, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[derive(Clone, Copy, PartialEq)]
enum PlayState {
    Play,
    Starting,
    Stop,
    Stopping,
}

/// The big Play / Stop button on the game page (icons are painted, not glyphs).
/// While starting or stopping it can't be clicked and shows that it's working.
/// `dx` offsets the drawing (the failed-action shake) without moving the layout.
fn play_button(ui: &mut egui::Ui, state: PlayState, phase: f32, dx: f32) -> egui::Response {
    let (label, fill) = match state {
        PlayState::Play => ("Play", pal().accent),
        PlayState::Starting => ("Starting…", pal().accent),
        PlayState::Stop => ("Stop", pal().danger),
        PlayState::Stopping => ("Stopping…", pal().danger),
    };
    let busy = matches!(state, PlayState::Starting | PlayState::Stopping);
    let size = egui::vec2(if busy { 158.0 } else { 132.0 }, 44.0);
    let sense = if busy { egui::Sense::hover() } else { egui::Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let rect = rect.translate(egui::vec2(dx, 0.0));
    let fill = match (busy, resp.hovered()) {
        (true, _) => fill.gamma_multiply(0.6),
        (false, true) => fill,
        (false, false) => fill.gamma_multiply(0.92),
    };
    let p = ui.painter();
    p.rect_filled(rect, rad(10), fill);
    let icon_c = if rtl() {
        egui::pos2(rect.right() - 34.0, rect.center().y)
    } else {
        egui::pos2(rect.left() + 34.0, rect.center().y)
    };
    match state {
        PlayState::Play => play_triangle(p, icon_c, 12.0, pal().on_accent),
        PlayState::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(icon_c, egui::vec2(13.0, 13.0)),
                rad(2),
                pal().on_accent,
            );
        }
        PlayState::Starting | PlayState::Stopping => {
            spinner_arc(p, icon_c, 8.0, phase * std::f32::consts::TAU, pal().on_accent);
            sheen(&p.with_clip_rect(rect), rect, phase);
        }
    }
    let (tx, talign) = if rtl() {
        (rect.right() - 54.0, egui::Align2::RIGHT_CENTER)
    } else {
        (rect.left() + 54.0, egui::Align2::LEFT_CENTER)
    };
    p.text(
        egui::pos2(tx, rect.center().y),
        talign,
        tr(label),
        egui::FontId::proportional(17.0),
        pal().on_accent,
    );
    if busy {
        resp
    } else {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// The dashed "+ Add game" tile at the end of the library grid, drawn at `rect`.
fn add_tile(ui: &mut egui::Ui, rect: egui::Rect, resp: egui::Response, acts: &mut Acts) {
    let poster = egui::Rect::from_min_size(rect.min, egui::vec2(tile_w(), tile_h()));
    let color = if resp.hovered() { pal().accent } else { pal().faint };
    let p = ui.painter();
    p.rect_filled(poster, rad(10), pal().card.gamma_multiply(0.5));
    let dash = egui::Shape::dashed_line(
        &[
            poster.left_top(),
            poster.right_top(),
            poster.right_bottom(),
            poster.left_bottom(),
            poster.left_top(),
        ],
        egui::Stroke::new(1.5, color),
        6.0,
        5.0,
    );
    p.extend(dash);
    let c = poster.center();
    p.line_segment([c - egui::vec2(12.0, 0.0), c + egui::vec2(12.0, 0.0)], egui::Stroke::new(2.0, color));
    p.line_segment([c - egui::vec2(0.0, 12.0), c + egui::vec2(0.0, 12.0)], egui::Stroke::new(2.0, color));
    p.text(
        c + egui::vec2(0.0, 30.0),
        egui::Align2::CENTER_TOP,
        tr("Add game"),
        egui::FontId::proportional(14.0),
        color,
    );
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.add = true;
    }
}

/// One "current artwork" entry on the Artwork page: preview, note, actions.
fn art_current(
    ui: &mut egui::Ui,
    kind: ArtKind,
    note: &str,
    custom: bool,
    acts: &mut Acts,
    preview: impl FnOnce(&mut egui::Ui),
) {
    preview(ui);
    vcol(ui, |ui| {
        let title = match kind {
            ArtKind::Icon => "Icon",
            ArtKind::Cover => "Cover",
            ArtKind::Hero => "Hero",
        };
        ui.label(egui::RichText::new(tr(title)).strong().color(pal().text));
        ui.label(egui::RichText::new(tr(note)).color(pal().subtext).small());
        ui.horizontal(|ui| {
            if ui.small_button(tr("Local file…")).clicked() {
                acts.art.push(ArtCmd::Local(kind));
            }
            if custom && ui.small_button(tr("Reset")).clicked() {
                acts.art.push(ArtCmd::Reset(kind));
            }
        });
    });
}

/// The game's exe (or, for Windows games, its Proton build) is missing.
fn game_missing_files(g: &CustomGame) -> bool {
    !Path::new(&g.exe).is_file()
        || (g.kind == GameKind::Windows && !Path::new(&g.proton_dir).join("proton").is_file())
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

impl App {
    /// The normal window: sidebar, status bar and the current page.
    fn draw_desktop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let ctx = ctx.clone();
        let game_count = self.cfg.custom_games.len();

        let nav = if rtl() { egui::Panel::right("nav") } else { egui::Panel::left("nav") };
        nav.exact_size(226.0)
            .resizable(false)
            .show_separator_line(true)
            .frame(
                egui::Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(egui::Margin::symmetric(14, 16)),
            )
            .show(ui, |ui| dir_ui(ui, |ui| {
                // Brand
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                    brand_mark(ui, rect);
                    ui.add_space(8.0);
                    vcol(ui, |ui| {
                        ui.add_space(1.0);
                        ui.label(
                            egui::RichText::new("Gamelo")
                                .size(17.0)
                                .strong()
                                .color(pal().text),
                        );
                        ui.label(
                            egui::RichText::new(tr("Your games, in one place"))
                                .size(11.5)
                                .color(pal().faint),
                        );
                    });
                });
                ui.add_space(20.0);

                ui.label(egui::RichText::new(tr("LIBRARY")).size(11.5).strong().color(pal().faint));
                ui.add_space(6.0);
                // The selection highlight is drawn underneath and slides between items.
                let highlight = ui.painter().add(egui::Shape::Noop);
                let mut selected: Option<egui::Rect> = None;
                let r = nav_item(ui, self.page == Page::Home, "Home", None);
                if self.page == Page::Home {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Home;
                }
                let in_library = matches!(self.page, Page::Library | Page::Game | Page::Artwork | Page::Saves);
                let r = nav_item(ui, in_library, "Library", Some(game_count));
                if in_library {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Library;
                }
                let r = nav_item(ui, self.page == Page::Stats, "Stats", None);
                if self.page == Page::Stats {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Stats;
                }
                let r = nav_item(ui, self.page == Page::Stores, "Stores", None);
                if self.page == Page::Stores {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Stores;
                }
                if nav_item(ui, false, "Couch mode", None).on_hover_text(tr("Fullscreen, for the sofa and a controller (F11 or the Guide button)")).clicked() {
                    self.couch_toggle(&ctx);
                }

                ui.add_space(16.0);
                ui.label(egui::RichText::new(tr("TOOLS")).size(11.5).strong().color(pal().faint));
                ui.add_space(6.0);
                let missing = self
                    .deps
                    .iter()
                    .filter(|r| !r.found && r.action.is_some())
                    .count();
                // A newer GE-Proton than any installed one (for people who use GE).
                let uses_ge = self.proton_tools.iter().any(|t| t.label.starts_with("GE-Proton"));
                if uses_ge && !self.proton.releases.contains_key("ge") {
                    self.load_releases(&ctx, 0, false);
                }
                let update = match self.proton.releases.get("ge") {
                    Some(Ok(list)) if uses_ge => list
                        .iter()
                        .find(|r| !r.prerelease)
                        .filter(|r| !self.proton_tools.iter().any(|t| t.label.starts_with(&r.tag)))
                        .map(|r| r.tag.clone()),
                    _ => None,
                };
                let r = nav_item(ui, self.page == Page::Proton, "Proton", None);
                if let Some(tag) = &update {
                    let dot = if rtl() { egui::pos2(r.rect.left() + 16.0, r.rect.center().y) } else { egui::pos2(r.rect.right() - 16.0, r.rect.center().y) };
                    ui.painter().circle_filled(dot, 4.0, pal().accent);
                    let _ = r.clone().on_hover_text(trf("{} is available", &[tag]));
                }
                if self.page == Page::Proton {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Proton;
                }
                if self.download.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Working…")).color(pal().subtext).small());
                    });
                } else {
                    let deps_label = if missing > 0 {
                        format!("{}  ({missing})", lg("Dependencies"))
                    } else {
                        lg("Dependencies").to_string()
                    };
                    let r = nav_item(ui, self.page == Page::Dependencies, &deps_label, None);
                    if self.page == Page::Dependencies {
                        selected = Some(r.rect);
                    }
                    if r.clicked() {
                        self.page = Page::Dependencies;
                    }
                }
                let r = nav_item(ui, self.page == Page::Cleanup, "Disk space", None);
                if self.page == Page::Cleanup {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Cleanup;
                }
                let r = nav_item(ui, self.page == Page::Settings, "Settings", None);
                if self.page == Page::Settings {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Settings;
                }
                if let Some(r) = selected {
                    let (now, secs) = (self.now, self.secs(0.22));
                    let tw = self.motion.nav_y.get_or_insert(Tween::at(r.top(), now));
                    let y = tw.follow(r.top(), now, secs);
                    self.motion.busy |= !tw.settled(now, secs);
                    let hr = egui::Rect::from_min_size(egui::pos2(r.left(), y), r.size());
                    let edge = if rtl() { hr.right() - 3.0 } else { hr.left() };
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(edge, hr.top() + 7.0),
                        egui::pos2(edge + 3.0, hr.bottom() - 7.0),
                    );
                    ui.painter().set(
                        highlight,
                        egui::Shape::Vec(vec![
                            egui::Shape::rect_filled(hr, rad(9), pal().accent.gamma_multiply(0.16)),
                            egui::Shape::rect_filled(bar, rad(2), pal().accent),
                        ]),
                    );
                }

                // Pinned status footer
                let mut playing: Vec<&str> = self.running.values().map(|m| m.name.as_str()).collect();
                playing.sort_unstable();
                let footer_h = 48.0 + 24.0 * playing.len() as f32 + 46.0 * self.tasks.len() as f32;
                let avail = ui.available_height();
                if avail > footer_h {
                    ui.add_space(avail - footer_h);
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(8.0);
                let umu_ok = self.deps.iter().any(|r| r.name == "umu-run" && r.found);
                status_line(ui, !umu_ok, "umu-run", if umu_ok { "ready" } else { "missing" });
                for name in playing {
                    status_line(ui, false, &tr("Playing"), &disp(name));
                }
                for t in &self.tasks {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(egui::RichText::new(tr(&t.label)).color(pal().subtext).small()).truncate());
                    let bar = egui::ProgressBar::new(t.fraction()).desired_height(6.0).corner_radius(rad(3));
                    ui.add(if t.progress.is_some() { bar } else { bar.animate(true) });
                }
            }));

        egui::Panel::bottom("status")
            .show_separator_line(true)
            .frame(
                egui::Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| dir_ui(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = if self.status_err {
                        pal().warn
                    } else if self.status.is_empty() {
                        pal().subtext
                    } else {
                        pal().info()
                    };
                    // New messages fade and slide in, so a change is noticed.
                    let t = self.progress(self.status_since, 0.3);
                    status_dot(ui, color, !self.status.is_empty() || self.status_err);
                    ui.add_space((1.0 - t) * 8.0);
                    let msg = if self.status.is_empty() { tr("Ready") } else { tr(&self.status) };
                    let label = ui.add(
                        egui::Label::new(
                            egui::RichText::new(msg).color(color.gamma_multiply(0.3 + 0.7 * t)).small(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    // While a controller is in use, a reminder of what its buttons do.
                    let pad_recent = self.pad.as_ref().and_then(|p| p.nav.last_used).is_some_and(|t| self.now - t < 20.0);
                    if pad_recent {
                        ui.with_layout(far_end(), |ui| {
                            ui.label(egui::RichText::new(tr("A open · B back · Start play · LB/RB tabs")).color(pal().faint).small());
                        });
                    }
                    if !self.status.is_empty() {
                        // Errors often end in "— see <log>": offer to open it.
                        let log = self
                            .status
                            .split_whitespace()
                            .rev()
                            .map(|w| w.trim_matches(|c| matches!(c, '\u{2066}'..='\u{2069}')))
                            .find(|w| w.ends_with(".log"))
                            .map(str::to_string);
                        let message = self.status.clone();
                        label.context_menu(|ui| menu_ui(ui, |ui| {
                            if ui.button(tr("Copy message")).clicked() {
                                acts.copy = Some(message.clone());
                                ui.close();
                            }
                            if let Some(log) = &log
                                && ui.button(tr("Open log")).clicked()
                            {
                                acts.open = Some(log.clone());
                                ui.close();
                            }
                        }));
                    }
                });
                ui.add_space(5.0);
            }));

        // Pages fade and rise into place when you switch to them.
        if self.motion.page != Some(self.page) {
            let prev = self.motion.page.replace(self.page);
            self.cache_bytes = None;
            self.motion.page_since = self.now;
            self.start_flight(&ctx, prev);
        }
        let enter = self.progress(self.motion.page_since, 0.24);
        egui::CentralPanel::default().show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 26, right: 14, top: 22, bottom: 22 })
                .show(ui, |ui| {
                    // Each page (and each game) keeps its own scroll position.
                    egui::ScrollArea::vertical()
                        .id_salt((self.page, self.selected.clone()))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.multiply_opacity(enter);
                            ui.add_space((1.0 - enter) * 14.0);
                            // The right stick scrolls; so do Up/Down outside the library grid.
                            let mut dy = -self.pad_scroll;
                            if self.page != Page::Library && ctx.memory(|m| m.focused().is_none()) && !overlay_open(&ctx) {
                                let (up, down) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                                dy += 120.0 * (up as i32 - down as i32) as f32;
                            }
                            if dy != 0.0 {
                                ui.scroll_with_delta(egui::vec2(0.0, dy));
                            }
                            // Keep content clear of the floating scroll bar on the right
                            // (right-to-left pages are flush against it otherwise).
                            let edge = egui::Margin { right: 12, ..Default::default() };
                            egui::Frame::new().inner_margin(edge).show(ui, |ui| dir_ui(ui, |ui| match self.page {
                                Page::Home => self.page_home(ui, &ctx, acts),
                                Page::Stats => self.page_stats(ui, &ctx, acts),
                                Page::Library => self.page_library(ui, &ctx, acts),
                                Page::Game => self.page_game(ui, &ctx, acts),
                                Page::Artwork => self.page_artwork(ui, &ctx, acts),
                                Page::Settings => self.page_settings(ui, acts),
                                Page::Dependencies => self.page_dependencies(ui, acts),
                                Page::Proton => self.page_proton(ui, &ctx, acts),
                                Page::Saves => self.page_saves(ui, &ctx, acts),
                                Page::Stores => self.page_stores(ui, &ctx, acts),
                                Page::Cleanup => self.page_cleanup(ui, &ctx, acts),
                            }));
                        });
                });
        });
    }

    /// One frame of the whole window (kept separate from eframe so it can be
    /// rendered offscreen by the screenshot test).
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.motion.busy = false;
        self.motion.looping = false;
        self.motion.focused = ctx.input(|i| i.focused);
        // egui's own transitions (hover fades, collapsing) follow "Reduce motion" too.
        let anim = if self.cfg.settings.reduce_motion { 0.0 } else { 1.0 / 12.0 };
        if (ctx.global_style().animation_time - anim).abs() > 1e-6 {
            ctx.all_styles_mut(|s| s.animation_time = anim);
        }
        self.sync_theme(&ctx);
        set_tile_size(&self.cfg.settings.tile_size);
        self.proton_poll();
        self.saves_poll();
        self.scan_poll();
        self.hltb_poll();
        self.store_poll(&ctx);
        self.stores_poll(&ctx);
        self.cleanup_poll();
        self.opti_poll();
        self.poll_tasks(&ctx);
        self.art_poll(&ctx);
        self.poll_running();

        // Keep the configured UI scale in sync with egui's zoom. If the user
        // changed zoom via the built-in shortcuts (Ctrl +/- or Ctrl+scroll),
        // adopt that value; otherwise apply the configured one.
        let cur = ctx.zoom_factor();
        let want = self.cfg.settings.zoom();
        if (cur - self.last_zoom).abs() > 1e-4 {
            // Ctrl +/−, Ctrl 0 or Ctrl+scroll: adopt it, and save once it settles.
            self.cfg.settings.zoom = cur;
            self.last_zoom = cur;
            self.zoom_save_at = Some(self.now + 0.8);
        } else if (want - cur).abs() > 1e-4 {
            ctx.set_zoom_factor(want);
            self.last_zoom = want;
        }

        // Poll any in-flight download job.
        let mut dl_result: Option<Result<String, String>> = None;
        let mut dl_label = String::new();
        if let Some(dl) = &self.download {
            match dl.rx.try_recv() {
                Ok(r) => dl_result = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    dl_result = Some(Err(lg("download task ended unexpectedly").into()));
                }
            }
            if dl_result.is_some() {
                dl_label = dl.label.clone();
            }
        }
        if let Some(r) = dl_result {
            self.download = None;
            match r {
                Ok(msg) => {
                    self.refresh();
                    self.set_status(msg, false);
                }
                Err(e) => self.set_status(format!("{dl_label}: {e}"), true),
            }
        }

        let mut acts = Acts::default();
        // F11 (a controller's Guide button) toggles couch mode.
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) && ctx.memory(|m| m.focused().is_none()) {
            self.couch_toggle(&ctx);
        }
        if self.couch.is_some() {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ui, |ui| self.couch_ui(ui, &ctx, &mut acts));
        } else {
            self.draw_desktop(ui, &ctx, &mut acts);
        }

        self.setup_modal(&ctx, &mut acts);
        if let Some(m) = self.add_modal_ui(&ctx) {
            self.add_game(&ctx, m);
        }
        self.remove_modal_ui(&ctx, &mut acts);
        self.proton_delete_modal(&ctx);
        self.saves_modals(&ctx);
        self.hltb_modal(&ctx);
        self.quick_modal(&ctx, &mut acts);
        self.tray_poll(&ctx, &mut acts);
        self.store_picker(&ctx);
        self.screenshot_viewer(&ctx);
        self.log_viewer(&ctx);
        if let Some(list) = self.scan_modal(&ctx) {
            self.add_games(&ctx, list);
        }
        self.steam_import_modal(&ctx);
        self.collection_modal(&ctx);
        self.stores_modals(&ctx);
        self.apply_acts(&ctx, acts);

        if let Some(at) = self.zoom_save_at {
            if self.now >= at {
                self.zoom_save_at = None;
                save_config(&self.cfg);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(at - self.now));
            }
        }
        // A flight whose destination wasn't drawn (filtered out, say) just ends.
        if self.motion.flight.as_ref().is_some_and(|f| self.now - f.since > 0.5) {
            self.motion.flight = None;
        }
        if self.motion.busy {
            ctx.request_repaint();
        } else if self.motion.looping && ctx.input(|i| i.focused) {
            // Loops only run while the window has focus, so they cost nothing mid-game.
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            // Notice games started or stopped outside the window (app-menu shortcuts).
            let tick = if self.running.is_empty() { 3 } else { 1 };
            ctx.request_repaint_after(Duration::from_secs(tick));
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

/// Find a regular-weight system UI font. egui's bundled Ubuntu Light is thin
/// and hard to read at small sizes.
fn find_ui_font() -> Option<(String, egui::FontData)> {
    use skrifa::MetadataProvider;
    for family in ["Inter", "Noto Sans", "Cantarell", "Roboto", "Open Sans", "DejaVu Sans"] {
        let out = Command::new("fc-match")
            .args(["-f", "%{family[0]}\n%{file}\n%{index}", &format!("{family}:style=Regular")])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let (Some(found), Some(file)) = (lines.next(), lines.next()) else {
            continue;
        };
        // fc-match always answers with *something*; only take the family we asked for.
        if !found.eq_ignore_ascii_case(family) {
            continue;
        }
        let index: u32 = lines.next().and_then(|i| i.parse().ok()).unwrap_or(0);
        let Ok(bytes) = fs::read(file) else { continue };
        // Validate before handing it to egui: a bad font would abort at startup.
        let usable = skrifa::FontRef::from_index(&bytes, index)
            .is_ok_and(|f| "Aa0".chars().all(|c| f.charmap().map(c).is_some()));
        if !usable {
            continue;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        return Some((family.to_string(), data));
    }
    None
}

/// Arabic glyphs (including the joined presentation forms the shaper emits),
/// bundled so Arabic works whatever fonts the system has. SIL OFL, see assets/.
const ARABIC_FONT: &[u8] = include_bytes!("../assets/NotoSansArabicUI-subset.ttf");

/// Put the system UI font first and egui's bundled fonts after it as glyph
/// fallbacks. The Arabic font goes before all of them: it has nothing but
/// Arabic glyphs, and owning every Arabic character keeps egui's shaping runs
/// predictable (see `shaping`) whatever the system font covers.
fn load_ui_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((name, data)) = find_ui_font() {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, name);
    }
    fonts
        .font_data
        .insert("arabic".into(), Arc::new(egui::FontData::from_static(ARABIC_FONT)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().insert(0, "arabic".into());
    }
    ctx.set_fonts(fonts);
}

fn configure_style(ctx: &egui::Context) {
    load_ui_font(ctx);
    ctx.all_styles_mut(|style| {
        // egui's defaults (Small = 9 px, Body = 12.5 px) are too small to read comfortably.
        use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, Proportional)),
            (TextStyle::Body, FontId::new(14.5, Proportional)),
            (TextStyle::Button, FontId::new(14.5, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
            (TextStyle::Heading, FontId::new(20.0, Proportional)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.menu_margin = egui::Margin::same(8);

    });
}

/// ProtonLaunch was renamed Gamelo. Move its folders over once, leaving a link
/// at each old path so anything still pointing there (prefixes, Steam, logs)
/// keeps working, and point saved paths and game shortcuts at the new names.
fn migrate_old_name() {
    let cache = thumb_cache_dir().parent().map(Path::to_path_buf).unwrap_or_default();
    let old_data = home().join(".local/share/protonlaunch");
    for (old, new) in [
        (home().join(".config/protonlaunch"), config_path().parent().map(Path::to_path_buf).unwrap_or_default()),
        (old_data.clone(), data_dir()),
        (cache.with_file_name("protonlaunch"), cache),
    ] {
        if !new.exists() && old.is_dir() && !old.is_symlink() && fs::rename(&old, &new).is_ok() {
            let _ = std::os::unix::fs::symlink(&new, &old);
        }
    }
    let old_prefix = format!("{}/", old_data.display());
    if let Ok(s) = fs::read_to_string(config_path())
        && s.contains(&old_prefix)
    {
        let _ = fs::write(config_path(), s.replace(&old_prefix, &format!("{}/", data_dir().display())));
    }
    // Game shortcuts in the app menu; not from a development build, which
    // would point them at itself.
    let exe = self_exe();
    if exe.components().any(|c| c.as_os_str() == "target") {
        return;
    }
    let apps = home().join(".local/share/applications");
    for e in fs::read_dir(&apps).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix("protonlaunch-").and_then(|n| n.strip_suffix(".desktop")) else { continue };
        let Ok(s) = fs::read_to_string(e.path()) else { continue };
        let exec = format!("Exec={} --run {id}", desktop_exec_arg(&exe.to_string_lossy()));
        let s: Vec<String> = s
            .lines()
            .map(|l| if l.starts_with("Exec=") { exec.clone() } else { l.replace("ProtonLaunch", "Gamelo") })
            .collect();
        if fs::write(apps.join(format!("gamelo-{id}.desktop")), s.join("\n") + "\n").is_ok() {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// The app icon, shared by the window, the sidebar and Settings.
const ICON_PNG: &[u8] = include_bytes!("../packaging/gamelo.png");

fn icon_rgba() -> image::RgbaImage {
    image::load_from_memory(ICON_PNG).map(|i| i.to_rgba8()).unwrap_or_default()
}

/// The icon as a texture (made once, with mipmaps so it stays crisp small).
fn brand_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("gamelo-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = icon_rgba();
    let image = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img);
    let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
    let tex = ctx.load_texture("gamelo-icon", image, options);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Paint the app icon into `rect`.
fn brand_mark(ui: &egui::Ui, rect: egui::Rect) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(brand_icon(ui.ctx()).id(), rect, uv, egui::Color32::WHITE);
}

fn main() -> eframe::Result {
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
mod tests {
    use super::*;

    #[test]
    fn shell_quote_basic() {
        assert_eq!(
            shell_quote("PROTON_FSR4_UPGRADE=1"),
            "PROTON_FSR4_UPGRADE=1"
        );
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn tokenize_respects_quotes() {
        assert_eq!(
            tokenize(r#"-windowed "-name foo bar" -x"#),
            vec!["-windowed", "-name foo bar", "-x"]
        );
    }

    #[test]
    fn flag_envs_follow_flags() {
        let f = GameFlags {
            fsr4: Fsr4Mode::Rrna3,
            watermark: true,
            ntsync: true,
            ..Default::default()
        };
        let envs = flag_envs(&f);
        assert!(envs.contains(&("PROTON_FSR4_RDNA3_UPGRADE", "1")));
        assert!(envs.contains(&("PROTON_FSR4_INDICATOR", "1")));
        assert!(envs.contains(&("PROTON_USE_NTSYNC", "1")));
        assert!(!envs.iter().any(|(k, _)| *k == "PROTON_ENABLE_WAYLAND"));
    }

    #[test]
    fn url_encode_path_segment() {
        assert_eq!(url_encode("Half-Life 2"), "Half-Life%202");
        assert_eq!(url_encode("a/b?c&d"), "a%2Fb%3Fc%26d");
        assert_eq!(url_encode("Café"), "Caf%C3%A9");
    }

    #[test]
    fn old_config_still_loads() {
        // A config written before umu_id / icon / steamgriddb_key existed.
        let json = r#"{"custom_games":[{"name":"G","exe":"/g.exe"}],"settings":{"zoom":1.0}}"#;
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.custom_games[0].umu_id, "");
        assert_eq!(cfg.custom_games[0].icon, "");
        assert_eq!(cfg.custom_games[0].cover, "");
        assert_eq!(cfg.custom_games[0].hero, "");
        assert!(!cfg.custom_games[0].art_checked);
        assert_eq!(cfg.settings.steamgriddb_key, "");
    }

    #[test]
    fn year_from_timestamp() {
        assert_eq!(year_of(0), 1970);
        assert_eq!(year_of(1_444_918_080), 2015); // Oct 2015
        assert_eq!(year_of(1_704_067_199), 2023); // 2023-12-31 23:59:59
        assert_eq!(year_of(1_704_067_200), 2024); // 2024-01-01 00:00:00
    }

    #[test]
    fn image_fitting() {
        let wide = image::RgbaImage::new(1000, 500);
        assert_eq!(cover_fit(&wide, 600, 900).dimensions(), (600, 900));
        assert_eq!(fit_within(&wide, 240, 360).dimensions(), (240, 120));
        let grey = image::RgbaImage::from_pixel(600, 900, image::Rgba([200; 4]));
        let hero = hero_from(&grey, true);
        assert_eq!(hero.dimensions(), (640, 210));
        // Dimmed, darkest bottom-left (behind the title), brightest top-right.
        let (bl, tr) = (hero.get_pixel(0, 209).0, hero.get_pixel(639, 0).0);
        assert!(bl[0] < tr[0] && bl[0] < 200, "banner must be darker bottom-left");
        assert_eq!(bl[3], 255);
        assert_eq!(hero_from(&grey, false).dimensions(), (1280, 420));
        let poster = generated_poster(Some(&image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255]))), "x");
        assert_eq!(poster.dimensions(), (300, 450));
        assert_eq!(generated_poster(None, "x").dimensions(), (300, 450));
        let small = image::RgbaImage::new(64, 64);
        assert_eq!(fit_within(&small, 240, 360).dimensions(), (64, 64));
    }

    /// Hits the real SteamGridDB API with the key from the user's config.
    /// Run with: cargo test -- --ignored sgdb_live
    #[test]
    #[ignore]
    fn sgdb_live() {
        let key = load_config().settings.steamgriddb_key;
        assert!(!key.trim().is_empty(), "no SteamGridDB key in config");
        let games = sgdb_search("Broforce", key.trim()).unwrap();
        let id = games.first().expect("no match").id;
        for kind in [ArtKind::Icon, ArtKind::Cover] {
            let (assets, _) = sgdb_assets(kind, id, 0, key.trim()).unwrap();
            let first = assets.first().expect("no assets");
            assert!(load_thumb(&first.thumb).is_some(), "thumbnail failed: {}", first.thumb);
        }
    }

    #[test]
    fn rename_keeps_auto_prefix() {
        let base = std::env::temp_dir().join(format!("plaunch-prefixes-{}", std::process::id()));
        fs::create_dir_all(base.join("townfall")).unwrap();
        let settings = Settings {
            prefix_base: base.to_string_lossy().into(),
            ..Default::default()
        };
        let old = CustomGame {
            name: "Townfall".into(),
            ..Default::default()
        };
        // Renamed with an auto prefix: pinned to the existing one.
        let mut renamed = CustomGame {
            name: "silent hill Townfall".into(),
            ..Default::default()
        };
        assert!(keep_prefix_on_rename(&old, &mut renamed, &settings));
        assert_eq!(game_prefix(&renamed, &settings), base.join("townfall"));
        // Same slug (only case changed): nothing to do.
        let mut recased = CustomGame {
            name: "TOWNFALL".into(),
            ..Default::default()
        };
        assert!(!keep_prefix_on_rename(&old, &mut recased, &settings));
        assert!(recased.prefix.is_empty());
        // Old game had no prefix on disk yet: nothing to keep.
        let fresh = CustomGame {
            name: "Never Launched".into(),
            ..Default::default()
        };
        let mut fresh_renamed = CustomGame {
            name: "Other".into(),
            ..Default::default()
        };
        assert!(!keep_prefix_on_rename(&fresh, &mut fresh_renamed, &settings));
        // A custom prefix the user cleared on purpose is not brought back.
        let custom = CustomGame {
            name: "Townfall".into(),
            prefix: "/somewhere/else".into(),
            ..Default::default()
        };
        let mut cleared = CustomGame {
            name: "Renamed".into(),
            ..Default::default()
        };
        assert!(!keep_prefix_on_rename(&custom, &mut cleared, &settings));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn guesses_names_from_paths() {
        let g = |p: &str| guess_game_name(Path::new(p));
        assert_eq!(g("/games/BroForce-v2354/BroForce.v2354/BroForce.v2354/Broforce.exe"), "BroForce");
        assert_eq!(g("/d/Games/Resonance - A Plague Tale Legacy/Resonance.exe"), "Resonance - A Plague Tale Legacy");
        assert_eq!(g("/d/Games/Silent Hill Townfall/Townfall/Binaries/Win64/Townfall-Win64-Shipping.exe"), "Townfall");
        assert_eq!(g("/d/Games/Half Life 2/hl2.exe"), "Half Life 2");
        assert_eq!(g("/home/u/Games/game.exe"), "game");
        assert_eq!(g("/g/Thronefall-v1-61/Thronefall.v1.61/Thronefall.v1.61/Thronefall.exe"), "Thronefall");
        assert_eq!(g("/g/Enter-the-Gungeon-v2-1-9/Enter.the.Gungeon.v2.1.9/EtG.exe"), "Enter the Gungeon");
        assert_eq!(g("/g/Streets.of.Rogue.v98.2/StreetsOfRogue.exe"), "Streets of Rogue");
    }

    #[test]
    fn desktop_exec_args_are_escaped() {
        assert_eq!(desktop_exec_arg("/usr/bin/gamelo"), "/usr/bin/gamelo");
        assert_eq!(desktop_exec_arg("/a b/app"), "\"/a b/app\"");
        assert_eq!(desktop_exec_arg("/100%/app"), "/100%%/app");
        assert_eq!(desktop_exec_arg("/a $x/app"), "\"/a \\\\$x/app\"");
    }

    #[test]
    fn best_match_prefers_exact_then_verified() {
        let game = |id, name: &str, verified| SgdbGame { id, name: name.into(), year: None, verified };
        let list = [game(1, "Broforce Forever", false), game(2, "Broforce", false), game(3, "Bro", true)];
        assert_eq!(best_match("broforce", &list).unwrap().id, 2);
        assert_eq!(best_match("nothing alike", &list).unwrap().id, 3);
        assert!(best_match("x", &[]).is_none());
    }

    #[test]
    fn sessions_accumulate_in_stats() {
        let dir = std::env::temp_dir().join(format!("plaunch-stats-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        record_session_in(&dir, "abc", 1_000, 600, Some(0));
        record_session_in(&dir, "abc", 5_000, 120, Some(1));
        record_session_in(&dir, "other", 7_000, 30, None);
        let stats = load_stats_in(&dir);
        let s = &stats["abc"];
        assert_eq!((s.playtime_secs, s.last_played, s.last_session_secs, s.last_exit), (720, 5_000, 120, Some(1)));
        assert_eq!(stats["other"].last_exit, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn durations_and_ages_read_naturally() {
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(25 * 60), "25m");
        assert_eq!(format_duration(12 * 3600 + 40 * 60), "12h 40m");
        let day = 86_400;
        assert_eq!(format_ago(10 * day, 10 * day + 100), "today");
        assert_eq!(format_ago(10 * day, 11 * day + 5), "yesterday");
        assert_eq!(format_ago(0, 3 * day), "3 days ago");
        assert_eq!(format_ago(0, 21 * day), "3 weeks ago");
    }

    #[test]
    fn process_tree_includes_children() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let tree = process_tree(std::process::id());
        assert_eq!(tree[0], std::process::id());
        assert!(tree.contains(&child.id()), "spawned child missing from tree");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A tiny CPU rasterizer for egui's output, so the UI can be checked
    /// without a window.
    struct Raster {
        w: usize,
        h: usize,
        px: Vec<[f32; 4]>,
        textures: HashMap<egui::TextureId, egui::ColorImage>,
    }

    impl Raster {
        fn new(w: usize, h: usize) -> Self {
            Raster { w, h, px: vec![[0.0; 4]; w * h], textures: HashMap::new() }
        }

        fn apply(&mut self, delta: &egui::epaint::textures::TexturesDelta) {
            for (id, deltas) in &delta.set {
                for d in deltas {
                    let egui::epaint::ImageData::Color(img) = &d.image;
                    match d.pos {
                        None => {
                            self.textures.insert(*id, (**img).clone());
                        }
                        Some([x0, y0]) => {
                            let Some(t) = self.textures.get_mut(id) else { continue };
                            for y in 0..img.size[1] {
                                for x in 0..img.size[0] {
                                    t.pixels[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                                }
                            }
                        }
                    }
                }
            }
            for id in &delta.free {
                self.textures.remove(id);
            }
        }

        fn sample(t: &egui::ColorImage, u: f32, v: f32) -> [f32; 4] {
            let x = ((u * t.size[0] as f32) as usize).min(t.size[0] - 1);
            let y = ((v * t.size[1] as f32) as usize).min(t.size[1] - 1);
            let c = t.pixels[y * t.size[0] + x];
            [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
        }

        fn draw(&mut self, prims: &[egui::ClippedPrimitive]) {
            for p in prims {
                let egui::epaint::Primitive::Mesh(m) = &p.primitive else { continue };
                let tex = self.textures.get(&m.texture_id).cloned();
                let clip = p.clip_rect;
                for tri in m.indices.as_chunks::<3>().0 {
                    let v = tri.map(|i| m.vertices[i as usize]);
                    let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
                    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    let x0 = a.x.min(b.x).min(c.x).max(clip.min.x).max(0.0) as usize;
                    let y0 = a.y.min(b.y).min(c.y).max(clip.min.y).max(0.0) as usize;
                    let x1 = (a.x.max(b.x).max(c.x).min(clip.max.x).ceil() as usize).min(self.w);
                    let y1 = (a.y.max(b.y).max(c.y).min(clip.max.y).ceil() as usize).min(self.h);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                            let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                            let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                            let w2 = 1.0 - w0 - w1;
                            if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                                continue;
                            }
                            let mut col = [0.0f32; 4];
                            for (k, vert) in v.iter().enumerate() {
                                let wk = [w0, w1, w2][k];
                                let c = vert.color;
                                col[0] += wk * c.r() as f32 / 255.0;
                                col[1] += wk * c.g() as f32 / 255.0;
                                col[2] += wk * c.b() as f32 / 255.0;
                                col[3] += wk * c.a() as f32 / 255.0;
                            }
                            if let Some(t) = &tex {
                                let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                                let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                                let s = Self::sample(t, u, vv);
                                for i in 0..4 {
                                    col[i] *= s[i];
                                }
                            }
                            // Premultiplied "over".
                            let d = &mut self.px[y * self.w + x];
                            for i in 0..4 {
                                d[i] = col[i] + d[i] * (1.0 - col[3]);
                            }
                        }
                    }
                }
            }
        }

        fn save(&self, path: &Path) {
            let img = image::RgbaImage::from_fn(self.w as u32, self.h as u32, |x, y| {
                let p = self.px[y as usize * self.w + x as usize];
                image::Rgba([(p[0] * 255.0) as u8, (p[1] * 255.0) as u8, (p[2] * 255.0) as u8, 255])
            });
            img.save(path).unwrap();
        }
    }

    /// Render the real UI offscreen (with a copy of your library) and save PNGs.
    /// Run alone with: SHOTS=/some/dir cargo test -- --ignored ui_screenshots
    #[test]
    #[ignore]
    fn ui_screenshots() {
        let out = PathBuf::from(std::env::var("SHOTS").expect("set SHOTS to an output dir"));
        fs::create_dir_all(&out).unwrap();
        // Work on a copy of the config so nothing here touches the real one.
        let fake_home = std::env::temp_dir().join(format!("plaunch-shots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&fake_home);
        fs::create_dir_all(fake_home.join(".config/gamelo")).unwrap();
        let _ = fs::copy(config_path(), fake_home.join(".config/gamelo/config.json"));
        // SAFETY: this ignored test is run on its own, so no other thread reads the env.
        unsafe {
            std::env::set_var("HOME", &fake_home);
        }
        let ctx = egui::Context::default();
        configure_style(&ctx);
        let mut app = App::new(find_steam_root());
        app.page = Page::Library;
        app.cfg.settings.zoom = 1.0; // in memory only
        app.cfg.settings.language = "en".into();
        app.cfg.settings.tile_size = String::new();
        app.cfg.settings.no_store = true;
        app.sort = SortMode::Recent;
        app.cfg.custom_games.iter_mut().for_each(|g| g.favorite = false);
        // Online cards change height as data arrives; keep the layout fixed.
        app.cfg.settings.no_hltb = true;
        let (w, h) = (1240usize, 860usize);
        let mut raster = Raster::new(w, h);
        let mut t = 0.0;
        let dt = std::cell::Cell::new(0.1);
        let mut frame = |app: &mut App, raster: &mut Raster, events: Vec<egui::Event>, name: Option<&str>| {
            t += dt.get();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| app.draw(ui));
            raster.apply(&output.textures_delta);
            output.textures_delta.clear();
            let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
            if let Some(name) = name {
                raster.px.iter_mut().for_each(|p| *p = [0.0; 4]);
                raster.draw(&prims);
                raster.save(&out.join(name));
            }
        };
        let click = |pos: egui::Pos2| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            ]
        };
        let release = |pos: egui::Pos2| {
            vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]
        };
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("1-library.png"));
        // Hover the first poster (226 sidebar + 26 margin, below the toolbar).
        // Posters start below the toolbar and the filter row.
        let first = egui::pos2(226.0 + 26.0 + 75.0, 280.0);
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(first)], None);
        frame(&mut app, &mut raster, vec![], Some("2-library-hover.png"));
        // Click the poster away from its play button to open the game page.
        let open = egui::pos2(226.0 + 26.0 + 20.0, 250.0);
        frame(&mut app, &mut raster, click(open), None);
        frame(&mut app, &mut raster, release(open), None);
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))], None);
        }
        frame(&mut app, &mut raster, vec![], Some("3-game.png"));
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 600.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -600.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("4-game-scrolled.png"));
        // The add dialog and the remove confirmation.
        app.add_modal = Some(app.new_add_modal(PathBuf::from("/home/u/Games/Hollow Knight/hollow_knight.exe")));
        for _ in 0..2 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("5-add.png"));
        app.add_modal = None;
        app.confirm_remove = app.selected.clone();
        for _ in 0..2 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("6-remove.png"));
        // Artwork page layout (no search, so no network).
        app.confirm_remove = None;
        app.art.game = app.selected_index();
        app.page = Page::Artwork;
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("7-artwork.png"));

        // Typing in a field and then leaving through the sidebar must still save it.
        app.page = Page::Game;
        app.game_tab = GameTab::Settings;
        // A fixed Performance card above Launch: no overlay rows, no frame cap.
        if let Some(i) = app.selected_index() {
            app.cfg.custom_games[i].flags.mangohud = false;
            app.cfg.custom_games[i].flags.gamescope = false;
            app.cfg.custom_games[i].tuning = Tuning::default();
        }
        for _ in 0..2 {
            frame(&mut app, &mut raster, vec![], None);
        }
        // This page remembers its scroll from earlier; go back to the top.
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 3000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        // Bring the Launch card's fields into view.
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -200.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
        for _ in 0..6 {
            frame(&mut app, &mut raster, vec![], None);
        }
        let args_field = egui::pos2(800.0, 616.0);
        frame(&mut app, &mut raster, click(args_field), None);
        frame(&mut app, &mut raster, release(args_field), None);
        frame(&mut app, &mut raster, vec![egui::Event::Text("-dx11".into())], None);
        let sidebar_library = egui::pos2(110.0, 184.0);
        frame(&mut app, &mut raster, click(sidebar_library), None);
        frame(&mut app, &mut raster, release(sidebar_library), None);
        let g = &app.cfg.custom_games[app.selected_index().unwrap()];
        assert!(app.page == Page::Library);
        assert!(g.args.ends_with("-dx11"), "edit lost on sidebar navigation: {:?}", g.args);
        let saved = fs::read_to_string(config_path()).unwrap();
        assert!(saved.contains("-dx11"), "edit not written to the config");

        // Animation states, captured mid-transition.
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        app.search = "town".into();
        dt.set(0.07);
        frame(&mut app, &mut raster, vec![], None);
        frame(&mut app, &mut raster, vec![], Some("8-glide-mid.png"));
        dt.set(0.1);
        app.search.clear();
        for _ in 0..6 {
            frame(&mut app, &mut raster, vec![], None);
        }
        let ids: Vec<String> = app.cfg.custom_games.iter().map(|g| g.id.clone()).collect();
        app.motion.launching.insert(ids[0].clone(), (0.0, std::process::id()));
        app.auto_pending.insert(ids[1].clone());
        dt.set(0.37);
        frame(&mut app, &mut raster, vec![], None);
        frame(&mut app, &mut raster, vec![], Some("9-starting-and-fetching.png"));
        app.selected = Some(ids[0].clone());
        app.page = Page::Game;
        dt.set(0.1);
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("10-game-starting.png"));
        // Back to the grid: the poster flies home to its slot.
        app.motion.launching.clear();
        app.auto_pending.clear();
        app.page = Page::Library;
        dt.set(0.02);
        frame(&mut app, &mut raster, vec![], None);
        dt.set(0.17);
        frame(&mut app, &mut raster, vec![], Some("11-flight-home.png"));
        dt.set(0.1);
        for _ in 0..5 {
            frame(&mut app, &mut raster, vec![], None);
        }
        // A session just ended: playtime counts up with a fading "+1h 25m".
        app.motion.played.insert(ids[3].clone(), (3600, 3600 + 85 * 60, None));
        app.stats.insert(ids[3].clone(), GameStats { playtime_secs: 3600 + 85 * 60, last_played: unix_now() - 60, ..Default::default() });
        dt.set(0.2);
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("12-playtime-count.png"));
        // Right-click menus: a poster, empty library space, a text field.
        app.motion.played.clear();
        dt.set(0.1);
        let right = |pos: egui::Pos2, pressed: bool| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() },
            ]
        };
        let away = egui::pos2(1150.0, 780.0);
        for (pos, name) in [
            (egui::pos2(335.0, 350.0), "13-menu-poster.png"),
            (egui::pos2(900.0, 640.0), "14-menu-library.png"),
            (egui::pos2(400.0, 127.0), "15-menu-text.png"),
        ] {
            for _ in 0..3 {
                frame(&mut app, &mut raster, vec![], None);
            }
            frame(&mut app, &mut raster, right(pos, true), None);
            frame(&mut app, &mut raster, right(pos, false), None);
            frame(&mut app, &mut raster, vec![], None);
            frame(&mut app, &mut raster, vec![], Some(name));
            frame(&mut app, &mut raster, click(away), None);
            frame(&mut app, &mut raster, release(away), None);
        }
        // The text menu works: "Clear" empties the search box.
        app.search = "ved".into();
        let search = egui::pos2(400.0, 127.0);
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, right(search, true), None);
        frame(&mut app, &mut raster, right(search, false), None);
        frame(&mut app, &mut raster, vec![], Some("16-menu-text-filled.png"));
        let clear_item = egui::pos2(428.0, 317.0);
        frame(&mut app, &mut raster, click(clear_item), None);
        frame(&mut app, &mut raster, release(clear_item), None);
        assert!(app.search.is_empty(), "Clear didn't clear: {:?}", app.search);
        // Settings, top and bottom; then a scale preset click applies and saves.
        app.page = Page::Settings;
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("17-settings.png"));
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -2000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, vec![], Some("18-settings-bottom.png"));
        // Clicking a scale preset applies it and saves it straight away.
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 3000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
        for _ in 0..4 {
            frame(&mut app, &mut raster, vec![], None);
        }
        let preset_125 = egui::pos2(679.0, 698.0);
        frame(&mut app, &mut raster, click(preset_125), None);
        frame(&mut app, &mut raster, release(preset_125), None);
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        assert_eq!(app.cfg.settings.zoom, 1.25);
        assert!((ctx.zoom_factor() - 1.25).abs() < 1e-4, "zoom not applied: {}", ctx.zoom_factor());
        let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path()).unwrap()).unwrap();
        assert_eq!(saved.settings.zoom, 1.25, "preset not saved");
        // Ctrl − changes it too, and is saved once it settles.
        let minus = egui::Event::Key {
            key: egui::Key::Minus,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        frame(&mut app, &mut raster, vec![minus], None);
        for _ in 0..12 {
            frame(&mut app, &mut raster, vec![], None);
        }
        let z = app.cfg.settings.zoom;
        assert!(z < 1.25, "Ctrl − ignored: {z}");
        let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path()).unwrap()).unwrap();
        assert!((saved.settings.zoom - z).abs() < 1e-4, "Ctrl − zoom not saved: {} vs {z}", saved.settings.zoom);
        let _ = fs::remove_dir_all(&fake_home);
    }

    /// Offscreen driver for the real UI: feed frames, save PNGs.
    struct Harness {
        ctx: egui::Context,
        app: App,
        raster: Raster,
        t: f64,
        out: PathBuf,
        home: PathBuf,
    }

    impl Harness {
        /// The UI on a copy of your config (in a fake HOME), at 1240×860.
        fn new(tag: &str) -> Self {
            let out = PathBuf::from(std::env::var("SHOTS").expect("set SHOTS to an output dir"));
            fs::create_dir_all(&out).unwrap();
            let real = config_path();
            let home = std::env::temp_dir().join(format!("plaunch-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            fs::create_dir_all(home.join(".config/gamelo")).unwrap();
            let _ = fs::copy(&real, home.join(".config/gamelo/config.json"));
            // Stats, artwork cache and save data are read from the real data dir copy.
            let real_data = data_dir();
            let data = home.join(".local/share/gamelo");
            fs::create_dir_all(&data).unwrap();
            let _ = fs::copy(real_data.join("stats.json"), data.join("stats.json"));
            fs::create_dir_all(data.join("cache")).unwrap();
            if let Ok(rd) = fs::read_dir(real_data.join("cache")) {
                for e in rd.flatten() {
                    let _ = fs::copy(e.path(), data.join("cache").join(e.file_name()));
                }
            }
            // Read-only view of Steam, so "find saves elsewhere" has something to find.
            let real_steam = real.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|h| h.join(".local/share/Steam"));
            if let Some(steam) = real_steam.filter(|s| s.is_dir()) {
                let _ = std::os::unix::fs::symlink(steam, data.parent().unwrap().join("Steam"));
            }
            // SAFETY: these ignored tests run alone (--test-threads=1).
            unsafe {
                std::env::set_var("HOME", &home);
            }
            let ctx = egui::Context::default();
            configure_style(&ctx);
            let mut app = App::new(find_steam_root());
            // Start from the defaults whatever the real config has picked.
            app.cfg.settings.zoom = 1.0;
            app.cfg.settings.language = "en".into();
            app.cfg.settings.tile_size = String::new();
            app.cfg.settings.no_store = true; // no network in screenshots
            app.sort = SortMode::Recent;
            app.cfg.custom_games.iter_mut().for_each(|g| g.favorite = false);
            Harness { ctx, app, raster: Raster::new(1240, 860), t: 0.0, out, home }
        }

        fn frame_dt(&mut self, dt: f64, events: Vec<egui::Event>, name: Option<&str>) {
            self.t += dt;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))),
                time: Some(self.t),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let mut output = self.ctx.run_ui(input, |ui| app.draw(ui));
            self.raster.apply(&output.textures_delta);
            output.textures_delta.clear();
            let prims = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            if let Some(name) = name {
                self.raster.px.iter_mut().for_each(|p| *p = [0.0; 4]);
                self.raster.draw(&prims);
                self.raster.save(&self.out.join(name));
            }
        }

        fn idle(&mut self, n: usize) {
            for _ in 0..n {
                self.frame_dt(0.1, vec![], None);
            }
        }

        fn shot(&mut self, name: &str) {
            self.idle(4);
            self.frame_dt(0.1, vec![], Some(name));
        }

        fn scroll(&mut self, dy: f32) {
            let ev = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, dy),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Default::default(),
            };
            self.frame_dt(0.1, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), ev], None);
            self.idle(3);
        }

        fn click(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
            let ev = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: Default::default() };
            self.frame_dt(0.1, vec![egui::Event::PointerMoved(pos), ev(true)], None);
            self.frame_dt(0.1, vec![ev(false)], None);
        }

        fn hover(&mut self, pos: egui::Pos2) {
            self.frame_dt(0.1, vec![egui::Event::PointerMoved(pos)], None);
            self.idle(8);
        }

        fn key(&mut self, key: egui::Key) {
            let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
            self.frame_dt(0.1, vec![ev(true), ev(false)], None);
        }

        /// Show `page` (for game pages, of the first game).
        fn page(&mut self, page: Page) {
            if self.app.selected.is_none() {
                self.app.selected = self.app.cfg.custom_games.first().map(|g| g.id.clone());
            }
            self.app.page = page;
            self.idle(2);
            self.scroll(5000.0);
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    /// Light mode, square corners, other themes: SHOTS=dir cargo test -- --ignored theme_shots --test-threads=1
    #[test]
    #[ignore]
    fn theme_shots() {
        let mut h = Harness::new("themes");
        h.app.cfg.settings.theme_mode = "light".into();
        h.page(Page::Library);
        h.shot("t1-light-library.png");
        h.page(Page::Game);
        h.shot("t2-light-game.png");
        h.page(Page::Settings);
        h.shot("t3-light-settings.png");
        h.app.cfg.settings.square_corners = true;
        h.app.cfg.settings.light_theme = "rose".into();
        h.page(Page::Library);
        h.shot("t4-rose-square-library.png");
        h.app.cfg.settings.theme_mode = "dark".into();
        h.app.cfg.settings.dark_theme = "ocean".into();
        h.page(Page::Game);
        h.shot("t5-ocean-square-game.png");
        h.app.cfg.settings.square_corners = false;
        h.app.cfg.settings.dark_theme = "galaxy".into();
        h.page(Page::Settings);
        h.shot("t6-galaxy-settings.png");
        h.app.cfg.settings.theme_mode = "light".into();
        h.app.cfg.settings.light_theme = "arctic".into();
        h.page(Page::Proton);
        h.shot("t7-arctic-proton.png");
        h.page(Page::Saves);
        h.shot("t8-arctic-saves.png");
        h.app.cfg.settings.light_theme = "botanical".into();
        h.app.game_tab = GameTab::Mods;
        h.page(Page::Game);
        h.scroll(-400.0);
        h.shot("t9-botanical-mods.png");
        h.app.cfg.settings.theme_mode = "dark".into();
        h.idle(1);
        assert!(pal().dark);
    }

    /// The Proton page with live release data: SHOTS=dir cargo test -- --ignored proton_shots --test-threads=1
    #[test]
    #[ignore]
    fn proton_shots() {
        let mut h = Harness::new("proton");
        h.page(Page::Proton);
        for _ in 0..150 {
            if h.app.proton.releases.contains_key("ge") && h.app.proton.fixes.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            h.idle(1);
        }
        assert!(matches!(h.app.proton.releases.get("ge"), Some(Ok(l)) if !l.is_empty()), "no GE releases");
        h.shot("p1-proton.png");
        h.scroll(-700.0);
        h.shot("p2-proton-mid.png");
        h.scroll(-3000.0);
        h.shot("p3-proton-fixes.png");
    }

    /// The Save Vault: SHOTS=dir cargo test -- --ignored saves_shots --test-threads=1
    #[test]
    #[ignore]
    fn saves_shots() {
        let mut h = Harness::new("saves");
        let town = h.app.cfg.custom_games.iter().find(|g| g.name.to_lowercase().contains("townfall")).map(|g| g.id.clone());
        h.app.selected = town.or_else(|| h.app.cfg.custom_games.first().map(|g| g.id.clone()));
        h.page(Page::Game);
        for _ in 0..50 {
            if h.app.saves.index.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            h.idle(1);
        }
        assert!(h.app.saves.index.is_some(), "save index not loaded");
        h.scroll(-900.0);
        h.shot("s1-game-saves-card.png");
        h.page(Page::Saves);
        h.shot("s2-vault.png");
        // Scan for other copies (button in the third card).
        let i = h.app.selected_index().unwrap();
        let gs = h.app.game_saves(i);
        let found = saves::find_elsewhere(&gs, &h.app.cfg.settings);
        h.app.saves.found.insert(gs.game.id.clone(), found);
        h.scroll(-700.0);
        h.shot("s3-vault-elsewhere.png");
        h.scroll(-3000.0);
        h.shot("s4-vault-sync.png");
        h.app.saves.search = Some("silent hill".into());
        h.shot("s5-db-search.png");
    }

    /// Game page cards that fetch online info: SHOTS=dir cargo test -- --ignored game_shots --test-threads=1
    #[test]
    #[ignore]
    fn game_shots() {
        let mut h = Harness::new("game");
        let valheim = h.app.cfg.custom_games.iter().find(|g| g.name == "Valheim").map(|g| g.id.clone());
        h.app.selected = valheim.or_else(|| h.app.cfg.custom_games.first().map(|g| g.id.clone()));
        let id = h.app.selected.clone().unwrap();
        h.app.stats.insert(id.clone(), GameStats { playtime_secs: 31 * 3600 + 20 * 60, last_played: unix_now() - 7200, ..Default::default() });
        h.page(Page::Game);
        for _ in 0..80 {
            if h.app.hltb.cache.contains_key(&id) || h.app.hltb.errors.contains_key(&id) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            h.idle(1);
        }
        assert!(h.app.hltb.cache.get(&id).is_some_and(|e| e.times.is_some()), "no HLTB data: {:?}", h.app.hltb.errors.get(&id));
        h.app.motion.page_since = h.app.now;
        h.scroll(-380.0);
        h.idle(10);
        h.shot("g1-hltb.png");
    }

    /// The OptiScaler card (live release list): SHOTS=dir cargo test -- --ignored opti_shots --test-threads=1
    #[test]
    #[ignore]
    fn opti_shots() {
        let mut h = Harness::new("opti");
        let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone());
        h.app.selected = id;
        h.page(Page::Game);
        for _ in 0..60 {
            if h.app.opti.releases.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            h.idle(1);
        }
        h.scroll(-1500.0);
        h.shot("o1-optiscaler.png");
        h.app.cfg.settings.theme_mode = "light".into();
        h.shot("o2-optiscaler-light.png");
    }

    /// The interface in Arabic (RTL): SHOTS=dir cargo test -- --ignored arabic_shots --test-threads=1
    #[test]
    #[ignore]
    fn arabic_shots() {
        let mut h = Harness::new("arabic");
        h.app.cfg.settings.language = "ar".into();
        h.page(Page::Library);
        h.shot("a1-library.png");
        let id = h.app.cfg.custom_games.iter().find(|g| g.name == "Valheim").map(|g| g.id.clone());
        h.app.selected = id;
        h.page(Page::Game);
        h.shot("a2-game.png");
        h.scroll(-600.0);
        h.shot("a3-game-mid.png");
        h.page(Page::Settings);
        h.shot("a4-settings.png");
        h.scroll(-900.0);
        h.shot("a5-settings-mid.png");
        h.page(Page::Proton);
        h.shot("a6-proton.png");
        h.page(Page::Saves);
        h.shot("a7-saves.png");
        h.app.confirm_remove = h.app.selected.clone();
        h.shot("a8-remove.png");
        h.app.confirm_remove = None;
        h.app.cfg.settings.theme_mode = "light".into();
        h.page(Page::Library);
        h.shot("a9-library-light.png");
        i18n::set_lang(i18n::Lang::En);
    }

    /// Store info on the game page (needs the network: real Steam data):
    /// SHOTS=dir cargo test -- --ignored store_shots --test-threads=1
    #[test]
    #[ignore]
    fn store_shots() {
        let mut h = Harness::new("store");
        h.app.cfg.settings.no_hltb = true;
        h.app.cfg.settings.no_store = false;
        let id = h.app.cfg.custom_games.iter().find(|g| g.name.to_lowercase().contains("broforce")).map(|g| g.id.clone()).expect("Broforce in the library");
        h.app.selected = Some(id.clone());
        for (lang, steam) in [("en", "english"), ("ar", "arabic")] {
            h.app.cfg.settings.language = lang.into();
            let info = store::details(274190, steam).unwrap();
            h.app.store.cache.insert(id.clone(), store::Entry { info, fetched: unix_now(), pinned: true });
            h.page(Page::Game);
            h.app.game_tab = GameTab::Overview;
            h.scroll(-380.0);
            // Let the thumbnails arrive.
            for _ in 0..80 {
                h.idle(1);
                if h.app.store.images.len() >= 4 && h.app.store.loading.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            h.shot(&format!("s1-about-{lang}.png"));
            h.app.store.viewer = Some((id.clone(), 1));
            for _ in 0..60 {
                h.idle(1);
                if h.app.store.loading.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            h.shot(&format!("s2-viewer-{lang}.png"));
            h.key(egui::Key::ArrowRight);
            assert_eq!(h.app.store.viewer.as_ref().map(|v| v.1), Some(2), "arrow flips to the next screenshot");
            h.key(egui::Key::Escape);
            h.idle(2);
            assert!(h.app.store.viewer.is_none() && h.app.page == Page::Game, "Escape closes the viewer only");
        }
    }

    /// Per-game tuning rows and the Disk space page:
    /// SHOTS=dir cargo test -- --ignored tune_shots --test-threads=1
    #[test]
    #[ignore]
    fn tune_shots() {
        let mut h = Harness::new("tune");
        let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone()).unwrap();
        let i = h.app.index_of(&id).unwrap();
        h.app.cfg.custom_games[i].flags.mangohud = true;
        h.app.cfg.custom_games[i].flags.gamescope = false;
        h.app.cfg.custom_games[i].tuning = Tuning { fps_limit: 60, overlay: "standard".into(), position: "top-right".into(), custom: String::new() };
        h.app.selected = Some(id.clone());
        h.page(Page::Game);
        h.app.game_tab = GameTab::Settings;
        h.idle(2);
        h.scroll(-380.0);
        h.shot("t1-tuning.png");
        let l = build_launch(&h.app.cfg.custom_games[i], &h.app.cfg.settings);
        if let Ok(l) = l {
            assert!(l.envs.iter().any(|(k, v)| k == "MANGOHUD_CONFIG" && v.contains("fps_limit=60") && v.contains("position=top-right")));
        }
        // A prefix no game uses, a shader cache and some logs to clean up.
        let orphan = h.app.cfg.settings.prefix_base().join("old_game");
        fs::create_dir_all(orphan.join("drive_c/users")).unwrap();
        fs::write(orphan.join("system.reg"), vec![0u8; 3_000_000]).unwrap();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        fs::create_dir_all(home.join(".cache/mesa_shader_cache")).unwrap();
        fs::write(home.join(".cache/mesa_shader_cache/index"), vec![0u8; 5_000_000]).unwrap();
        fs::create_dir_all(data_dir().join("logs")).unwrap();
        fs::write(data_dir().join("logs/a.log"), "x".repeat(40_000)).unwrap();
        h.page(Page::Cleanup);
        for _ in 0..30 {
            h.idle(1);
            if h.app.cleanup.sizes.lock().unwrap().values().all(|v| v.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        h.shot("t2-disk-space.png");
        assert!(h.app.orphan_prefixes().contains(&orphan));
        // Moving it to the Trash takes it off the list.
        let trash = home.join(".local/share/Trash");
        assert!(cleanup::move_to_trash(&orphan).is_ok());
        assert!(!orphan.exists() && (trash.join("files/old_game").is_dir() || find_in_path("gio").is_some()));
        h.app.cfg.settings.language = "ar".into();
        h.page(Page::Cleanup);
        h.shot("t3-disk-space-ar.png");
        h.app.cfg.settings.language = "en".into();
    }

    /// The Stores page in each state (no network: a stand-in tool and account):
    /// SHOTS=dir cargo test -- --ignored stores_shots --test-threads=1
    #[test]
    #[ignore]
    fn stores_shots() {
        const STORE_TILE: (f32, f32) = (600.0, 400.0);
        const ACCOUNT_CHIP: (f32, f32) = (960.0, 59.0);
        const ACCOUNT_CHIP_AR: (f32, f32) = (260.0, 59.0);
        use stores::{Installed, Owned, SizeInfo, Store};
        let mut h = Harness::new("stores");
        h.app.page = Page::Stores;
        h.page(Page::Stores);
        h.shot("st1-no-tool.png");
        // A stand-in tool: signed out shows the sign-in steps.
        let tools = data_dir().join("tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(tools.join("legendary"), "#!/bin/sh\nexit 1\n").unwrap();
        h.app.stores.code = "not a code".into();
        h.idle(3);
        h.shot("st2-sign-in.png");
        h.app.stores.code = "0123456789abcdef0123456789abcdef".into();
        h.app.stores.opened.insert(Store::Epic);
        h.app.stores.errors.insert(Store::Epic, "the code has expired".into());
        h.idle(3);
        h.shot("st2b-code-ok.png");
        h.app.stores.errors.clear();
        h.app.stores.code.clear();
        h.app.cfg.settings.language = "ar".into();
        h.idle(3);
        h.shot("st2c-sign-in-ar.png");
        h.app.cfg.settings.language = "en".into();
        // Signed in, with a library (the refresh is marked done so nothing runs).
        let cfg = std::env::var_os("HOME").map(PathBuf::from).unwrap().join(".config/legendary");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("user.json"), r#"{"displayName":"Tester","account_id":"x"}"#).unwrap();
        h.app.stores.loaded.insert(Store::Epic);
        // Covers from the copied library stand in for store art (already "downloaded").
        let covers: Vec<String> = h.app.cfg.custom_games.iter().map(|g| g.cover.clone()).filter(|c| Path::new(c).is_file()).collect();
        let owned = |app: &str, title: &str, version: &str| {
            let k = app.len() % covers.len().max(1);
            let cover = covers.get(k).map(|c| {
                let url = format!("https://test/{app}.jpg");
                let path = store::image_path(&url);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::copy(c, &path).unwrap();
                url
            });
            Owned { app: app.into(), title: title.into(), version: version.into(), cover: cover.unwrap_or_default(), ..Default::default() }
        };
        h.app.stores.owned.insert(Store::Epic, vec![owned("Fortnite", "Fortnite", "2"), owned("Hades", "Hades", "1"), owned("Control", "Control", "5"), owned("Celeste", "Celeste", "1"), owned("Alan", "Alan Wake 2", "1")]);
        h.app.stores.installed.insert(Store::Epic, vec![
            Installed { app: "Hades".into(), title: "Hades".into(), path: "/games/Hades".into(), version: "1".into() },
            Installed { app: "Control".into(), title: "Control".into(), path: "/games/Control".into(), version: "4".into() },
        ]);
        h.idle(3);
        h.shot("st3-library.png");
        h.hover(egui::pos2(STORE_TILE.0, STORE_TILE.1));
        h.shot("st3b-hover.png");
        h.click(egui::pos2(STORE_TILE.0, STORE_TILE.1 + 40.0), egui::PointerButton::Secondary);
        h.idle(3);
        h.shot("st3c-menu.png");
        h.key(egui::Key::Escape);
        h.app.cfg.settings.library_view = "list".into();
        h.idle(3);
        h.shot("st3d-list.png");
        h.app.cfg.settings.library_view = "grid".into();
        h.click(egui::pos2(ACCOUNT_CHIP.0, ACCOUNT_CHIP.1), egui::PointerButton::Primary);
        h.idle(4);
        h.shot("st3e-account.png");
        h.key(egui::Key::Escape);
        h.app.cfg.settings.language = "ar".into();
        h.idle(4);
        h.click(egui::pos2(ACCOUNT_CHIP_AR.0, ACCOUNT_CHIP_AR.1), egui::PointerButton::Primary);
        h.idle(4);
        h.shot("st3g-account-ar.png");
        h.app.cfg.settings.language = "en".into();
        h.idle(2);
        h.key(egui::Key::Escape);
        h.idle(2);
        h.app.stores.show = stores_ui::Show::Updates;
        h.idle(2);
        h.shot("st4-updates.png");
        h.app.stores.show = stores_ui::Show::All;
        h.app.stores.confirm = Some((Store::Epic, owned("Celeste", "Celeste", "1"), Some(Ok(SizeInfo { download: 1_200_000_000, disk: 1_650_000_000, folder: "Celeste".into() }))));
        h.idle(3);
        h.shot("st5-install.png");
        h.app.stores.confirm = None;
        h.app.stores.uninstall = Some((Store::Epic, Installed { app: "Hades".into(), title: "Hades".into(), path: "/games/Hades".into(), version: "1".into() }, true));
        h.idle(3);
        h.shot("st6-uninstall.png");
        h.app.stores.uninstall = None;
        h.app.cfg.settings.language = "ar".into();
        h.idle(3);
        h.shot("st7-library-ar.png");
        h.app.stores.tab = Store::Gog;
        h.idle(3);
        h.shot("st8-gog-ar.png");
        h.app.cfg.settings.language = "en".into();
    }

    /// Ctrl+K quick search and first-run setup:
    /// SHOTS=dir cargo test -- --ignored polish_shots --test-threads=1
    #[test]
    #[ignore]
    fn polish_shots() {
        let mut h = Harness::new("polish");
        h.page(Page::Library);
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let ev = |pressed| egui::Event::Key { key: egui::Key::K, physical_key: None, pressed, repeat: false, modifiers: ctrl };
        h.frame_dt(0.1, vec![ev(true), ev(false)], None);
        h.idle(3);
        h.shot("p1-quick-empty.png");
        h.frame_dt(0.1, vec![egui::Event::Text("re".into())], None);
        h.idle(2);
        h.key(egui::Key::ArrowDown);
        h.idle(2);
        h.shot("p2-quick-typed.png");
        h.app.cfg.settings.language = "ar".into();
        h.idle(3);
        h.shot("p3-quick-ar.png");
        h.key(egui::Key::Escape);
        h.idle(2);
        assert!(h.app.quick.is_none(), "Escape closes quick search");
        h.app.setup = Some(setup::Setup::default());
        h.idle(3);
        h.shot("p4-setup-ar.png");
        h.app.cfg.settings.language = "en".into();
        h.idle(3);
        h.shot("p5-setup-tools.png");
        h.app.setup = Some(setup::Setup { step: 1 });
        h.idle(3);
        h.shot("p6-setup-games.png");
        h.app.setup = None;
    }

    /// Home and Stats with two months of made-up sessions:
    /// SHOTS=dir cargo test -- --ignored home_shots --test-threads=1
    #[test]
    #[ignore]
    fn home_shots() {
        let mut h = Harness::new("home");
        let order = h.app.library_order();
        let ids: Vec<String> = order.iter().map(|&i| h.app.cfg.custom_games[i].id.clone()).collect();
        // Sessions on most days of the last ~70 days, heavier at weekends.
        let now = unix_now();
        let mut log = Vec::new();
        for back in 0..70u64 {
            if back % 5 == 3 {
                continue;
            }
            let start = now - back * 86_400 - 3 * 3600;
            let weekend = sessions::weekday(sessions::day_of(start)) >= 5;
            let secs = (25 + (back * 37) % 140) * 60 * if weekend { 2 } else { 1 };
            log.push(sessions::Session { id: ids[(back as usize / 3) % 4].clone(), start, secs });
        }
        for s in &log {
            let st = h.app.stats.entry(s.id.clone()).or_default();
            st.playtime_secs += s.secs;
            st.last_played = st.last_played.max(s.start);
        }
        h.app.sessions = log;
        h.app.cfg.custom_games[order[6]].status = GameStatus::Backlog;
        h.app.home.free = Some(vec![]);
        h.app.page = Page::Home;
        h.idle(4);
        h.shot("h1-home.png");
        h.scroll(-600.0);
        h.shot("h2-home-bottom.png");
        h.app.page = Page::Stats;
        h.idle(2);
        h.scroll(5000.0);
        h.shot("h3-stats.png");
        h.scroll(-700.0);
        h.shot("h4-stats-mid.png");
        h.scroll(-2000.0);
        h.shot("h5-stats-bottom.png");
        h.app.home.range = stats_ui::Range::Year;
        h.scroll(5000.0);
        h.shot("h6-stats-year.png");
        h.app.cfg.settings.language = "ar".into();
        h.app.page = Page::Home;
        h.idle(3);
        h.scroll(5000.0);
        h.shot("h7-home-ar.png");
        h.app.page = Page::Stats;
        h.idle(3);
        h.scroll(5000.0);
        h.shot("h8-stats-ar.png");
        h.scroll(-700.0);
        h.shot("h9-stats-ar-mid.png");
        h.app.cfg.settings.language = "en".into();
    }

    /// Statuses, collections and the list view:
    /// SHOTS=dir cargo test -- --ignored org_shots --test-threads=1
    #[test]
    #[ignore]
    fn org_shots() {
        use egui::PointerButton::{Primary, Secondary};
        let mut h = Harness::new("org");
        h.app.cfg.settings.collections = vec!["Co-op".into(), "Soulslikes".into()];
        let order = h.app.library_order();
        let set = [GameStatus::Playing, GameStatus::Finished, GameStatus::Backlog, GameStatus::Dropped, GameStatus::Backlog];
        for (k, st) in set.into_iter().enumerate() {
            h.app.cfg.custom_games[order[k]].status = st;
        }
        h.app.cfg.custom_games[order[0]].collections = vec!["Co-op".into()];
        h.app.cfg.custom_games[order[3]].collections = vec!["Co-op".into(), "Soulslikes".into()];
        h.page(Page::Library);
        h.shot("o1-grid-statuses.png");
        // Only filters with games show up; picking one narrows the grid.
        let labels: Vec<String> = h.app.filter_options().into_iter().map(|(_, l)| l).collect();
        assert!(labels.contains(&"Backlog".to_string()) && labels.contains(&"Co-op".to_string()));
        h.app.lib_filter = LibFilter::Status(GameStatus::Backlog);
        h.idle(4);
        assert_eq!(h.app.library_order().len(), 2);
        h.shot("o2-filter-backlog.png");
        h.app.lib_filter = LibFilter::Collection("Co-op".into());
        h.idle(4);
        assert_eq!(h.app.library_order().len(), 2);
        h.app.lib_filter = LibFilter::All;
        // List view, sorted by clicking a header.
        h.app.cfg.settings.library_view = "list".into();
        h.page(Page::Library);
        h.shot("o3-list.png");
        // Right-click a row: its game menu, with Status and Collections.
        h.click(egui::pos2(500.0, 520.0), Secondary);
        h.idle(2);
        h.shot("o4-row-menu.png");
        h.key(egui::Key::Escape);
        // A new collection from the dialog.
        h.app.collection_edit = Some(CollectionEdit { game: Some(h.app.cfg.custom_games[order[1]].id.clone()), ..Default::default() });
        h.idle(2);
        h.frame_dt(0.1, vec![egui::Event::Text("Cozy".into())], None);
        h.shot("o5-new-collection.png");
        h.key(egui::Key::Enter);
        h.idle(2);
        assert!(h.app.cfg.settings.collections.contains(&"Cozy".to_string()));
        assert!(h.app.cfg.custom_games[order[1]].collections.contains(&"Cozy".to_string()));
        // Rename it, then delete it: games follow.
        h.app.collection_edit = Some(CollectionEdit { game: None, renaming: Some("Cozy".into()), name: "Comfy".into() });
        h.idle(2);
        h.key(egui::Key::Enter);
        h.idle(2);
        assert!(h.app.cfg.custom_games[order[1]].collections.contains(&"Comfy".to_string()));
        h.app.cfg.settings.library_view = String::new();
        // Arabic list view.
        h.app.cfg.settings.language = "ar".into();
        h.app.cfg.settings.library_view = "list".into();
        h.page(Page::Library);
        h.shot("o6-list-ar.png");
        h.app.cfg.settings.library_view = String::new();
        h.app.cfg.settings.language = "en".into();
        // Couch details: View steps the status.
        h.page(Page::Library);
        h.key(egui::Key::F11);
        h.idle(4);
        h.key(egui::Key::I);
        let focus = h.app.couch.as_ref().unwrap().focus.clone().unwrap();
        let before = h.app.cfg.custom_games[h.app.index_of(&focus).unwrap()].status;
        h.key(egui::Key::F);
        h.idle(2);
        assert_eq!(h.app.cfg.custom_games[h.app.index_of(&focus).unwrap()].status, before.next());
        h.shot("o7-couch-status.png");
        h.key(egui::Key::Escape);
        h.key(egui::Key::Escape);
        let _ = Primary;
    }

    /// Couch mode, driven with the keys a controller sends:
    /// SHOTS=dir cargo test -- --ignored couch_shots --test-threads=1
    #[test]
    #[ignore]
    fn couch_shots() {
        let mut h = Harness::new("couch");
        for lang in ["en", "ar"] {
            h.app.cfg.settings.language = lang.into();
            h.page(Page::Library);
            h.key(egui::Key::F11);
            assert!(h.app.couch.is_some(), "F11 / Guide enters couch mode");
            h.idle(8);
            h.shot(&format!("c1-home-{lang}.png"));
            let first = h.app.couch.as_ref().unwrap().focus.clone();
            // Right moves forward in English; Left does in Arabic.
            h.key(if lang == "ar" { egui::Key::ArrowLeft } else { egui::Key::ArrowRight });
            h.frame_dt(0.15, vec![], Some(&format!("c2-moving-{lang}.png")));
            h.idle(8);
            assert_ne!(h.app.couch.as_ref().unwrap().focus, first, "the focus moved");
            h.shot(&format!("c3-second-{lang}.png"));
            h.key(egui::Key::I);
            h.idle(4);
            assert!(h.app.couch.as_ref().unwrap().details);
            h.shot(&format!("c4-details-{lang}.png"));
            h.key(egui::Key::Escape);
            assert!(h.app.couch.as_ref().is_some_and(|c| !c.details), "B closes details first");
            h.key(egui::Key::CloseBracket);
            h.idle(6);
            assert!(h.app.lib_filter != LibFilter::All);
            h.shot(&format!("c5-filter-{lang}.png"));
            h.app.lib_filter = LibFilter::All;
            h.key(egui::Key::Escape);
            assert!(h.app.couch.is_none(), "B leaves couch mode");
        }
    }

    /// Library hover and click animations, frame by frame:
    /// SHOTS=dir cargo test -- --ignored hover_shots --test-threads=1
    #[test]
    #[ignore]
    fn hover_shots() {
        let mut h = Harness::new("hover");
        h.app.cfg.custom_games.retain(|g| g.cover.is_empty() || Path::new(&g.cover).is_file());
        h.page(Page::Library);
        // Tiles sit below the "Jump back in" banner when something was played.
        let banner = h.app.stats.values().any(|s| s.last_played > 0);
        let top = if banner { 338.0 } else { 172.0 };
        let tile = egui::Rect::from_min_size(egui::pos2(260.0, top), egui::vec2(tile_w(), tile_h()));
        // Pointer near the top-left: the light and the art follow it.
        h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.min + egui::vec2(30.0, 40.0))], None);
        h.frame_dt(0.05, vec![], Some("h1-hover-start.png"));
        h.idle(6);
        h.shot("h2-hover-corner.png");
        // On the play button: it swells and gets a halo.
        h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.center())], None);
        h.idle(6);
        h.shot("h3-over-play.png");
        // Held down: the poster squashes.
        h.frame_dt(0.05, vec![egui::Event::PointerButton { pos: tile.center() + egui::vec2(0.0, 70.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], None);
        h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.center() + egui::vec2(0.0, 70.0))], None);
        h.frame_dt(0.05, vec![], Some("h4-pressed.png"));
        h.frame_dt(0.05, vec![egui::Event::PointerMoved(egui::pos2(1230.0, 850.0))], None);
        h.idle(8);
        // Play pressed: rings burst from the button (shown without launching anything).
        let id = h.app.library_order().first().map(|&i| h.app.cfg.custom_games[i].id.clone()).unwrap();
        h.app.motion.ripple.insert(id, h.app.now);
        h.frame_dt(0.2, vec![], Some("h5-ripple.png"));
    }

    /// Arabic menus, dropdowns, dialogs and the remaining pages:
    /// SHOTS=dir cargo test -- --ignored arabic_popups --test-threads=1
    #[test]
    #[ignore]
    fn arabic_popups() {
        use egui::PointerButton::{Primary, Secondary};
        let mut h = Harness::new("arabic-popups");
        h.app.cfg.settings.language = "ar".into();
        h.page(Page::Library);
        h.click(egui::pos2(900.0, 450.0), Secondary);
        h.shot("p1-tile-menu.png");
        h.key(egui::Key::Escape);
        h.click(egui::pos2(90.0, 127.0), Primary);
        h.shot("p2-sort-dropdown.png");
        h.key(egui::Key::Escape);
        h.click(egui::pos2(500.0, 600.0), Secondary);
        h.shot("p3-library-menu.png");
        h.key(egui::Key::Escape);
        h.idle(2);
        h.hover(egui::pos2(255.0, 127.0));
        h.shot("p4-tooltip.png");
        // Typing Arabic into the search box: reads right to left, edits as typed.
        h.click(egui::pos2(840.0, 127.0), Primary);
        h.frame_dt(0.1, vec![egui::Event::Text("لعبة الحرب".into())], None);
        // Straight away, while the caret is lit (it blinks).
        h.frame_dt(0.02, vec![], Some("p4b-search-arabic.png"));
        assert_eq!(h.app.search, "لعبة الحرب");
        // Backspace removes the last letter typed (the leftmost one on screen).
        h.key(egui::Key::Backspace);
        assert_eq!(h.app.search, "لعبة الحر");
        // A click on the far left puts the caret at the end; typing continues there.
        h.click(egui::pos2(712.0, 127.0), Primary);
        h.frame_dt(0.1, vec![egui::Event::Text("ب 2".into())], None);
        assert_eq!(h.app.search, "لعبة الحرب 2");
        // The status bar draws the same text through the label path, for comparison.
        h.app.status = "لعبة الحرب 2".into();
        h.shot("p4c-search-mixed.png");
        h.app.status.clear();
        h.app.search.clear();
        h.key(egui::Key::Escape);
        h.idle(2);
        h.app.add_modal = Some(h.app.new_add_modal(PathBuf::from("/home/u/Games/Hollow Knight/hollow_knight.exe")));
        h.shot("p5-add.png");
        h.app.add_modal = None;
        h.page(Page::Settings);
        h.scroll(-1600.0);
        h.shot("p6-settings-lower.png");
        h.scroll(-1600.0);
        h.shot("p7-settings-bottom.png");
        h.page(Page::Dependencies);
        h.shot("p8-deps.png");
        h.app.selected = h.app.cfg.custom_games.first().map(|g| g.id.clone());
        h.page(Page::Game);
        h.click(egui::pos2(500.0, 360.0), Primary);
        h.shot("p9-more-menu.png");
        h.key(egui::Key::Escape);
        h.idle(2);
        assert!(h.app.page == Page::Game, "Escape closing a menu also left the page");
        h.app.game_tab = GameTab::Settings;
        h.idle(2);
        h.scroll(-400.0);
        h.shot("p10-game-settings.png");
        h.scroll(-600.0);
        h.shot("p11-game-settings-2.png");
        // The "Advanced" header sits on the reading side and opens on click.
        h.click(egui::pos2(935.0, 741.0), Primary);
        h.scroll(-600.0);
        h.shot("p11b-advanced-open.png");
        let log = data_dir().join("logs/test.log");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(&log, "info: start\n0024:err:module:import_dll Library VCRUNTIME140.dll (needed by game.exe) not found\n").unwrap();
        let (lines, hints) = extras::read_log(&log);
        h.app.extras.log = Some(extras::LogView { title: "Test".into(), path: log, lines, hints, errors_only: false });
        h.shot("p12-log.png");
        h.app.extras.log = None;
        let games = PathBuf::from(std::env::var("GAMES").unwrap_or_default());
        if games.is_dir() {
            h.app.page = Page::Library;
            h.app.scan.results = Some(extras::scan_folder(&games, &[]));
            h.app.scan.root = games;
            h.shot("p13-scan.png");
        }
        h.app.scan.results = None;
        let i = h.app.selected_index().unwrap();
        h.app.open_artwork(&h.ctx.clone(), i);
        h.idle(4);
        h.shot("p14-artwork.png");
        i18n::set_lang(i18n::Lang::En);
    }

    /// Game page tabs, gamescope, the log viewer and a folder scan:
    /// SHOTS=dir cargo test -- --ignored extras_shots --test-threads=1
    #[test]
    #[ignore]
    fn extras_shots() {
        let mut h = Harness::new("extras");
        let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone()).unwrap();
        h.app.selected = Some(id.clone());
        let i = h.app.index_of(&id).unwrap();
        h.app.cfg.custom_games[i].flags.gamescope = true;
        h.app.cfg.custom_games[i].env = "DXVK_HUD=fps".into();
        h.app.stats.insert(id.clone(), GameStats { playtime_secs: 4000, last_played: unix_now() - 3600, last_session_secs: 4, last_exit: Some(1), sessions: 3 });
        h.app.cfg.settings.no_hltb = true;
        h.page(Page::Game);
        h.scroll(-330.0);
        // Give the disk-use measurement a moment.
        for _ in 0..50 {
            if h.app.extras.disk.lock().unwrap().get(&id).is_some_and(|v| v.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        h.shot("e1-overview.png");
        h.app.game_tab = GameTab::Settings;
        h.idle(2);
        h.scroll(-330.0);
        h.shot("e2-settings-gamescope.png");
        h.scroll(-500.0);
        h.shot("e3-settings-env.png");
        h.app.game_tab = GameTab::Mods;
        h.scroll(3000.0);
        h.scroll(-600.0);
        h.shot("e4-mods.png");
        // A log with a known problem.
        let log = data_dir().join("logs/test.log");
        fs::create_dir_all(log.parent().unwrap()).unwrap();
        fs::write(&log, "info: start\nwarn: something slow\n0024:err:module:import_dll Library VCRUNTIME140.dll (needed by game.exe) not found\nwine: Unhandled page fault on read access\n").unwrap();
        let (lines, hints) = extras::read_log(&log);
        h.app.extras.log = Some(extras::LogView { title: "Test".into(), path: log, lines, hints, errors_only: false });
        h.shot("e5-log.png");
        h.app.extras.log = None;
        // Favourites lead the grid, under the "Jump back in" banner.
        for g in h.app.cfg.custom_games.iter_mut().skip(3).take(2) {
            g.favorite = true;
        }
        h.page(Page::Library);
        h.shot("e7-library-favorites.png");
        h.app.lib_filter = LibFilter::Favorites;
        h.idle(2);
        h.shot("e8-favorites-only.png");
        h.app.lib_filter = LibFilter::All;
        for size in ["small", "large"] {
            h.app.cfg.settings.tile_size = size.into();
            h.idle(6);
            h.shot(&format!("e12-tiles-{size}.png"));
        }
        h.app.cfg.settings.tile_size = String::new();
        h.idle(6);
        // Arrow keys move a focus ring; Enter opens that game.
        for k in [egui::Key::ArrowRight, egui::Key::ArrowRight, egui::Key::ArrowRight, egui::Key::ArrowDown] {
            h.key(k);
        }
        h.shot("e11-keys.png");
        let order = h.app.library_order();
        let want = h.app.cfg.custom_games[order[7]].id.clone();
        assert_eq!(h.app.lib_cursor.as_deref(), Some(want.as_str()));
        h.key(egui::Key::Enter);
        h.idle(2);
        assert!(h.app.page == Page::Game && h.app.selected.as_deref() == Some(want.as_str()));
        // ] and [ (a controller's RB/LB) step through the game page's tabs.
        h.app.game_tab = GameTab::Overview;
        h.key(egui::Key::CloseBracket);
        assert!(h.app.game_tab == GameTab::Settings);
        h.key(egui::Key::OpenBracket);
        h.key(egui::Key::OpenBracket);
        let last = if h.app.cfg.custom_games[h.app.selected_index().unwrap()].kind == GameKind::Windows { GameTab::Mods } else { GameTab::Settings };
        assert!(h.app.game_tab == last, "[ wraps around to the last tab");
        h.page(Page::Library);
        h.app.cfg.settings.language = "ar".into();
        h.idle(2);
        h.shot("e9-library-ar.png");
        h.page(Page::Game);
        h.scroll(-330.0);
        h.shot("e10-overview-ar.png");
        h.app.cfg.settings.language = "en".into();
        let games = PathBuf::from(std::env::var("GAMES").unwrap_or_default());
        if games.is_dir() {
            h.app.page = Page::Library;
            h.app.scan.results = Some(extras::scan_folder(&games, &[]));
            h.app.scan.root = games;
            h.shot("e6-scan.png");
        }
    }

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plaunch-tmp-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, contents).unwrap();
        p
    }

    /// A Windows game with a fake exe, Proton dir and umu-run on disk.
    fn windows_game(umu_id: &str) -> (CustomGame, Settings) {
        let exe = temp_file("game.exe", "");
        let proton_dir =
            std::env::temp_dir().join(format!("plaunch-tmp-{}/fake-proton", std::process::id()));
        fs::create_dir_all(&proton_dir).unwrap();
        let umu = temp_file("umu-run", "");
        fs::set_permissions(&umu, fs::Permissions::from_mode(0o755)).unwrap();

        let g = CustomGame {
            name: "Test Game".into(),
            kind: GameKind::Windows,
            exe: exe.to_string_lossy().into(),
            args: "-windowed".into(),
            proton_dir: proton_dir.to_string_lossy().into(),
            flags: GameFlags {
                mangohud: true,
                fsr4: Fsr4Mode::Upgrade,
                ..Default::default()
            },
            umu_id: umu_id.into(),
            ..Default::default()
        };
        let settings = Settings {
            umu_bin: umu.to_string_lossy().into(),
            ..Default::default()
        };
        (g, settings)
    }

    #[test]
    fn windows_command_uses_umu() {
        let (g, settings) = windows_game("");
        let cmd = launch_command_string(&g, &settings).unwrap();
        assert!(cmd.contains("PROTON_FSR4_UPGRADE=1"));
        assert!(cmd.contains("MANGOHUD=1"));
        assert!(cmd.contains("WINEPREFIX="));
        assert!(cmd.contains("PROTONPATH="));
        assert!(cmd.contains("GAMEID=umu-test_game"));
        assert!(cmd.contains("umu-run"));
        assert!(cmd.ends_with("game.exe -windowed"));
    }

    #[test]
    fn openxr_bridge_is_off_unless_vr_is_allowed() {
        let (mut g, mut s) = windows_game("");
        g.env = "WINEDLLOVERRIDES=dinput8=n,b".into();
        let ov = |s: &Settings| build_launch(&g, s).unwrap().envs.into_iter().find(|(k, _)| k == "WINEDLLOVERRIDES").map(|(_, v)| v);
        // Off by default, and the user's own overrides are added to it.
        assert_eq!(ov(&s).as_deref(), Some("wineopenxr=d;dinput8=n,b"));
        s.allow_openxr = true;
        assert_eq!(ov(&s).as_deref(), Some("dinput8=n,b"));
    }

    #[test]
    fn tuning_becomes_env() {
        let t = |fps: u32, overlay: &str| Tuning { fps_limit: fps, overlay: overlay.into(), position: "top-right".into(), custom: "fps,\n gpu_temp".into() };
        // No overlay preset: DXVK/VKD3D cap, MangoHud's own config untouched.
        let e = tuning_envs(&t(60, ""), true);
        assert!(e.contains(&("DXVK_FRAME_RATE".into(), "60".into())) && !e.iter().any(|(k, _)| k == "MANGOHUD_CONFIG"));
        // A preset with MangoHud on: everything in MANGOHUD_CONFIG.
        let e = tuning_envs(&t(90, "minimal"), true);
        assert_eq!(e, vec![("MANGOHUD_CONFIG".to_string(), "fps_only,position=top-right,fps_limit=90".to_string())]);
        assert_eq!(tuning_envs(&t(0, "custom"), true), vec![("MANGOHUD_CONFIG".to_string(), "fps,gpu_temp,position=top-right".to_string())]);
        // MangoHud off: presets don't apply, the cap still does.
        assert_eq!(tuning_envs(&t(30, "full"), false).len(), 2);
        assert!(tuning_envs(&t(0, ""), true).is_empty());
    }

    #[test]
    fn gamescope_wraps_and_env_merges() {
        let (mut g, settings) = windows_game("");
        g.flags.gamescope = true;
        g.gamescope_opts = GamescopeOpts { width: 2560, height: 1440, refresh: 144, fullscreen: true, filter: "fsr".into(), ..Default::default() };
        g.env = "# comment\nDXVK_HUD=fps\nexport WINEDLLOVERRIDES=\"xinput1_3=n,b\"\nbad line\n".into();
        let l = build_launch(&g, &settings).unwrap();
        assert_eq!(l.program, "gamescope");
        let a = l.args.join(" ");
        assert!(a.starts_with("-W 2560 -H 1440 -r 144 -f -F fsr --mangoapp -- "), "{a}");
        assert!(a.contains("umu-run"));
        assert!(!l.envs.iter().any(|(k, _)| k == "MANGOHUD"), "mangoapp replaces MANGOHUD");
        assert!(l.envs.contains(&("DXVK_HUD".into(), "fps".into())));
        assert!(l.envs.contains(&("WINEDLLOVERRIDES".into(), "wineopenxr=d;xinput1_3=n,b".into())));
        assert_eq!(parse_env("A=1\n=2\nB C=3\n"), vec![("A".to_string(), "1".to_string())]);
    }

    #[test]
    fn custom_umu_id_is_used() {
        let (g, settings) = windows_game("  umu-1091500 ");
        let cmd = launch_command_string(&g, &settings).unwrap();
        assert!(cmd.contains("GAMEID=umu-1091500 "));
        assert!(!cmd.contains("umu-test_game"));
    }

    #[test]
    fn linux_command_is_direct() {
        let exe = temp_file("native-run", "");
        let g = CustomGame {
            name: "Native".into(),
            kind: GameKind::Linux,
            exe: exe.to_string_lossy().into(),
            flags: GameFlags {
                mangohud: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let cmd = launch_command_string(&g, &Settings::default()).unwrap();
        assert!(!cmd.contains("GAMEID"));
        assert!(!cmd.contains("PROTONPATH"));
        assert!(cmd.contains("mangohud "));
        assert!(cmd.contains("native-run"));
    }
}
