//! The data model: Steam lookup, flags, Proton tools, games, settings and the config file.

use crate::*;

// ---------------------------------------------------------------------------
// Steam install (only used to locate compatibilitytools.d for Proton builds)
// ---------------------------------------------------------------------------

pub(crate) fn find_steam_root() -> Option<PathBuf> {
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
pub(crate) enum Fsr4Mode {
    #[default]
    Off,
    Upgrade,
    Rrna3,
}

impl Fsr4Mode {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Fsr4Mode::Off => "FSR4: Off",
            Fsr4Mode::Upgrade => "FSR4: Upgrade",
            Fsr4Mode::Rrna3 => "FSR4: RDNA3",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GameFlags {
    pub(crate) mangohud: bool,
    pub(crate) fsr4: Fsr4Mode,
    pub(crate) watermark: bool,
    pub(crate) ntsync: bool,
    pub(crate) wayland: bool,
    pub(crate) gamemode: bool,
    /// Turn off umu-protonfixes for this game.
    pub(crate) no_protonfixes: bool,
    /// Run inside gamescope (Valve's micro-compositor).
    pub(crate) gamescope: bool,
}

/// How gamescope runs a game.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct GamescopeOpts {
    /// Output size; 0 = the monitor's.
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Size the game renders at (upscaled to the output); 0 = same as output.
    pub(crate) game_width: u32,
    pub(crate) game_height: u32,
    /// Refresh / frame limit; 0 = unlimited.
    pub(crate) refresh: u32,
    pub(crate) fullscreen: bool,
    /// "", "fsr", "nis", "pixel" or "linear".
    pub(crate) filter: String,
    pub(crate) hdr: bool,
    /// Extra command-line options.
    pub(crate) extra: String,
}

impl GamescopeOpts {
    /// gamescope's options, up to (not including) the `--` before the game.
    pub(crate) fn args(&self, mangoapp: bool) -> Vec<String> {
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
pub(crate) fn parse_env(text: &str) -> Vec<(String, String)> {
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

pub(crate) fn tokenize(s: &str) -> Vec<String> {
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
pub(crate) struct Tuning {
    /// Frames per second; 0 = no cap.
    #[serde(default)]
    pub(crate) fps_limit: u32,
    /// "" (MangoHud's own config), "minimal", "standard", "full" or "custom".
    #[serde(default)]
    pub(crate) overlay: String,
    /// MangoHud position ("top-left", "top-right"…); "" = its default.
    #[serde(default)]
    pub(crate) position: String,
    /// Custom MangoHud options, comma or line separated.
    #[serde(default)]
    pub(crate) custom: String,
}

/// Environment for a game's tuning. With MangoHud on and a preset chosen, it
/// all goes into MANGOHUD_CONFIG (whose limiter works for every API).
/// Otherwise the cap uses DXVK's and VKD3D's own limiters, so a MangoHud.conf
/// the user keeps is never overridden.
pub(crate) fn tuning_envs(t: &Tuning, mangohud: bool) -> Vec<(String, String)> {
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

pub(crate) fn flag_envs(f: &GameFlags) -> Vec<(&'static str, &'static str)> {
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

pub(crate) struct ProtonTool {
    pub(crate) label: String,
    pub(crate) dir: PathBuf,
}

/// Directories that may contain Proton builds (GE-Proton etc.).
pub(crate) fn proton_search_roots(steam_root: Option<&Path>) -> Vec<PathBuf> {
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

pub(crate) fn detect_proton_tools(steam_root: Option<&Path>) -> Vec<ProtonTool> {
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
pub(crate) enum GameKind {
    /// Windows executable run through a Proton layer.
    #[default]
    Windows,
    /// Native Linux executable, run directly.
    Linux,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct CustomGame {
    /// Stable identifier used by shortcuts, stats and running markers.
    #[serde(default)]
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) kind: GameKind,
    pub(crate) exe: String,
    #[serde(default)]
    pub(crate) args: String,
    #[serde(default)]
    pub(crate) workdir: String,
    /// Proton tool directory (Windows games only).
    #[serde(default)]
    pub(crate) proton_dir: String,
    /// Custom prefix path; empty = auto under the prefix base dir.
    #[serde(default)]
    pub(crate) prefix: String,
    #[serde(default)]
    pub(crate) flags: GameFlags,
    /// umu GAMEID; empty = `umu-<slug>`. A umu-database ID enables protonfixes.
    #[serde(default)]
    pub(crate) umu_id: String,
    /// Icon image path; empty = the exe's embedded icon.
    #[serde(default)]
    pub(crate) icon: String,
    /// Cover (600×900 poster) image path; empty = none.
    #[serde(default)]
    pub(crate) cover: String,
    /// Hero (wide banner) image path for the game page; empty = blurred cover.
    #[serde(default)]
    pub(crate) hero: String,
    /// SteamGridDB auto-fetch has already been tried for this game.
    #[serde(default)]
    pub(crate) art_checked: bool,
    /// Extra environment variables, one KEY=value per line.
    #[serde(default)]
    pub(crate) env: String,
    #[serde(default)]
    pub(crate) gamescope_opts: GamescopeOpts,
    /// Starred: listed first in the library.
    #[serde(default)]
    pub(crate) favorite: bool,
    #[serde(default)]
    pub(crate) status: GameStatus,
    /// Names of the collections the game is in (see `Settings::collections`).
    #[serde(default)]
    pub(crate) collections: Vec<String>,
    #[serde(default)]
    pub(crate) tuning: Tuning,
    /// Installed from Epic or GOG: launched with fresh online parameters and
    /// cloud saves synced around each session.
    #[serde(default)]
    pub(crate) store: Option<stores::StoreLink>,
    /// Unix time it was added (0 = before this was recorded).
    #[serde(default)]
    pub(crate) added: u64,
    /// A Windows launcher, or a game that starts through one.
    #[serde(default)]
    pub(crate) launcher: Option<launchers::LauncherLink>,
    /// A ROM, run by an emulator (shown on the Emulation page, not the Library).
    #[serde(default)]
    pub(crate) rom: Option<emulation::RomLink>,
}

/// Where a game stands: set by hand, except that launching an unsorted or
/// backlog game makes it Playing.
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GameStatus {
    #[default]
    None,
    Playing,
    Backlog,
    Finished,
    Dropped,
}

impl GameStatus {
    pub(crate) const ALL: [GameStatus; 4] = [GameStatus::Playing, GameStatus::Backlog, GameStatus::Finished, GameStatus::Dropped];

    pub(crate) fn label(self) -> &'static str {
        match self {
            GameStatus::None => "No status",
            GameStatus::Playing => "Playing",
            GameStatus::Backlog => "Backlog",
            GameStatus::Finished => "Finished",
            GameStatus::Dropped => "Dropped",
        }
    }

    pub(crate) fn color(self) -> egui::Color32 {
        match self {
            GameStatus::None => pal().faint,
            GameStatus::Playing => egui::Color32::from_rgb(0x6F, 0xD0, 0x8C),
            GameStatus::Backlog => egui::Color32::from_rgb(0xE6, 0xB5, 0x66),
            GameStatus::Finished => egui::Color32::from_rgb(0x6F, 0xB8, 0xFF),
            GameStatus::Dropped => pal().danger,
        }
    }

    /// The next one round the cycle (for a single button that steps through them).
    pub(crate) fn next(self) -> GameStatus {
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
pub(crate) struct Settings {
    /// Base directory for auto prefixes (empty = ~/.local/share/gamelo/prefixes).
    #[serde(default)]
    pub(crate) prefix_base: String,
    #[serde(default)]
    pub(crate) mangohud_bin: String,
    #[serde(default)]
    pub(crate) gamemoderun_bin: String,
    /// Path to umu-run (empty = search PATH).
    #[serde(default)]
    pub(crate) umu_bin: String,
    /// UI zoom factor (1.0 = default). 0 = unset.
    #[serde(default)]
    pub(crate) zoom: f32,
    /// Optional SteamGridDB API key, used to fetch game artwork.
    #[serde(default)]
    pub(crate) steamgriddb_key: String,
    /// Turn off animations.
    #[serde(default)]
    pub(crate) reduce_motion: bool,
    /// Let games use VR (Wine's OpenXR bridge). Off by default: on systems
    /// without a working OpenXR runtime the bridge can crash DXVK as a game
    /// starts (it probes OpenXR for extensions even in flat games).
    #[serde(default)]
    pub(crate) allow_openxr: bool,
    /// Don't navigate with a game controller.
    #[serde(default)]
    pub(crate) no_gamepad: bool,
    /// Where Epic and GOG games are installed (empty = ~/Games/Epic, ~/Games/GOG).
    #[serde(default)]
    pub(crate) epic_dir: String,
    #[serde(default)]
    pub(crate) gog_dir: String,
    /// The user's collections, in the order they're shown.
    #[serde(default)]
    pub(crate) collections: Vec<String>,
    /// Library layout: "grid" (default) or "list".
    #[serde(default)]
    pub(crate) library_view: String,
    /// Library tile size: "small", "medium" (default) or "large".
    #[serde(default)]
    pub(crate) tile_size: String,
    /// Library sort: "recent" (default), "name" or "playtime".
    #[serde(default)]
    pub(crate) library_sort: String,
    /// Don't download artwork automatically when a game is added.
    #[serde(default)]
    pub(crate) skip_auto_fetch: bool,
    /// First-run setup was finished or skipped.
    #[serde(default)]
    pub(crate) setup_done: bool,
    /// Folders scanned for ROMs (Emulation page).
    #[serde(default)]
    pub(crate) rom_dirs: Vec<String>,
    /// When the last crash the user dismissed happened (unix time).
    #[serde(default)]
    pub(crate) crash_seen: u64,
    /// Performance flags new games start with (None = MangoHud + FSR4 upgrade).
    #[serde(default)]
    pub(crate) new_flags: Option<GameFlags>,
    /// Proton build new games use (empty = the one the last added game uses).
    #[serde(default)]
    pub(crate) default_proton: String,
    /// "dark", "light" or "system" (empty = dark).
    #[serde(default)]
    pub(crate) theme_mode: String,
    /// Theme keys used in dark and in light mode (empty = the first of each).
    #[serde(default)]
    pub(crate) dark_theme: String,
    #[serde(default)]
    pub(crate) light_theme: String,
    /// Accent colour key replacing the theme's own (empty = the theme's).
    #[serde(default)]
    pub(crate) accent: String,
    #[serde(default)]
    pub(crate) square_corners: bool,
    /// Interface language: "en", "ar", or empty to follow the desktop locale.
    #[serde(default)]
    pub(crate) language: String,
    /// Folder each game's saves are mirrored to (Syncthing, Nextcloud, a
    /// shared drive…); empty = off.
    #[serde(default)]
    pub(crate) save_sync_dir: String,
    /// How many save backups to keep per game (0 = 10).
    #[serde(default)]
    pub(crate) save_keep: u32,
    #[serde(default)]
    pub(crate) save_no_auto_backup: bool,
    /// Don't look games up on HowLongToBeat.
    #[serde(default)]
    pub(crate) no_hltb: bool,
    /// Don't show Steam store info (description, screenshots…) on game pages.
    #[serde(default)]
    pub(crate) no_store: bool,
}

/// UI scale presets offered in Settings (Ctrl +/− and Ctrl+scroll can still land between them).
pub(crate) const ZOOM_STEPS: [f32; 8] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct AppConfig {
    #[serde(default)]
    pub(crate) custom_games: Vec<CustomGame>,
    #[serde(default)]
    pub(crate) settings: Settings,
}

pub(crate) fn config_path() -> PathBuf {
    home().join(".config/gamelo/config.json")
}

pub(crate) fn load_config() -> AppConfig {
    load_config_noting().0
}

/// The config, and — if it was damaged — the backup used and where the
/// damaged file was put.
pub(crate) fn load_config_noting() -> (AppConfig, Option<(Option<PathBuf>, PathBuf)>) {
    let (mut cfg, restored) = match safety::load(&config_path()) {
        safety::Loaded::Fine(c) => (c, None),
        safety::Loaded::Fresh => (AppConfig::default(), None),
        safety::Loaded::Restored(c, backup, aside) => (c, Some((backup, aside))),
    };
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
    (cfg, restored)
}

/// A short random hex id for a new game.
pub(crate) fn new_game_id() -> String {
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

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
