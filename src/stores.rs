//! Epic Games and GOG: sign in, list owned games, install, update, launch and
//! sync cloud saves. The work is done by two open-source command-line tools
//! (Heroic's builds of legendary for Epic and gogdl for GOG), downloaded on
//! demand and checked against GitHub's published checksums. Games are still
//! launched by ProtonLaunch through umu + Proton like any other game.
//!
//! Everything here blocks: call it from a worker thread.

use crate::i18n::{fmt_logical as lfmt, logical as lg};
use crate::*;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Epic,
    Gog,
}

impl Store {
    pub fn name(self) -> &'static str {
        match self {
            Store::Epic => "Epic Games",
            Store::Gog => "GOG",
        }
    }

    fn tool(self) -> &'static str {
        match self {
            Store::Epic => "legendary",
            Store::Gog => "gogdl",
        }
    }

    fn repo(self) -> &'static str {
        match self {
            Store::Epic => "Heroic-Games-Launcher/legendary",
            Store::Gog => "Heroic-Games-Launcher/heroic-gogdl",
        }
    }

    /// The page that signs you in and shows the code to paste back.
    pub fn login_url(self) -> &'static str {
        match self {
            Store::Epic => "https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a%26responseType%3Dcode",
            Store::Gog => "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=galaxy",
        }
    }
}

/// A game from a store, linked to a library entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct StoreLink {
    pub store: Store,
    /// Epic's app name or GOG's product id.
    pub app: String,
    /// The install folder.
    #[serde(default)]
    pub path: String,
}

/// An owned game as the store lists it.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Owned {
    pub app: String,
    pub title: String,
    /// Landscape image for the store list.
    #[serde(default)]
    pub thumb: String,
    /// Portrait cover and wide banner for the library.
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub hero: String,
    /// Latest build (Epic), to spot updates.
    #[serde(default)]
    pub version: String,
    /// Epic's cloud save folder pattern ("{AppData}/…"), empty = no cloud saves.
    #[serde(default)]
    pub cloud_saves: String,
}

/// An installed store game.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct Installed {
    pub app: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub version: String,
}

/// What the store knows before an install: sizes and the folder it will use.
#[derive(Clone, Debug, Default)]
pub struct SizeInfo {
    pub download: u64,
    pub disk: u64,
    pub folder: String,
}

/// How to start an installed game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchInfo {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub workdir: Option<PathBuf>,
}

/// Our own records: GOG installs (gogdl keeps none), names and save timestamps.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Records {
    #[serde(default)]
    pub gog_user: String,
    #[serde(default)]
    pub gog_installed: HashMap<String, Installed>,
    /// GOG cloud save sync times, by "<product id>/<location name>".
    #[serde(default)]
    pub gog_save_ts: HashMap<String, String>,
}

fn records_path() -> PathBuf {
    data_dir().join("stores.json")
}

pub fn load_records() -> Records {
    fs::read_to_string(records_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_records(r: &Records) {
    let _ = fs::create_dir_all(data_dir());
    let tmp = records_path().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(r).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, records_path());
    }
}

fn tools_dir() -> PathBuf {
    data_dir().join("tools")
}

fn gog_auth_path() -> PathBuf {
    data_dir().join("gog-auth.json")
}

/// legendary's config folder (shared with a standalone legendary install).
fn legendary_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().join(".config"));
    base.join("legendary")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// The tool to use: ours if downloaded, else a system legendary.
pub fn tool_path(store: Store) -> Option<PathBuf> {
    let ours = tools_dir().join(store.tool());
    if ours.is_file() {
        return Some(ours);
    }
    (store == Store::Epic).then(|| find_in_path("legendary")).flatten()
}

