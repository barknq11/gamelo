//! Playtime stats and running games: the headless runner, its marker files, stopping.

use crate::*;

// ---------------------------------------------------------------------------
// Playtime stats (separate from the config so headless runs never clobber it)
// ---------------------------------------------------------------------------

#[derive(Clone, Default, Serialize, Deserialize)]
pub(crate) struct GameStats {
    #[serde(default)]
    pub(crate) playtime_secs: u64,
    /// Unix time the last session started.
    #[serde(default)]
    pub(crate) last_played: u64,
    #[serde(default)]
    pub(crate) last_session_secs: u64,
    /// Exit code of the last session (None = killed by a signal).
    #[serde(default)]
    pub(crate) last_exit: Option<i32>,
    /// Sessions counted since this field was added.
    #[serde(default)]
    pub(crate) sessions: u32,
}

pub(crate) type Stats = HashMap<String, GameStats>;

pub(crate) fn load_stats_in(dir: &Path) -> Stats {
    fs::read_to_string(dir.join("stats.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub(crate) fn load_stats() -> Stats {
    load_stats_in(&data_dir())
}

/// Add a finished session to `dir`/stats.json. Locked, so two games ending at
/// once can't overwrite each other's update.
pub(crate) fn record_session_in(dir: &Path, id: &str, started: u64, secs: u64, exit: Option<i32>) {
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
pub(crate) fn dir_size(dir: &Path) -> u64 {
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
pub(crate) fn format_bytes(n: u64) -> String {
    match n {
        0..1_000_000 => format!("{} KB", n.div_ceil(1000)),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}

/// "12h 40m", "25m", "40s".
pub(crate) fn format_duration(secs: u64) -> String {
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
pub(crate) fn format_ago(then: u64, now: u64) -> String {
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
pub(crate) struct RunMarker {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) runner_pid: u32,
    pub(crate) game_pid: u32,
    pub(crate) started: u64,
    pub(crate) log: String,
}

pub(crate) fn running_dir() -> PathBuf {
    data_dir().join("running")
}

pub(crate) fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Live running markers by game id; markers whose runner is gone are removed.
pub(crate) fn scan_running() -> HashMap<String, RunMarker> {
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
pub(crate) fn process_tree(root: u32) -> Vec<u32> {
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
pub(crate) fn stop_game(marker: &RunMarker, game: Option<&CustomGame>, settings: &Settings) -> Option<Child> {
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
pub(crate) fn self_exe() -> PathBuf {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("gamelo"))
}

/// Start `gamelo --run <id>` detached, so the game (and its playtime
/// tracking) outlives the window.
pub(crate) fn spawn_runner(id: &str) -> Result<Child, String> {
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
pub(crate) fn run_headless(id: &str) -> i32 {
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
    // Through a launcher, the command returns before the game ends.
    if let Some(link) = &g.launcher {
        launchers::wait_for_game(link, &cfg.settings);
    }
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
pub(crate) fn notify(title: &str, body: &str) {
    if find_in_path("notify-send").is_some() {
        let _ = Command::new("notify-send")
            .args(["-a", "Gamelo", "-i", "dialog-warning", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

pub(crate) fn save_config(cfg: &AppConfig) {
    let p = config_path();
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    safety::daily_backup(&p);
    // Private: the config may hold the SteamGridDB API key.
    if let Err(e) = safety::write_atomic(&p, &serde_json::to_string_pretty(cfg).unwrap_or_default(), true) {
        eprintln!("gamelo: config save failed: {e}");
    }
}

pub(crate) fn slugify(s: &str) -> String {
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

pub(crate) fn data_dir() -> PathBuf {
    home().join(".local/share/gamelo")
}
