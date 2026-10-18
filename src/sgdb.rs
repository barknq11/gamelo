//! SteamGridDB artwork: search, thumbnails, downloads and the automatic cover fetch.

use crate::*;

// ---------------------------------------------------------------------------
// SteamGridDB artwork (optional — needs a free API key)
// ---------------------------------------------------------------------------

pub(crate) const SGDB_API: &str = "https://www.steamgriddb.com/api/v2";

/// Percent-encode a string for use as a single URL path segment.
pub(crate) fn url_encode(s: &str) -> String {
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

pub(crate) fn sgdb_get(path: &str, key: &str) -> Result<serde_json::Value, String> {
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
pub(crate) struct SgdbGame {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) year: Option<i64>,
    pub(crate) verified: bool,
}

/// One downloadable image in a SteamGridDB listing.
pub(crate) struct SgdbAsset {
    pub(crate) url: String,
    pub(crate) thumb: String,
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) style: String,
    pub(crate) author: String,
}

/// Which kind of artwork the browser shows.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ArtKind {
    #[default]
    Icon,
    Cover,
    Hero,
}

impl ArtKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ArtKind::Icon => "Icons",
            ArtKind::Cover => "Covers",
            ArtKind::Hero => "Heroes",
        }
    }

    pub(crate) fn noun_plural(self) -> &'static str {
        match self {
            ArtKind::Icon => "icons",
            ArtKind::Cover => "covers",
            ArtKind::Hero => "heroes",
        }
    }

    pub(crate) fn noun(self) -> &'static str {
        match self {
            ArtKind::Icon => "icon",
            ArtKind::Cover => "cover",
            ArtKind::Hero => "hero",
        }
    }

    /// Listing endpoint for a game: square icons, or static 600×900 covers.
    pub(crate) fn list_path(self, id: u64, page: u32) -> String {
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
pub(crate) fn year_of(ts: i64) -> i64 {
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

pub(crate) fn sgdb_search(query: &str, key: &str) -> Result<Vec<SgdbGame>, String> {
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
pub(crate) fn sgdb_assets(
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
pub(crate) fn thumb_cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".cache"))
        .join("gamelo/sgdb")
}

/// Download (or reuse a cached copy of) a thumbnail, scaled for the browser grid.
pub(crate) fn load_thumb(url: &str) -> Option<image::RgbaImage> {
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
pub(crate) fn download_art(url: &str, game_id: &str, kind: ArtKind) -> Result<PathBuf, String> {
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
pub(crate) fn best_match<'a>(name: &str, games: &'a [SgdbGame]) -> Option<&'a SgdbGame> {
    let n = name.trim().to_lowercase();
    games
        .iter()
        .find(|g| g.name.to_lowercase() == n)
        .or_else(|| games.iter().find(|g| g.verified))
        .or_else(|| games.first())
}

/// Artwork found by auto-fetch (None = SteamGridDB had none of that kind).
pub(crate) struct FetchedArt {
    pub(crate) cover: Option<PathBuf>,
    pub(crate) hero: Option<PathBuf>,
}

/// Look `name` up and download the top cover and/or hero for it.
pub(crate) fn auto_fetch_art(
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
pub(crate) fn spawn_thumb_workers(
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
    pub(crate) fn prefix_base(&self) -> PathBuf {
        if !self.prefix_base.is_empty() {
            PathBuf::from(&self.prefix_base)
        } else {
            data_dir().join("prefixes")
        }
    }

    pub(crate) fn mangohud_bin(&self) -> &str {
        if self.mangohud_bin.is_empty() {
            "mangohud"
        } else {
            &self.mangohud_bin
        }
    }

    pub(crate) fn gamemoderun_bin(&self) -> &str {
        if self.gamemoderun_bin.is_empty() {
            "gamemoderun"
        } else {
            &self.gamemoderun_bin
        }
    }

    pub(crate) fn umu_bin(&self) -> Option<PathBuf> {
        if !self.umu_bin.is_empty() {
            Some(PathBuf::from(&self.umu_bin))
        } else {
            find_umu()
        }
    }

    pub(crate) fn new_game_flags(&self) -> GameFlags {
        self.new_flags.unwrap_or(GameFlags {
            mangohud: true,
            fsr4: Fsr4Mode::Upgrade,
            ..Default::default()
        })
    }

    pub(crate) fn save_keep(&self) -> usize {
        if self.save_keep == 0 { 10 } else { self.save_keep as usize }
    }

    pub(crate) fn save_auto_backup(&self) -> bool {
        !self.save_no_auto_backup
    }

    /// UI zoom factor, clamped to a sane range (1.0 when unset).
    pub(crate) fn zoom(&self) -> f32 {
        if (0.5..=3.0).contains(&self.zoom) {
            self.zoom
        } else {
            1.0
        }
    }
}

pub(crate) fn game_prefix(g: &CustomGame, settings: &Settings) -> PathBuf {
    if !g.prefix.is_empty() {
        PathBuf::from(&g.prefix)
    } else {
        settings.prefix_base().join(slugify(&g.name))
    }
}

/// Auto prefixes are named after the game, so a rename would strand the old one
/// (and its saves). Pin the renamed game to it instead. Returns true if pinned.
pub(crate) fn keep_prefix_on_rename(old: &CustomGame, new: &mut CustomGame, settings: &Settings) -> bool {
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
pub(crate) fn game_umu_id(g: &CustomGame) -> String {
    let id = g.umu_id.trim();
    if id.is_empty() {
        format!("umu-{}", slugify(&g.name))
    } else {
        id.to_string()
    }
}

pub(crate) fn game_workdir(g: &CustomGame) -> Option<PathBuf> {
    if !g.workdir.is_empty() {
        Some(PathBuf::from(&g.workdir))
    } else {
        Path::new(&g.exe).parent().map(|p| p.to_path_buf())
    }
}

/// A fully resolved launch: program + args + environment + working dir.
pub(crate) struct Launch {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) envs: Vec<(String, String)>,
    pub(crate) cwd: Option<PathBuf>,
}

pub(crate) fn build_launch(g: &CustomGame, settings: &Settings) -> Result<Launch, String> {
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

pub(crate) fn shell_quote(s: &str) -> String {
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
pub(crate) fn ensure_prefix(g: &CustomGame, settings: &Settings) -> Result<PathBuf, String> {
    let prefix = game_prefix(g, settings);
    if let Some(parent) = prefix.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(prefix)
}

/// Build the common umu-run environment (GAMEID/PROTONPATH/WINEPREFIX) for a Windows game.
pub(crate) fn umu_env(
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
pub(crate) fn launch_game(
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
pub(crate) fn init_prefix(g: &CustomGame, settings: &Settings) -> Result<Child, String> {
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
pub(crate) fn run_winecfg(g: &CustomGame, settings: &Settings) -> Result<Child, String> {
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
pub(crate) fn launch_command_string(
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
pub(crate) fn desktop_exec_arg(arg: &str) -> String {
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
pub(crate) fn write_desktop_entry(g: &CustomGame, settings: &Settings) -> Result<PathBuf, String> {
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
pub(crate) fn guess_game_name(exe: &Path) -> String {
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

pub(crate) fn pick_executable() -> Option<PathBuf> {
    pick_file(lg("Select game executable"), lg("Executables"), &["*.exe", "*.EXE"])
}

pub(crate) fn pick_image() -> Option<PathBuf> {
    pick_file(
        "Select artwork image",
        "Images",
        &["*.png", "*.jpg", "*.jpeg", "*.ico", "*.PNG", "*.JPG", "*.JPEG", "*.ICO"],
    )
}

pub(crate) fn pick_any_file(title: &str) -> Option<PathBuf> {
    pick_file(title, lg("All files"), &["*"])
}

/// Open a native folder chooser and return the selected folder.
pub(crate) fn pick_folder(title: &str) -> Option<PathBuf> {
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
pub(crate) fn run_picker(candidates: Vec<(String, Vec<String>, bool)>) -> Option<PathBuf> {
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
pub(crate) fn pick_file(title: &str, filter_name: &str, patterns: &[&str]) -> Option<PathBuf> {
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
pub(crate) fn xdg_open(target: impl AsRef<std::ffi::OsStr>) -> Option<Child> {
    Command::new("xdg-open").arg(target).spawn().ok()
}