/// Download the store's tool from its latest GitHub release, checking the
/// SHA-256 GitHub publishes for it.
pub fn install_tool(store: Store) -> Result<String, String> {
    let api = format!("https://api.github.com/repos/{}/releases/latest", store.repo());
    let v: serde_json::Value = serde_json::from_str(&http_fetch(&api, None)?).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with("linux_x86_64"))))
        .ok_or_else(|| lfmt("No Linux build of {} found", &[&store.tool()]))?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default();
    let digest = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_lowercase);
    fs::create_dir_all(tools_dir()).map_err(|e| e.to_string())?;
    let dest = tools_dir().join(store.tool());
    let tmp = dest.with_extension("part");
    http_fetch(url, Some(&tmp))?;
    if let Some(want) = digest {
        let out = Command::new("sha256sum").arg(&tmp).output().map_err(|e| format!("sha256sum: {e}"))?;
        let got = String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or_default().to_lowercase();
        if got != want {
            let _ = fs::remove_file(&tmp);
            return Err(lg("The download is corrupted (checksum mismatch) — try again").to_string());
        }
    }
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    Ok(lfmt("Installed {} {}", &[&store.tool(), &tag]))
}

/// A command for the store's tool.
fn cmd(store: Store) -> Result<Command, String> {
    let tool = tool_path(store).ok_or_else(|| lfmt("{} isn't installed yet", &[&store.tool()]))?;
    let mut c = Command::new(tool);
    if store == Store::Gog {
        c.arg("--auth-config-path").arg(gog_auth_path());
    }
    c.stdin(Stdio::null());
    Ok(c)
}

/// Run the tool; its stdout on success, the end of its output otherwise.
fn run(store: Store, args: &[&str]) -> Result<String, String> {
    let out = cmd(store)?.args(args).output().map_err(|e| format!("{}: {e}", store.tool()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let last = stderr.lines().chain(stdout.lines()).rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    Err(format!("{}: {last}", store.tool()))
}

/// The JSON value in a tool's output (they print log lines around it).
fn json_in(text: &str) -> Result<serde_json::Value, String> {
    let start = text.find(['{', '[']).ok_or_else(|| lg("The store sent something unexpected").to_string())?;
    serde_json::from_str(&text[start..]).map_err(|e| format!("{}: {e}", lg("The store sent something unexpected")))
}

/// The authorization code in whatever was pasted: the bare code, Epic's JSON
/// page, or GOG's redirect address.
pub fn extract_code(store: Store, pasted: &str) -> Option<String> {
    let s = pasted.trim();
    let code = match store {
        Store::Epic if s.contains("authorizationCode") => {
            let rest = &s[s.find("authorizationCode")? + "authorizationCode".len()..];
            rest.split('"').find(|p| p.len() >= 16 && p.chars().all(|c| c.is_ascii_alphanumeric()))?.to_string()
        }
        Store::Gog if s.contains("code=") => s[s.find("code=")? + 5..].split(['&', '#', ' ']).next()?.to_string(),
        _ => s.to_string(),
    };
    (code.len() >= 16 && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some(code)
}

/// Sign in with a pasted code. Returns the account name.
pub fn login(store: Store, code: &str) -> Result<String, String> {
    match store {
        Store::Epic => {
            run(store, &["auth", "--code", code, "--disable-webview"])?;
            account(store).ok_or_else(|| lg("Signing in didn't work — get a fresh code and try again").to_string())
        }
        Store::Gog => {
            run(store, &["auth", "--code", code])?;
            let token = gog_token()?;
            let user = http_fetch_with("https://embed.gog.com/userData.json", None, &[("Authorization", &format!("Bearer {token}"))])
                .ok()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                .and_then(|v| v["username"].as_str().map(String::from))
                .unwrap_or_else(|| "GOG".into());
            let mut r = load_records();
            r.gog_user = user.clone();
            save_records(&r);
            Ok(user)
        }
    }
}

pub fn logout(store: Store) -> Result<(), String> {
    match store {
        Store::Epic => run(store, &["auth", "--delete"]).map(|_| ()),
        Store::Gog => {
            let _ = fs::remove_file(gog_auth_path());
            let mut r = load_records();
            r.gog_user.clear();
            save_records(&r);
            Ok(())
        }
    }
}

/// The signed-in account name, if any (read from disk, no network).
pub fn account(store: Store) -> Option<String> {
    match store {
        Store::Epic => {
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(legendary_dir().join("user.json")).ok()?).ok()?;
            v["displayName"].as_str().map(String::from)
        }
        Store::Gog => {
            gog_auth_path().is_file().then(|| load_records().gog_user).map(|u| if u.is_empty() { "GOG".into() } else { u })
        }
    }
}

/// A current GOG access token (gogdl refreshes it when it has expired).
fn gog_token() -> Result<String, String> {
    let v = json_in(&run(Store::Gog, &["auth"])?)?;
    v["access_token"].as_str().map(String::from).ok_or_else(|| lg("Sign in to GOG again").to_string())
}

/// Every game the account owns (Windows builds).
pub fn library(store: Store) -> Result<Vec<Owned>, String> {
    match store {
        Store::Epic => {
            let v = json_in(&run(store, &["list", "--json"])?)?;
            Ok(v.as_array().map(|a| a.iter().map(epic_owned).collect()).unwrap_or_default())
        }
        Store::Gog => {
            let token = gog_token()?;
            let auth = format!("Bearer {token}");
            let mut out = Vec::new();
            let mut page = 1;
            loop {
                let url = format!("https://embed.gog.com/account/getFilteredProducts?mediaType=1&page={page}");
                let body = http_fetch_with(&url, None, &[("Authorization", &auth)])?;
                let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
                for p in v["products"].as_array().into_iter().flatten() {
                    if p["worksOn"]["Windows"].as_bool() == Some(false) {
                        continue;
                    }
                    let image = p["image"].as_str().unwrap_or_default();
                    out.push(Owned {
                        app: p["id"].as_u64().map(|i| i.to_string()).unwrap_or_default(),
                        title: p["title"].as_str().unwrap_or_default().to_string(),
                        thumb: if image.is_empty() { String::new() } else { format!("https:{image}_196.jpg") },
                        ..Default::default()
                    });
                }
                let pages = v["totalPages"].as_u64().unwrap_or(1);
                if page >= pages {
                    break;
                }
                page += 1;
            }
            out.sort_by_key(|o| o.title.to_lowercase());
            Ok(out)
        }
    }
}

/// One entry of `legendary list --json`.
fn epic_owned(g: &serde_json::Value) -> Owned {
    let images = g["metadata"]["keyImages"].as_array().cloned().unwrap_or_default();
    let image = |kind: &str| {
        images.iter().find(|i| i["type"] == kind).and_then(|i| i["url"].as_str()).unwrap_or_default().to_string()
    };
    Owned {
        app: g["app_name"].as_str().unwrap_or_default().to_string(),
        title: g["app_title"].as_str().unwrap_or_default().to_string(),
        thumb: image("DieselGameBox"),
        cover: image("DieselGameBoxTall"),
        hero: image("DieselGameBox"),
        version: g["asset_infos"]["Windows"]["build_version"].as_str().unwrap_or_default().to_string(),
        cloud_saves: g["metadata"]["customAttributes"]["CloudSaveFolder"]["value"].as_str().unwrap_or_default().to_string(),
    }
}

/// Installed games.
pub fn installed(store: Store) -> Result<Vec<Installed>, String> {
    match store {
        Store::Epic => {
            let v = json_in(&run(store, &["list-installed", "--json"])?)?;
            Ok(v.as_array()
                .map(|a| {
                    a.iter()
                        .filter(|g| !g["is_dlc"].as_bool().unwrap_or(false))
                        .map(|g| Installed {
                            app: g["app_name"].as_str().unwrap_or_default().to_string(),
                            title: g["title"].as_str().unwrap_or_default().to_string(),
                            path: g["install_path"].as_str().unwrap_or_default().to_string(),
                            version: g["version"].as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default())
        }
        Store::Gog => Ok(load_records().gog_installed.into_values().collect()),
    }
}

/// Download and disk size, and the folder an install will create.
pub fn size_info(store: Store, app: &str) -> Result<SizeInfo, String> {
    match store {
        Store::Epic => {
            let v = json_in(&run(store, &["info", app, "--json"])?)?;
            Ok(SizeInfo {
                download: v["manifest"]["download_size"].as_u64().unwrap_or(0),
                disk: v["manifest"]["disk_size"].as_u64().unwrap_or(0),
                folder: v["game"]["metadata"]["customAttributes"]["FolderName"]["value"]
                    .as_str()
                    .or(v["manifest"]["install_path"].as_str())
                    .unwrap_or(app)
                    .to_string(),
            })
        }
        Store::Gog => {
            let v = json_in(&run(store, &["info", app, "--os", "windows"])?)?;
            let size = &v["size"];
            let lang = gog_lang(&v);
            let part = |k: &str, f: &str| size[k][f].as_u64().unwrap_or(0);
            Ok(SizeInfo {
                download: part("*", "download_size") + part(&lang, "download_size"),
                disk: part("*", "disk_size") + part(&lang, "disk_size"),
                folder: v["folder_name"].as_str().unwrap_or(app).to_string(),
            })
        }
    }
}

/// English when the game has it, else its first language.
fn gog_lang(info: &serde_json::Value) -> String {
    let langs: Vec<&str> = info["languages"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect()).unwrap_or_default();
    if langs.contains(&"en-US") || langs.is_empty() { "en-US".into() } else { langs[0].to_string() }
}

/// The percentage in legendary's ("= Progress: 12.34% (…)") and gogdl's
/// ("= Progress: 12.34 120/970, …") log lines, as a fraction.
pub fn parse_progress(line: &str) -> Option<f32> {
    let rest = line[line.find("Progress:")? + "Progress:".len()..].trim_start();
    let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let pct: f32 = num.parse().ok()?;
    let after = &rest[num.len()..];
    // gogdl: the number is followed by a "done/total" count.
    let counted = after.strip_prefix(' ').is_some_and(|t| t.split_whitespace().next().is_some_and(|w| w.contains('/')));
    ((after.starts_with('%') || counted) && (0.0..=100.0).contains(&pct)).then_some(pct / 100.0)
}

/// Install (or update) a game under `base`, reporting progress in ten-thousandths.
pub fn install(store: Store, app: &str, base: &Path, progress: Arc<AtomicU32>) -> Result<Installed, String> {
    fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let base_s = base.to_string_lossy().to_string();
    let info = size_info(store, app).unwrap_or_default();
    let mut c = cmd(store)?;
    match store {
        Store::Epic => {
            c.args(["install", app, "--base-path", &base_s, "-y", "--skip-sdl", "--skip-dlcs", "--platform", "Windows"]);
        }
        Store::Gog => {
            let v = json_in(&run(store, &["info", app, "--os", "windows"])?)?;
            let lang = gog_lang(&v);
            c.args(["download", app, "--platform", "windows", "--path", &base_s, "--lang", &lang, "--skip-dlcs"]);
        }
    }
    let mut child = c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{}: {e}", store.tool()))?;
    // Both tools log progress to stderr; keep the last lines for errors.
    let stderr = child.stderr.take().ok_or("no stderr")?;
    let p = progress.clone();
    let folder = base.join(&info.folder);
    let reader = std::thread::spawn(move || {
        let mut tail: Vec<String> = Vec::new();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(f) = parse_progress(&line) {
                p.store((f * 10_000.0) as u32, Ordering::Relaxed);
            }
            tail.push(line);
            if tail.len() > 12 {
                tail.remove(0);
            }
        }
        tail
    });
    // Without progress lines, the folder's size against the expected size.
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if info.disk > 0 && progress.load(Ordering::Relaxed) == 0 && folder.is_dir() {
            let f = (dir_size(&folder) as f64 / info.disk as f64).min(0.99);
            progress.store((f * 10_000.0) as u32, Ordering::Relaxed);
        }
    }
    let ok = child.wait().map_err(|e| e.to_string())?.success();
    let tail = reader.join().unwrap_or_default();
    if !ok {
        let last = tail.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default();
        return Err(format!("{}: {}", store.tool(), last.trim()));
    }
    progress.store(10_000, Ordering::Relaxed);
    match store {
        Store::Epic => installed(store)?
            .into_iter()
            .find(|g| g.app == app)
            .ok_or_else(|| lg("The install finished but the game isn't listed as installed").to_string()),
        Store::Gog => {
            let path = gog_install_dir(base, app, &info.folder).ok_or_else(|| lg("The install finished but its folder wasn't found").to_string())?;
            let title = gog_info_file(&path, app).and_then(|v| v["name"].as_str().map(String::from)).unwrap_or_else(|| info.folder.clone());
            let g = Installed { app: app.to_string(), title, path: path.to_string_lossy().into(), version: String::new() };
            let mut r = load_records();
            r.gog_installed.insert(app.to_string(), g.clone());
            save_records(&r);
            Ok(g)
        }
    }
}

/// The folder holding goggame-<id>.info under `base` (usually `base/folder`).
fn gog_install_dir(base: &Path, id: &str, folder: &str) -> Option<PathBuf> {
    let direct = base.join(folder);
    if direct.join(format!("goggame-{id}.info")).is_file() {
        return Some(direct);
    }
    fs::read_dir(base).ok()?.flatten().map(|e| e.path()).find(|p| p.join(format!("goggame-{id}.info")).is_file())
}

fn gog_info_file(dir: &Path, id: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(dir.join(format!("goggame-{id}.info"))).ok()?).ok()
}

/// How to start an installed game (its main executable, arguments and folder).
pub fn launch_info(store: Store, app: &str, path: &Path) -> Result<LaunchInfo, String> {
    match store {
        Store::Epic => {
            let v = json_in(&run(store, &["launch", app, "--json", "--offline", "--skip-version-check"])?)?;
            epic_launch_info(&v)
        }
        Store::Gog => gog_launch_info(&gog_info_file(path, app).ok_or_else(|| lg("The game's GOG info file is missing").to_string())?, path),
    }
}

fn epic_launch_info(v: &serde_json::Value) -> Result<LaunchInfo, String> {
    let dir = PathBuf::from(v["game_directory"].as_str().unwrap_or_default());
    let exe = v["game_executable"].as_str().filter(|e| !e.is_empty()).ok_or_else(|| lg("The store didn't say which program starts the game").to_string())?;
    let strings = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
    Ok(LaunchInfo {
        exe: dir.join(exe),
        args: strings("game_parameters").into_iter().chain(strings("user_parameters")).collect(),
        workdir: v["working_directory"].as_str().filter(|w| !w.is_empty()).map(PathBuf::from).or(Some(dir)),
    })
}

fn gog_launch_info(v: &serde_json::Value, dir: &Path) -> Result<LaunchInfo, String> {
    let tasks = v["playTasks"].as_array().cloned().unwrap_or_default();
    let task = tasks
        .iter()
        .find(|t| t["isPrimary"].as_bool() == Some(true) && t["type"] == "FileTask")
        .or_else(|| tasks.iter().find(|t| t["type"] == "FileTask"))
        .ok_or_else(|| lg("The store didn't say which program starts the game").to_string())?;
    let rel = task["path"].as_str().unwrap_or_default().replace('\\', "/");
    let work = task["workingDir"].as_str().unwrap_or_default().replace('\\', "/");
    Ok(LaunchInfo {
        exe: dir.join(rel),
        args: tokenize(task["arguments"].as_str().unwrap_or_default()),
        workdir: Some(if work.is_empty() { dir.to_path_buf() } else { dir.join(work) }),
    })
}

/// Fresh online parameters for an Epic game (some games need them to start;
/// they expire, so they're fetched for every launch). Empty when offline.
pub fn epic_online_args(app: &str) -> Vec<String> {
    run(Store::Epic, &["launch", app, "--json", "--skip-version-check"])
        .ok()
        .and_then(|out| json_in(&out).ok())
        .and_then(|v| v["egl_parameters"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()))
        .unwrap_or_default()
}

pub fn uninstall(store: Store, app: &str, path: &Path) -> Result<(), String> {
    match store {
        Store::Epic => run(store, &["uninstall", app, "-y", "--skip-uninstaller"]).map(|_| ()),
        Store::Gog => {
            if path.join(format!("goggame-{app}.info")).is_file() {
                fs::remove_dir_all(path).map_err(|e| e.to_string())?;
            }
            let mut r = load_records();
            r.gog_installed.remove(app);
            save_records(&r);
            Ok(())
        }
    }
}

/// A Windows save location with store placeholders, inside a Proton prefix.
pub fn resolve_save_path(pattern: &str, prefix: &Path, install: &Path) -> PathBuf {
    let user = prefix.join("drive_c/users/steamuser");
    let map: [(&str, PathBuf); 13] = [
        ("{appdata}", user.join("AppData/Local")),
        ("{userdir}", user.join("Documents")),
        ("{userprofile}", user.clone()),
        ("{usersavedgames}", user.join("Saved Games")),
        ("{installdir}", install.to_path_buf()),
        ("<?install?>", install.to_path_buf()),
        ("<?saved_games?>", user.join("Saved Games")),
        ("<?application_data_local?>", user.join("AppData/Local")),
        ("<?application_data_local_low?>", user.join("AppData/LocalLow")),
        ("<?application_data_roaming?>", user.join("AppData/Roaming")),
        ("<?documents?>", user.join("Documents")),
        ("<?local_appdata?>", user.join("AppData/Local")),
        ("<?roaming_appdata?>", user.join("AppData/Roaming")),
    ];
    let p = pattern.replace('\\', "/");
    let lower = p.to_lowercase();
    for (key, dir) in &map {
        if let Some(at) = lower.find(key) {
            let rest = p[at + key.len()..].trim_start_matches('/');
            return dir.join(rest);
        }
    }
    PathBuf::from(p)
}

/// Sync a game's cloud saves: `upload` after playing, download before.
/// Failures are logged and never stop a launch.
pub fn sync_saves(link: &StoreLink, prefix: &Path, install: &Path, upload: bool) -> Result<(), String> {
    match link.store {
        Store::Epic => {
            let meta: serde_json::Value = fs::read_to_string(legendary_dir().join("metadata").join(format!("{}.json", link.app)))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let pattern = meta["metadata"]["customAttributes"]["CloudSaveFolder"]["value"].as_str().unwrap_or_default();
            if pattern.is_empty() {
                return Ok(());
            }
            let path = resolve_save_path(pattern, prefix, install);
            let _ = fs::create_dir_all(&path);
            let p = path.to_string_lossy().to_string();
            let dir = if upload { "--skip-download" } else { "--skip-upload" };
            run(Store::Epic, &["sync-saves", &link.app, "--save-path", &p, dir, "-y"]).map(|_| ())
        }
        Store::Gog => {
            let info = gog_info_file(install, &link.app).ok_or("no GOG info file")?;
            let client = info["clientId"].as_str().ok_or("no GOG client id")?;
            let url = format!("https://remote-config.gog.com/components/galaxy_client/clients/{client}?component_version=2.0.45");
            let cfg: serde_json::Value = serde_json::from_str(&http_fetch(&url, None)?).map_err(|e| e.to_string())?;
            let storage = &cfg["content"]["Windows"]["cloudStorage"];
            if !storage["enabled"].as_bool().unwrap_or(false) {
                return Ok(());
            }
            let mut records = load_records();
            for loc in storage["locations"].as_array().into_iter().flatten() {
                let name = loc["name"].as_str().unwrap_or("__default");
                let path = resolve_save_path(loc["location"].as_str().unwrap_or_default(), prefix, install);
                let _ = fs::create_dir_all(&path);
                let key = format!("{}/{name}", link.app);
                let ts = records.gog_save_ts.get(&key).cloned().unwrap_or_else(|| "0".into());
                let p = path.to_string_lossy().to_string();
                let dir = if upload { "--skip-download" } else { "--skip-upload" };
                let out = run(Store::Gog, &["save-sync", &p, client, "--os", "windows", "--ts", &ts, "--name", name, dir])?;
                // gogdl prints the new sync time last.
                if let Some(t) = out.lines().rev().map(str::trim).find(|l| l.parse::<f64>().is_ok()) {
                    records.gog_save_ts.insert(key, t.to_string());
                }
            }
            save_records(&records);
            Ok(())
        }
    }
}

/// Games installed by Heroic (Epic and GOG), with Heroic's prefix for each
/// when it has one: (store, installed game, prefix).
pub fn heroic_installs() -> Vec<(Store, Installed, Option<PathBuf>)> {
    let heroic = home_dir().join(".config/heroic");
    let mut out = Vec::new();
    let prefix_of = |app: &str| -> Option<PathBuf> {
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(heroic.join("GamesConfig").join(format!("{app}.json"))).ok()?).ok()?;
        v[app]["winePrefix"].as_str().map(PathBuf::from).filter(|p| p.is_dir())
    };
    if let Some(v) = fs::read_to_string(heroic.join("legendaryConfig/legendary/installed.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        && let Some(map) = v.as_object()
    {
        for (app, g) in map {
            if g["is_dlc"].as_bool().unwrap_or(false) {
                continue;
            }
            let i = Installed {
                app: app.clone(),
                title: g["title"].as_str().unwrap_or(app).to_string(),
                path: g["install_path"].as_str().unwrap_or_default().to_string(),
                version: g["version"].as_str().unwrap_or_default().to_string(),
            };
            out.push((Store::Epic, i, prefix_of(app)));
        }
    }
    if let Some(v) = fs::read_to_string(heroic.join("gog_store/installed.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        for g in v["installed"].as_array().into_iter().flatten() {
            let app = g["appName"].as_str().unwrap_or_default().to_string();
            if g["platform"].as_str().is_some_and(|p| p != "windows") || app.is_empty() {
                continue;
            }
            let path = g["install_path"].as_str().unwrap_or_default().to_string();
            let title = gog_info_file(Path::new(&path), &app).and_then(|v| v["name"].as_str().map(String::from)).unwrap_or_else(|| app.clone());
            let prefix = prefix_of(&app);
            out.push((Store::Gog, Installed { app, title, path, version: String::new() }, prefix));
        }
    }
    out.retain(|(_, i, _)| Path::new(&i.path).is_dir());
    out
}

/// Bring a game installed elsewhere into our records (Epic: legendary import).
pub fn import(store: Store, g: &Installed) -> Result<(), String> {
    match store {
        Store::Epic => {
            if installed(store).is_ok_and(|list| list.iter().any(|i| i.app == g.app)) {
                return Ok(());
            }
            run(store, &["import", &g.app, &g.path, "--skip-dlcs", "--platform", "Windows"]).map(|_| ())
        }
        Store::Gog => {
            let mut r = load_records();
            r.gog_installed.insert(g.app.clone(), g.clone());
            save_records(&r);
            Ok(())
        }
    }
}

/// GOG's portrait cover and wide banner for a product (public API).
pub fn gog_art(id: &str) -> (String, String) {
    let v: serde_json::Value = http_fetch(&format!("https://api.gog.com/v2/games/{id}?locale=en-US"), None)
        .ok()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let href = |k: &str| v["_links"][k]["href"].as_str().unwrap_or_default().to_string();
    (href("boxArtImage"), href("backgroundImage"))
}

/// Free bytes on the disk holding `path` (or its nearest existing parent).
pub fn free_space(path: &Path) -> Option<u64> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct we pass for a valid, NUL-terminated path.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c.as_ptr(), &mut st) == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
    }
}

/// Download a store image to the artwork folder. Returns its path.
pub fn save_art(url: &str, name: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    let dir = data_dir().join("art");
    fs::create_dir_all(&dir).ok()?;
    let ext = if url.to_lowercase().contains(".png") { "png" } else { "jpg" };
    let path = dir.join(format!("{name}.{ext}"));
    http_fetch(url, Some(&path)).ok()?;
    Some(path.to_string_lossy().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads both tools (into a temporary HOME) and asks GOG for a game's
    /// size, which needs no account:
    /// cargo test -- --ignored stores_live --nocapture
    #[test]
    #[ignore]
    fn stores_live() {
        let home = std::env::temp_dir().join(format!("plaunch-stores-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(&home).unwrap();
        // SAFETY: ignored test, run on its own.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        }
        for store in [Store::Epic, Store::Gog] {
            eprintln!("{}", install_tool(store).unwrap());
            assert!(tools_dir().join(store.tool()).is_file());
        }
        assert!(account(Store::Epic).is_none() && account(Store::Gog).is_none());
        let info = size_info(Store::Gog, "1207658924").unwrap();
        eprintln!("{info:?}");
        assert!(info.disk > 1_000_000_000 && info.folder.contains("Witcher"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn codes_are_found_in_what_people_paste() {
        let epic = r#"{"warning":"Do not share this code with any 3rd party service.","redirectUrl":"https://localhost/launcher/authorized?code=0123456789abcdef0123456789abcdef","authorizationCode":"0123456789abcdef0123456789abcdef","exchangeCode":null,"sid":null}"#;
        assert_eq!(extract_code(Store::Epic, epic).as_deref(), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(extract_code(Store::Epic, "  0123456789abcdef0123456789abcdef \n").as_deref(), Some("0123456789abcdef0123456789abcdef"));
        let gog = "https://embed.gog.com/on_login_success?origin=client&code=AbCdEf-123_ghijklmnopqrstuvwxyz";
        assert_eq!(extract_code(Store::Gog, gog).as_deref(), Some("AbCdEf-123_ghijklmnopqrstuvwxyz"));
        assert_eq!(extract_code(Store::Epic, "hello"), None);
    }

    #[test]
    fn progress_lines_and_save_paths() {
        assert_eq!(parse_progress("[DLManager] INFO: = Progress: 42.50% (850/2000), Running for 00:01:02, ETA: 00:01:30"), Some(0.425));
        assert_eq!(parse_progress("[PROGRESS] INFO: = Progress: 7.00 3/40, Running for: 00:00:02"), Some(0.07));
        assert_eq!(parse_progress("Progress: 12 apples"), None);
        assert_eq!(parse_progress("nothing here"), None);
        let pfx = Path::new("/p");
        assert_eq!(resolve_save_path("{AppData}/Game/Saved/SaveGames", pfx, Path::new("/g")), PathBuf::from("/p/drive_c/users/steamuser/AppData/Local/Game/Saved/SaveGames"));
        assert_eq!(resolve_save_path("<?INSTALL?>\\saves", pfx, Path::new("/g")), PathBuf::from("/g/saves"));
        assert_eq!(resolve_save_path("<?SAVED_GAMES?>/Studio/Game", pfx, Path::new("/g")), PathBuf::from("/p/drive_c/users/steamuser/Saved Games/Studio/Game"));
    }

    #[test]
    fn launch_info_from_store_data() {
        let epic: serde_json::Value = serde_json::from_str(r#"{"game_parameters":["-SaveToUserDir"],"game_executable":"Binaries/Win64/Game.exe","game_directory":"/games/Game","egl_parameters":["-AUTH_PASSWORD=x"],"working_directory":"","user_parameters":["-nomovie"]}"#).unwrap();
        let l = epic_launch_info(&epic).unwrap();
        assert_eq!(l.exe, PathBuf::from("/games/Game/Binaries/Win64/Game.exe"));
        assert_eq!(l.args, vec!["-SaveToUserDir", "-nomovie"]);
        assert_eq!(l.workdir, Some(PathBuf::from("/games/Game")));
        let gog: serde_json::Value = serde_json::from_str(r#"{"name":"Game","playTasks":[{"category":"launcher","isPrimary":false,"path":"Launcher.exe","type":"FileTask"},{"isPrimary":true,"path":"bin\\game.exe","arguments":"-windowed -x","workingDir":"bin","type":"FileTask"}]}"#).unwrap();
        let l = gog_launch_info(&gog, Path::new("/games/G")).unwrap();
        assert_eq!(l.exe, PathBuf::from("/games/G/bin/game.exe"));
        assert_eq!(l.args, vec!["-windowed", "-x"]);
        assert_eq!(l.workdir, Some(PathBuf::from("/games/G/bin")));
    }
}
