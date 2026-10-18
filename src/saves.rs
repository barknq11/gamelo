//! Save Vault: finds where a game keeps its saves, backs them up after every
//! session, finds copies left in other launchers' prefixes or on a Windows
//! drive, and keeps a sync folder up to date.
//!
//! Save locations are stored as Ludusavi-style patterns ("<home>/AppData/
//! LocalLow/Team Cherry/Hollow Knight/*.dat"), so the same location can be
//! resolved in this game's prefix, another prefix, or a real Windows install.

use crate::*;
use crate::i18n::{fmt_logical as lfmt, logical as lg};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Ludusavi manifest: a big community list of save locations per game
// ---------------------------------------------------------------------------

pub const MANIFEST_URL: &str = "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";

#[derive(Clone, Serialize, Deserialize)]
pub struct MFile {
    pub path: String,
    /// "windows", "linux", or "" (any).
    #[serde(default)]
    pub os: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MGame {
    pub name: String,
    #[serde(default)]
    pub steam: Option<u64>,
    /// Typical install folder names.
    #[serde(default)]
    pub dirs: Vec<String>,
    pub files: Vec<MFile>,
}

fn index_path() -> PathBuf {
    data_dir().join("cache/ludusavi-index.json")
}

/// Unquote a YAML scalar key (double, single or plain).
fn yaml_scalar(s: &str) -> String {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            out.push(ch);
                        }
                    }
                    Some(o) => out.push(o),
                    None => {}
                },
                c => out.push(c),
            }
        }
        out
    } else if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.rfind('\'').unwrap_or(rest.len());
        rest[..end].replace("''", "'")
    } else {
        s.to_string()
    }
}

/// The key of a "key:" or "key: value" line.
fn yaml_key(line: &str) -> String {
    let t = line.trim();
    if t.starts_with('"') || t.starts_with('\'') {
        return yaml_scalar(t);
    }
    let t = t.strip_suffix(": {}").unwrap_or(t);
    let t = t.strip_suffix(':').unwrap_or(t);
    yaml_scalar(t)
}

/// Parse the manifest (a regular, machine-written YAML file) with a small
/// line-based reader: only save files, Steam ids and install folder names.
/// A manifest file entry being read: path, tags, (os, store) conditions.
type FileEntry = (String, Vec<String>, Vec<(String, String)>);

pub fn parse_manifest(text: &str) -> Vec<MGame> {
    struct Cur {
        game: Option<MGame>,
        section: String,
        file: Option<FileEntry>,
        sub: String,
    }
    fn finish_file(c: &mut Cur) {
        let Some((path, tags, whens)) = c.file.take() else { return };
        let Some(g) = c.game.as_mut() else { return };
        if !tags.iter().any(|t| t == "save") {
            return;
        }
        // (os, store) pairs; an empty list means "everywhere".
        let fits = |want: &str| {
            whens.is_empty()
                || whens.iter().any(|(os, store)| (os.is_empty() || os == want) && store != "microsoft")
        };
        let os = match (fits("windows"), fits("linux")) {
            (true, true) => "",
            (true, false) => "windows",
            (false, true) => "linux",
            _ => return,
        };
        g.files.push(MFile { path, os: os.into() });
    }
    fn finish_game(c: &mut Cur, out: &mut Vec<MGame>) {
        finish_file(c);
        if let Some(g) = c.game.take()
            && !g.files.is_empty()
        {
            out.push(g);
        }
    }
    let mut out = Vec::new();
    let mut c = Cur { game: None, section: String::new(), file: None, sub: String::new() };
    for line in text.lines() {
        if line.trim().is_empty() || line == "---" {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        match indent {
            0 => {
                finish_game(&mut c, &mut out);
                c.game = Some(MGame { name: yaml_key(line), steam: None, dirs: Vec::new(), files: Vec::new() });
                c.section.clear();
            }
            2 => {
                finish_file(&mut c);
                c.section = yaml_key(line);
            }
            _ => match c.section.as_str() {
                "files" => {
                    if indent == 4 {
                        finish_file(&mut c);
                        c.file = Some((yaml_key(line), Vec::new(), Vec::new()));
                    } else if indent == 6 {
                        c.sub = yaml_key(line);
                    } else if let Some(f) = c.file.as_mut() {
                        if c.sub == "tags" {
                            if let Some(tag) = t.strip_prefix("- ") {
                                f.1.push(tag.trim().to_string());
                            }
                        } else if c.sub == "when" {
                            let (new_item, kv) = match t.strip_prefix("- ") {
                                Some(rest) => (true, rest),
                                None => (false, t),
                            };
                            if new_item {
                                f.2.push((String::new(), String::new()));
                            }
                            if let (Some((k, v)), Some(last)) = (kv.split_once(':'), f.2.last_mut()) {
                                let v = yaml_scalar(v);
                                match k.trim() {
                                    "os" => last.0 = v,
                                    "store" => last.1 = v,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                "steam" if indent == 4 => {
                    if let (Some(v), Some(g)) = (t.strip_prefix("id:"), c.game.as_mut()) {
                        g.steam = v.trim().parse().ok();
                    }
                }
                "installDir" if indent == 4 => {
                    if let Some(g) = c.game.as_mut() {
                        g.dirs.push(yaml_key(line));
                    }
                }
                _ => {}
            },
        }
    }
    finish_game(&mut c, &mut out);
    out
}

/// Download the manifest and rebuild the compact index used by the app.
pub fn update_manifest() -> Result<String, String> {
    let dir = data_dir().join("cache");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let yaml = dir.join("ludusavi-manifest.yaml");
    let part = dir.join("ludusavi-manifest.yaml.part");
    http_fetch(MANIFEST_URL, Some(&part))?;
    fs::rename(&part, &yaml).map_err(|e| e.to_string())?;
    let text = fs::read_to_string(&yaml).map_err(|e| e.to_string())?;
    let games = parse_manifest(&text);
    let _ = fs::remove_file(&yaml);
    if games.len() < 1000 {
        return Err(lg("The save database looks incomplete — try again later").to_string());
    }
    let json = serde_json::to_string(&games).map_err(|e| e.to_string())?;
    let tmp = index_path().with_extension("tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, index_path()).map_err(|e| e.to_string())?;
    Ok(lfmt("Save database updated: {} games", &[&games.len()]))
}

pub fn load_index() -> Option<Vec<MGame>> {
    serde_json::from_str(&fs::read_to_string(index_path()).ok()?).ok()
}

/// When the index was last built (unix secs).
pub fn index_age() -> Option<u64> {
    let m = fs::metadata(index_path()).ok()?.modified().ok()?;
    m.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Steam app id from a umu ID like "umu-367520" (or a bare number).
fn steam_id_of(g: &CustomGame) -> Option<u64> {
    let id = game_umu_id(g);
    id.strip_prefix("umu-").unwrap_or(&id).parse().ok()
}

/// The manifest entry for a game: the user's choice, else by Steam id, else
/// by name, else by an install-folder name found in the exe's path.
pub fn match_game<'a>(index: &'a [MGame], g: &CustomGame, chosen: Option<&str>) -> Option<&'a MGame> {
    if let Some(name) = chosen {
        return index.iter().find(|m| m.name == name);
    }
    if let Some(id) = steam_id_of(g)
        && let Some(m) = index.iter().find(|m| m.steam == Some(id))
    {
        return Some(m);
    }
    let n = proton::norm_title(&g.name);
    if let Some(m) = index.iter().find(|m| proton::norm_title(&m.name) == n) {
        return Some(m);
    }
    let folders: Vec<String> = Path::new(&g.exe)
        .ancestors()
        .skip(1)
        .take(5)
        .filter_map(|a| a.file_name().map(|f| f.to_string_lossy().to_lowercase()))
        .collect();
    index
        .iter()
        .find(|m| m.dirs.iter().any(|d| d.len() > 3 && folders.contains(&d.to_lowercase())))
}

/// Manifest entries whose name contains every word of `query`.
pub fn search_index<'a>(index: &'a [MGame], query: &str) -> Vec<&'a MGame> {
    let words: Vec<String> = query.split_whitespace().map(proton::norm_title).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&MGame> = index
        .iter()
        .filter(|m| {
            let n = proton::norm_title(&m.name);
            words.iter().all(|w| n.contains(w.as_str()))
        })
        .take(200)
        .collect();
    hits.sort_by_key(|m| m.name.len());
    hits.truncate(30);
    hits
}

// ---------------------------------------------------------------------------
// Where patterns resolve: a Wine/Proton prefix, a Windows install, or Linux
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Ctx {
    /// Shown to the user ("Steam prefix 367520", "Windows drive …").
    pub label: String,
    /// The Windows user folder (or the Linux home for native games).
    pub home: PathBuf,
    /// The C: drive root (None for Linux).
    pub drive_c: Option<PathBuf>,
    /// The game's install folder.
    pub base: Option<PathBuf>,
    pub steam_id: Option<u64>,
    pub linux: bool,
    /// Writing here goes to a real Windows install.
    pub windows_drive: bool,
}

/// The Windows user folder inside a prefix: steamuser (Proton), else the
/// first real user (plain Wine uses the Linux user name).
fn prefix_user_dir(drive_c: &Path) -> Option<PathBuf> {
    let users = drive_c.join("users");
    let steam = users.join("steamuser");
    if steam.is_dir() {
        return Some(steam);
    }
    fs::read_dir(&users)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && !matches!(p.file_name().and_then(|n| n.to_str()), Some("Public" | "public")))
}

pub fn prefix_ctx(prefix: &Path, label: String) -> Option<Ctx> {
    let drive_c = prefix.join("drive_c");
    let home = prefix_user_dir(&drive_c)?;
    Some(Ctx { label, home, drive_c: Some(drive_c), base: None, steam_id: None, linux: false, windows_drive: false })
}

/// The game's install folder: an ancestor of the exe named like a manifest
/// install dir, else the exe's own folder.
fn game_base(g: &CustomGame, m: Option<&MGame>) -> Option<PathBuf> {
    let exe = Path::new(&g.exe);
    if let Some(m) = m {
        for a in exe.ancestors().skip(1).take(6) {
            let name = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
            if name.is_some_and(|n| m.dirs.iter().any(|d| d.to_lowercase() == n)) {
                return Some(a.to_path_buf());
            }
        }
    }
    game_workdir(g)
}

/// This game's own context: its prefix (Windows) or the home folder (Linux).
pub fn game_ctx(g: &CustomGame, settings: &Settings, m: Option<&MGame>) -> Ctx {
    let base = game_base(g, m);
    let steam_id = steam_id_of(g).or(m.and_then(|m| m.steam));
    match g.kind {
        GameKind::Linux => Ctx {
            label: lg("This computer").to_string(),
            home: home(),
            drive_c: None,
            base,
            steam_id,
            linux: true,
            windows_drive: false,
        },
        GameKind::Windows => {
            let prefix = game_prefix(g, settings);
            let drive_c = prefix.join("drive_c");
            let home = prefix_user_dir(&drive_c).unwrap_or_else(|| drive_c.join("users/steamuser"));
            Ctx { label: lg("This game's prefix").to_string(), home, drive_c: Some(drive_c), base, steam_id, linux: false, windows_drive: false }
        }
    }
}

/// Replace placeholders; None when the pattern can't apply in `ctx`.
pub fn expand(pattern: &str, ctx: &Ctx) -> Option<String> {
    let mut out = String::with_capacity(pattern.len() + 40);
    let mut rest = pattern;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('>')? + start;
        let name = &rest[start + 1..end];
        let h = ctx.home.to_string_lossy();
        let value: String = match (name, ctx.linux) {
            ("home", _) => h.to_string(),
            ("xdgData", true) => format!("{h}/.local/share"),
            ("xdgConfig", true) => format!("{h}/.config"),
            ("winAppData", false) => format!("{h}/AppData/Roaming"),
            ("winLocalAppData", false) => format!("{h}/AppData/Local"),
            ("winLocalAppDataLow", false) => format!("{h}/AppData/LocalLow"),
            ("winDocuments", false) => format!("{h}/Documents"),
            ("winPublic", false) => ctx.home.parent()?.join("Public").to_string_lossy().into(),
            ("winProgramData", false) => ctx.drive_c.as_ref()?.join("ProgramData").to_string_lossy().into(),
            ("winDir", false) => ctx.drive_c.as_ref()?.join("windows").to_string_lossy().into(),
            ("base", _) => ctx.base.as_ref()?.to_string_lossy().into(),
            ("game", _) => ctx.base.as_ref()?.file_name()?.to_string_lossy().into(),
            ("storeUserId", _) => "*".into(),
            ("storeGameId", _) => ctx.steam_id.map_or("*".into(), |i| i.to_string()),
            ("osUserName", _) => ctx.home.file_name()?.to_string_lossy().into(),
            _ => return None,
        };
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn has_glob(seg: &str) -> bool {
    seg.contains(['*', '?'])
}

/// Case-insensitive wildcard match of one path segment (`*` and `?`).
fn wild(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A child of `dir` named `seg`, matched case-insensitively if needed
/// (Windows paths are case-insensitive; prefixes and drives may differ).
fn child_ci(dir: &Path, seg: &str) -> Option<PathBuf> {
    let exact = dir.join(seg);
    if exact.exists() {
        return Some(exact);
    }
    let low = seg.to_lowercase();
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().to_lowercase() == low)
        .map(|e| e.path())
}

/// Every existing path matching an expanded absolute pattern.
pub fn glob(expanded: &str) -> Vec<PathBuf> {
    let mut cur: Vec<PathBuf> = vec![PathBuf::from("/")];
    for seg in expanded.split('/').filter(|s| !s.is_empty()) {
        let mut next = Vec::new();
        for dir in &cur {
            if seg == "**" {
                // Any depth, including none.
                let mut stack = vec![dir.clone()];
                while let Some(d) = stack.pop() {
                    next.push(d.clone());
                    if let Ok(rd) = fs::read_dir(&d) {
                        stack.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
                    }
                    if next.len() > 5000 {
                        break;
                    }
                }
            } else if has_glob(seg) {
                if let Ok(rd) = fs::read_dir(dir) {
                    next.extend(rd.flatten().filter(|e| wild(seg, &e.file_name().to_string_lossy())).map(|e| e.path()));
                }
            } else if let Some(p) = child_ci(dir, seg) {
                next.push(p);
            }
        }
        if next.is_empty() {
            return next;
        }
        cur = next;
    }
    cur
}

/// The part of a pattern before its first wildcard — what matches are stored
/// relative to. A pattern without wildcards is relative to its parent.
pub fn pattern_base(pattern: &str) -> String {
    let segs: Vec<&str> = pattern.split('/').collect();
    match segs.iter().position(|s| has_glob(s)) {
        Some(i) => segs[..i].join("/"),
        None => segs[..segs.len().saturating_sub(1)].join("/"),
    }
}

/// Resolve a wildcard-free pattern to a directory to write into, reusing
/// existing folders case-insensitively and creating the rest.
fn dir_for_write(base_pattern: &str, ctx: &Ctx) -> Option<PathBuf> {
    let expanded = expand(base_pattern, ctx)?;
    let mut cur = PathBuf::from("/");
    for seg in expanded.split('/').filter(|s| !s.is_empty()) {
        cur = child_ci(&cur, seg).unwrap_or_else(|| cur.join(seg));
    }
    fs::create_dir_all(&cur).ok()?;
    Some(cur)
}

// ---------------------------------------------------------------------------
// Save locations for a game
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Origin {
    /// From the Ludusavi database.
    Database,
    /// Seen changing while the game ran.
    Learned,
    /// Added by the user.
    Manual,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Location {
    pub pattern: String,
    pub origin: Origin,
}

/// Per-game Save Vault state, kept beside the backups (not in the config, so
/// the headless runner can update it).
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Vault {
    /// Manifest entry picked by the user (None = automatic).
    #[serde(default)]
    pub manifest_name: Option<String>,
    #[serde(default)]
    pub learned: Vec<String>,
    #[serde(default)]
    pub manual: Vec<String>,
    /// Patterns the user turned off.
    #[serde(default)]
    pub ignored: Vec<String>,
    /// Don't back up automatically after sessions.
    #[serde(default)]
    pub no_auto_backup: bool,
    /// Leave this game out of the sync folder.
    #[serde(default)]
    pub no_sync: bool,
    /// When this computer last pushed to / pulled from the sync folder.
    #[serde(default)]
    pub last_sync: u64,
    /// The sync folder and this computer both changed since the last sync.
    #[serde(default)]
    pub conflict: bool,
}

pub fn vault_dir(id: &str) -> PathBuf {
    data_dir().join("saves").join(id)
}

pub fn load_vault(id: &str) -> Vault {
    fs::read_to_string(vault_dir(id).join("vault.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_vault(id: &str, v: &Vault) {
    let dir = vault_dir(id);
    let _ = fs::create_dir_all(&dir);
    let tmp = dir.join("vault.json.tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(v).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, dir.join("vault.json"));
    }
}

/// All locations for a game: database ones fitting its kind, learned, manual.
pub fn locations(g: &CustomGame, m: Option<&MGame>, v: &Vault) -> Vec<Location> {
    let want = if g.kind == GameKind::Linux { "linux" } else { "windows" };
    let mut out: Vec<Location> = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |pattern: &str, origin: Origin| {
        if seen.insert(pattern.to_string()) {
            out.push(Location { pattern: pattern.to_string(), origin });
        }
    };
    if let Some(m) = m {
        for f in m.files.iter().filter(|f| f.os.is_empty() || f.os == want) {
            add(&f.path, Origin::Database);
        }
    }
    for p in &v.learned {
        add(p, Origin::Learned);
    }
    for p in &v.manual {
        add(p, Origin::Manual);
    }
    out
}

/// Matches of the active (not ignored) locations in `ctx`, by pattern, with
/// anything inside another match dropped so nothing is copied twice.
pub fn resolve_all(locs: &[Location], ignored: &[String], ctx: &Ctx) -> Vec<(String, Vec<PathBuf>)> {
    let mut per: Vec<(String, Vec<PathBuf>)> = locs
        .iter()
        .filter(|l| !ignored.contains(&l.pattern))
        .filter_map(|l| Some((l.pattern.clone(), glob(&expand(&l.pattern, ctx)?))))
        .collect();
    let dirs: Vec<PathBuf> = per.iter().flat_map(|(_, m)| m.iter()).filter(|p| p.is_dir()).cloned().collect();
    for (_, matches) in &mut per {
        matches.retain(|p| !dirs.iter().any(|d| d != p && p.starts_with(d)));
    }
    per.retain(|(_, m)| !m.is_empty());
    per
}

/// Size, file count and newest modification time under these paths.
pub fn measure(paths: &[PathBuf]) -> (u64, usize, u64) {
    let (mut size, mut files, mut newest) = (0u64, 0usize, 0u64);
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        let Ok(md) = fs::symlink_metadata(&p) else { continue };
        if md.is_dir() {
            if let Ok(rd) = fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else if md.is_file() {
            size += md.len();
            files += 1;
            let t = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            newest = newest.max(t);
        }
    }
    (size, files, newest)
}

/// A fingerprint of the files (names, sizes, times), to skip identical backups.
fn fingerprint(resolved: &[(String, Vec<PathBuf>)]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut stack: Vec<PathBuf> = resolved.iter().flat_map(|(_, m)| m.iter().cloned()).collect();
    let mut entries = Vec::new();
    while let Some(p) = stack.pop() {
        let Ok(md) = fs::symlink_metadata(&p) else { continue };
        if md.is_dir() {
            if let Ok(rd) = fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else {
            let t = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            entries.push((p, md.len(), t));
        }
    }
    entries.sort();
    entries.hash(&mut h);
    h.finish()
}

/// Copy a file or a whole folder.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let md = fs::symlink_metadata(from)?;
    if md.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
    } else if md.is_file() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
        // Keep modification times, so "newest save" comparisons stay meaningful.
        if let Ok(t) = md.modified() {
            let _ = fs::File::options().write(true).open(to).and_then(|f| f.set_modified(t));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Save sets: backups, sync copies and export packs share one layout
//   <dir>/set.json, <dir>/files/<n>/<path relative to the pattern's base>
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct SetEntry {
    pub pattern: String,
    /// Wildcard-free part of the pattern the files are relative to.
    pub base: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SetInfo {
    pub created: u64,
    #[serde(default)]
    pub label: String,
    pub game: String,
    #[serde(default)]
    pub machine: String,
    pub entries: Vec<SetEntry>,
    pub size: u64,
    pub files: usize,
    #[serde(default)]
    pub fingerprint: u64,
    /// Newest save file time inside the set.
    #[serde(default)]
    pub newest: u64,
}

fn machine_name() -> String {
    fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| "linux".into())
}

/// Write the resolved saves of `ctx` into a new set at `dir`.
pub fn write_set(dir: &Path, game: &str, label: &str, resolved: &[(String, Vec<PathBuf>)], ctx: &Ctx) -> Result<SetInfo, String> {
    let tmp = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("files")).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for (n, (pattern, matches)) in resolved.iter().enumerate() {
        let base_pat = pattern_base(pattern);
        let Some(base) = expand(&base_pat, ctx).map(|b| glob(&b)).and_then(|v| v.into_iter().next()) else {
            continue;
        };
        for m in matches {
            let rel = m.strip_prefix(&base).unwrap_or(m.file_name().map(Path::new).unwrap_or(m));
            copy_tree(m, &tmp.join("files").join(n.to_string()).join(rel)).map_err(|e| format!("{}: {e}", m.display()))?;
        }
        entries.push(SetEntry { pattern: pattern.clone(), base: base_pat });
    }
    let all: Vec<PathBuf> = resolved.iter().flat_map(|(_, m)| m.iter().cloned()).collect();
    let (size, files, newest) = measure(&all);
    let info = SetInfo {
        created: unix_now(),
        label: label.into(),
        game: game.into(),
        machine: machine_name(),
        entries,
        size,
        files,
        fingerprint: fingerprint(resolved),
        newest,
    };
    fs::write(tmp.join("set.json"), serde_json::to_string_pretty(&info).unwrap_or_default()).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(dir);
    fs::rename(&tmp, dir).map_err(|e| e.to_string())?;
    Ok(info)
}

pub fn read_set(dir: &Path) -> Option<SetInfo> {
    serde_json::from_str(&fs::read_to_string(dir.join("set.json")).ok()?).ok()
}

/// Copy a set's files back into `ctx`. Returns the number of files written.
pub fn restore_set(dir: &Path, ctx: &Ctx) -> Result<usize, String> {
    let info = read_set(dir).ok_or_else(|| lg("That backup is damaged (set.json missing)").to_string())?;
    let mut count = 0;
    for (n, e) in info.entries.iter().enumerate() {
        let src = dir.join("files").join(n.to_string());
        if !src.is_dir() {
            continue;
        }
        let target = dir_for_write(&e.base, ctx).ok_or_else(|| lfmt("Can't place saves for {} here", &[&e.pattern]))?;
        for item in fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
            copy_tree(&item.path(), &target.join(item.file_name())).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------

pub fn backups_dir(id: &str) -> PathBuf {
    vault_dir(id).join("backups")
}

/// Backups, newest first: (folder, info).
pub fn list_backups(id: &str) -> Vec<(PathBuf, SetInfo)> {
    let mut v: Vec<(PathBuf, SetInfo)> = fs::read_dir(backups_dir(id))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.extension().is_none())
                .filter_map(|p| read_set(&p).map(|i| (p, i)))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|b| std::cmp::Reverse(b.1.created));
    v
}

/// What the vault needs to work on one game.
pub struct GameSaves {
    pub game: CustomGame,
    pub vault: Vault,
    pub manifest: Option<MGame>,
    pub ctx: Ctx,
    pub locations: Vec<Location>,
}

impl GameSaves {
    pub fn new(g: &CustomGame, settings: &Settings, index: Option<&[MGame]>) -> GameSaves {
        let vault = load_vault(&g.id);
        let manifest = index.and_then(|ix| match_game(ix, g, vault.manifest_name.as_deref())).cloned();
        let ctx = game_ctx(g, settings, manifest.as_ref());
        let locations = locations(g, manifest.as_ref(), &vault);
        GameSaves { game: g.clone(), vault, manifest, ctx, locations }
    }

    /// Matches in another place. Patterns under the game folder only apply
    /// where that place knows its own copy of the game (Steam installs).
    pub fn resolved_in(&self, ctx: &Ctx) -> Vec<(String, Vec<PathBuf>)> {
        resolve_all(&self.locations, &self.vault.ignored, ctx)
    }

    pub fn resolved(&self) -> Vec<(String, Vec<PathBuf>)> {
        resolve_all(&self.locations, &self.vault.ignored, &self.ctx)
    }
}

/// Back up the game's saves. `force` backs up even if nothing changed.
/// Returns Ok(None) when there was nothing (new) to back up.
pub fn backup(gs: &GameSaves, label: &str, keep: usize, force: bool) -> Result<Option<SetInfo>, String> {
    let resolved = gs.resolved();
    if resolved.is_empty() {
        return Ok(None);
    }
    let existing = list_backups(&gs.game.id);
    if !force && existing.first().is_some_and(|(_, i)| i.fingerprint == fingerprint(&resolved)) {
        return Ok(None);
    }
    // Automatic backups skip giant "save" folders (usually a cache that was
    // learned by mistake); a manual backup still goes ahead.
    let all: Vec<PathBuf> = resolved.iter().flat_map(|(_, m)| m.iter().cloned()).collect();
    if !force && measure(&all).0 > 1_000_000_000 {
        return Ok(None);
    }
    let dir = backups_dir(&gs.game.id).join(format!("{}", unix_now()));
    let info = write_set(&dir, &gs.game.name, label, &resolved, &gs.ctx)?;
    // Keep the newest `keep`.
    for (old, _) in list_backups(&gs.game.id).into_iter().skip(keep.max(1)) {
        let _ = fs::remove_dir_all(old);
    }
    Ok(Some(info))
}

/// Restore a backup or a set from elsewhere into the game, backing up the
/// current saves first so the restore can be undone.
pub fn restore_into_game(gs: &GameSaves, dir: &Path, keep: usize) -> Result<String, String> {
    backup(gs, lg("before restore"), keep + 1, false)?;
    let n = restore_set(dir, &gs.ctx)?;
    Ok(lfmt("Restored {} items — your previous saves were backed up first", &[&n]))
}

/// Copy saves from another place (`from`) into `to`, backing up whatever is
/// in `to` first (into this game's backups).
pub fn transfer(gs: &GameSaves, from: &Ctx, to: &Ctx, keep: usize) -> Result<String, String> {
    let resolved = gs.resolved_in(from);
    if resolved.is_empty() {
        return Err(lg("No saves found there").to_string());
    }
    // Safety copy of the destination.
    let dest = gs.resolved_in(to);
    if !dest.is_empty() {
        let dir = backups_dir(&gs.game.id).join(format!("{}", unix_now()));
        write_set(&dir, &gs.game.name, &lfmt("before copying from {}", &[&from.label]), &dest, to)?;
        for (old, _) in list_backups(&gs.game.id).into_iter().skip(keep + 1) {
            let _ = fs::remove_dir_all(old);
        }
    }
    let staging = data_dir().join("saves/.transfer");
    let _ = fs::remove_dir_all(&staging);
    write_set(&staging, &gs.game.name, "transfer", &resolved, from)?;
    let n = restore_set(&staging, to);
    let _ = fs::remove_dir_all(&staging);
    let n = n?;
    Ok(lfmt("Copied {} items from {}", &[&n, &from.label]))
}

// ---------------------------------------------------------------------------
// Learning save folders from what changes during a session
// ---------------------------------------------------------------------------

/// Folders that change every run but never hold saves.
const NOISE: &[&str] = &[
    "temp", "tmp", "microsoft", "nvidia", "nvidia corporation", "amd", "d3dscache", "crashdumps", "cache",
    "caches", "shadercache", "gpucache", "webcache", "inetcache", "history", "logs", "crashreports",
    "packages", "unrealengine", "mesa_shader_cache", "vulkan", "dxvk", "cef", "crashpad",
];

pub type Snapshot = HashMap<PathBuf, (u64, u64)>;

/// Modification times of the files a game might save into.
pub fn scan_user_dirs(ctx: &Ctx) -> Snapshot {
    let mut snap = Snapshot::new();
    let roots: Vec<PathBuf> = if ctx.linux {
        vec![home().join(".local/share"), home().join(".config")]
    } else {
        let h = &ctx.home;
        let mut r = vec![h.join("AppData"), h.join("Documents"), h.join("Saved Games")];
        if let Some(p) = h.parent() {
            r.push(p.join("Public/Documents"));
        }
        r
    };
    let mut stack = roots;
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_lowercase();
            if ft.is_dir() {
                if !NOISE.contains(&name.as_str()) && !name.contains("cache") {
                    stack.push(p);
                }
            } else if ft.is_file()
                && let Ok(md) = e.metadata()
            {
                let t = md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                snap.insert(p, (t, md.len()));
            }
            if snap.len() > 150_000 {
                return snap;
            }
        }
    }
    snap
}

/// Folders (as patterns) holding files that changed between two scans.
pub fn learn(before: &Snapshot, after: &Snapshot, ctx: &Ctx) -> Vec<String> {
    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for (p, stamp) in after {
        if before.get(p) == Some(stamp) {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if [".log", ".tmp", ".lock", ".pid", ".etl"].iter().any(|x| name.ends_with(x)) || name.contains("crash") {
            continue;
        }
        let Some(pattern) = folder_pattern(p, ctx) else { continue };
        *dirs.entry(pattern).or_default() += 1;
    }
    dirs.into_keys().take(4).collect()
}

/// The game's folder (as a pattern) a changed file belongs to: e.g.
/// <home>/AppData/LocalLow/Company/Game, <winDocuments>/My Games/Game.
fn folder_pattern(file: &Path, ctx: &Ctx) -> Option<String> {
    let (root, rel) = if ctx.linux {
        let rel = file.strip_prefix(&ctx.home).ok()?;
        ("<home>", rel.to_path_buf())
    } else if let Ok(rel) = file.strip_prefix(&ctx.home) {
        ("<home>", rel.to_path_buf())
    } else {
        let public = ctx.home.parent()?.join("Public");
        ("<winPublic>", file.strip_prefix(&public).ok()?.to_path_buf())
    };
    let segs: Vec<String> = rel.parent()?.iter().map(|s| s.to_string_lossy().to_string()).collect();
    let low: Vec<String> = segs.iter().map(|s| s.to_lowercase()).collect();
    let depth = match low.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["appdata", "locallow", ..] => 4,
        ["appdata", ..] => 3,
        ["documents", "my games", ..] => 3,
        ["documents", ..] | ["saved games", ..] => 2,
        [".local", "share", ..] | [".config", ..] if ctx.linux => if low[0] == ".config" { 2 } else { 3 },
        _ => return None,
    };
    if segs.len() < depth {
        return None;
    }
    Some(format!("{root}/{}", segs[..depth].join("/")))
}

// ---------------------------------------------------------------------------
// Other places saves may be: other launchers' prefixes, Windows drives
// ---------------------------------------------------------------------------

/// Steam library folders (from libraryfolders.vdf).
fn steam_libraries() -> Vec<PathBuf> {
    let Some(root) = find_steam_root() else { return Vec::new() };
    let mut libs = vec![root.clone()];
    if let Ok(vdf) = fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        for line in vdf.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("\"path\"") {
                let p = rest.trim().trim_matches('"').replace("\\\\", "\\");
                let p = PathBuf::from(p);
                if p.is_dir() && !libs.contains(&p) {
                    libs.push(p);
                }
            }
        }
    }
    libs
}

/// The name Steam shows for an installed app, from its appmanifest.
fn steam_app_name(lib: &Path, appid: &str) -> Option<(String, Option<String>)> {
    let acf = fs::read_to_string(lib.join(format!("steamapps/appmanifest_{appid}.acf"))).ok()?;
    let field = |k: &str| {
        acf.lines().find_map(|l| {
            let t = l.trim();
            t.strip_prefix(&format!("\"{k}\"")).map(|v| v.trim().trim_matches('"').to_string())
        })
    };
    Some((field("name")?, field("installdir")))
}

/// Names of non-Steam games added to Steam, by the compatdata folder number
/// Steam gives them (from the binary shortcuts.vdf files).
fn steam_shortcut_names() -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(root) = find_steam_root() else { return out };
    let Ok(users) = fs::read_dir(root.join("userdata")) else { return out };
    for u in users.flatten() {
        let Ok(data) = fs::read(u.path().join("config/shortcuts.vdf")) else { continue };
        let lower = data.to_ascii_lowercase();
        let mut appid: Option<u32> = None;
        let mut i = 0;
        while i < data.len() {
            if lower[i..].starts_with(b"\x02appid\x00") && i + 11 <= data.len() {
                let b = &data[i + 7..i + 11];
                appid = Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                i += 11;
            } else if lower[i..].starts_with(b"\x01appname\x00") {
                let start = i + 9;
                let end = data[start..].iter().position(|&c| c == 0).map_or(data.len(), |e| start + e);
                if let Some(id) = appid.take() {
                    let name = String::from_utf8_lossy(&data[start..end]).trim_end_matches(".exe").to_string();
                    out.insert(id.to_string(), name);
                }
                i = end;
            } else {
                i += 1;
            }
        }
    }
    out
}

/// Mounted Windows installs: (mount, user folder) for every real user.
fn windows_users() -> Vec<(PathBuf, PathBuf)> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut out = Vec::new();
    for line in mounts.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 || !matches!(f[2], "ntfs" | "ntfs3" | "fuseblk" | "exfat" | "vfat") {
            continue;
        }
        let mnt = PathBuf::from(f[1].replace("\\040", " "));
        let Some(users) = child_ci(&mnt, "Users") else { continue };
        let Ok(rd) = fs::read_dir(&users) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if matches!(n.as_str(), "Public" | "Default" | "Default User" | "All Users" | "desktop.ini") {
                continue;
            }
            if e.path().join("AppData").is_dir() {
                out.push((mnt.clone(), e.path()));
            }
        }
    }
    out
}

/// Every place other than the game's own prefix that might hold its saves.
pub fn other_contexts(own_prefix: Option<&Path>, settings: &Settings) -> Vec<Ctx> {
    let mut out: Vec<Ctx> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let own = own_prefix.and_then(|p| p.canonicalize().ok());
    let mut push_prefix = |prefix: &Path, label: String, base: Option<PathBuf>, out: &mut Vec<Ctx>| {
        let canon = prefix.canonicalize().unwrap_or_else(|_| prefix.to_path_buf());
        if Some(&canon) == own.as_ref() || !seen.insert(canon) {
            return;
        }
        if let Some(mut c) = prefix_ctx(prefix, label) {
            c.base = base;
            out.push(c);
        }
    };
    // Steam's Proton prefixes.
    let shortcuts = steam_shortcut_names();
    for lib in steam_libraries() {
        let Ok(rd) = fs::read_dir(lib.join("steamapps/compatdata")) else { continue };
        for e in rd.flatten() {
            let appid = e.file_name().to_string_lossy().to_string();
            let pfx = e.path().join("pfx");
            if !pfx.join("drive_c").is_dir() {
                continue;
            }
            let (label, base) = match steam_app_name(&lib, &appid) {
                Some((name, dir)) => (
                    lfmt("Steam: {} ({})", &[&name, &appid]),
                    dir.map(|d| lib.join("steamapps/common").join(d)),
                ),
                None => match shortcuts.get(&appid) {
                    Some(name) => (lfmt("Steam (non-Steam game): {}", &[name]), None),
                    None => (lfmt("Steam prefix {}", &[&appid]), None),
                },
            };
            push_prefix(&pfx, label, base, &mut out);
        }
    }
    // Other launchers and our own prefixes: any folder holding a drive_c.
    let h = home();
    let roots: Vec<(PathBuf, &str, usize)> = vec![
        (h.join("Games/Heroic/Prefixes"), "Heroic", 2),
        (h.join(".config/heroic/Prefixes"), "Heroic", 2),
        (h.join(".local/share/lutris/prefixes"), "Lutris", 1),
        (h.join("Games"), "Lutris / Games", 2),
        (h.join(".local/share/bottles/bottles"), "Bottles", 1),
        (h.join(".var/app/com.usebottles.bottles/data/bottles/bottles"), "Bottles", 1),
        (h.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/Prefixes"), "Heroic", 2),
        (settings.prefix_base(), "ProtonLaunch", 1),
    ];
    for (root, who, depth) in roots {
        let mut level = vec![root];
        for _ in 0..depth {
            let mut next = Vec::new();
            for d in &level {
                let Ok(rd) = fs::read_dir(d) else { continue };
                for e in rd.flatten().filter(|e| e.path().is_dir()) {
                    let p = e.path();
                    if p.join("drive_c").is_dir() {
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        push_prefix(&p, format!("{who}: {name}"), None, &mut out);
                    } else {
                        next.push(p);
                    }
                }
            }
            level = next;
        }
    }
    if h.join(".wine/drive_c").is_dir() {
        push_prefix(&h.join(".wine"), "Wine (~/.wine)".into(), None, &mut out);
    }
    // Real Windows installs on mounted drives.
    for (mnt, user) in windows_users() {
        let name = user.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        out.push(Ctx {
            label: lfmt("Windows ({}) at {}", &[&name, &mnt.display()]),
            home: user,
            drive_c: Some(mnt),
            base: None,
            steam_id: None,
            linux: false,
            windows_drive: true,
        });
    }
    out
}

/// A copy of a game's saves found somewhere else.
#[derive(Clone)]
pub struct Found {
    pub ctx: Ctx,
    pub size: u64,
    pub files: usize,
    pub newest: u64,
    pub paths: Vec<PathBuf>,
}

/// Look for this game's saves in every other place.
pub fn find_elsewhere(gs: &GameSaves, settings: &Settings) -> Vec<Found> {
    let own = (gs.game.kind == GameKind::Windows).then(|| game_prefix(&gs.game, settings));
    let mut out: Vec<Found> = other_contexts(own.as_deref(), settings)
        .into_iter()
        .filter_map(|ctx| {
            let resolved = gs.resolved_in(&ctx);
            let paths: Vec<PathBuf> = resolved.into_iter().flat_map(|(_, m)| m).collect();
            if paths.is_empty() {
                return None;
            }
            let (size, files, newest) = measure(&paths);
            (files > 0).then_some(Found { ctx, size, files, newest, paths })
        })
        .collect();
    out.sort_by_key(|f| std::cmp::Reverse(f.newest));
    out
}

// ---------------------------------------------------------------------------
// Sync folder: one copy per game, pulled before launch and pushed after
// ---------------------------------------------------------------------------

pub fn sync_dir_for(settings: &Settings, g: &CustomGame) -> Option<PathBuf> {
    let root = settings.save_sync_dir.trim();
    if root.is_empty() {
        return None;
    }
    let safe: String = g
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_.()'".contains(c) { c } else { '_' })
        .collect();
    Some(PathBuf::from(root).join(safe.trim()))
}

pub enum SyncState {
    Off,
    /// The folder copy is newer (pull before playing).
    Newer(SetInfo),
    /// This computer has changes the folder doesn't.
    LocalNewer,
    Same,
    /// Both changed since the last sync.
    Conflict(SetInfo),
    Empty,
}

pub fn sync_state(gs: &GameSaves, settings: &Settings) -> SyncState {
    let Some(dir) = sync_dir_for(settings, &gs.game).filter(|_| !gs.vault.no_sync) else {
        return SyncState::Off;
    };
    let resolved = gs.resolved();
    let local_fp = fingerprint(&resolved);
    let local_newest = measure(&resolved.iter().flat_map(|(_, m)| m.iter().cloned()).collect::<Vec<_>>()).2;
    let Some(remote) = read_set(&dir) else {
        return if resolved.is_empty() { SyncState::Empty } else { SyncState::LocalNewer };
    };
    if remote.fingerprint == local_fp {
        return SyncState::Same;
    }
    let remote_changed = remote.created > gs.vault.last_sync;
    let local_changed = resolved.is_empty() || local_newest > gs.vault.last_sync;
    match (remote_changed, local_changed) {
        (true, false) => SyncState::Newer(remote),
        (true, true) if gs.vault.last_sync == 0 && resolved.is_empty() => SyncState::Newer(remote),
        (true, true) if gs.vault.last_sync == 0 => {
            // Never synced here: the newer saves win.
            if remote.newest > local_newest { SyncState::Newer(remote) } else { SyncState::LocalNewer }
        }
        (true, true) => SyncState::Conflict(remote),
        _ => SyncState::LocalNewer,
    }
}

pub fn push_sync(gs: &mut GameSaves, settings: &Settings) -> Result<String, String> {
    let dir = sync_dir_for(settings, &gs.game).ok_or_else(|| lg("No sync folder set (Settings › Saves)").to_string())?;
    let resolved = gs.resolved();
    if resolved.is_empty() {
        return Err(lg("No saves to sync yet").to_string());
    }
    fs::create_dir_all(dir.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
    write_set(&dir, &gs.game.name, "sync", &resolved, &gs.ctx)?;
    gs.vault.last_sync = unix_now();
    gs.vault.conflict = false;
    save_vault(&gs.game.id, &gs.vault);
    Ok(lfmt("Saves for {} copied to the sync folder", &[&gs.game.name]))
}

pub fn pull_sync(gs: &mut GameSaves, settings: &Settings) -> Result<String, String> {
    let dir = sync_dir_for(settings, &gs.game).ok_or_else(|| lg("No sync folder set (Settings › Saves)").to_string())?;
    let msg = restore_into_game(gs, &dir, settings.save_keep())?;
    gs.vault.last_sync = unix_now();
    gs.vault.conflict = false;
    save_vault(&gs.game.id, &gs.vault);
    Ok(msg)
}

// ---------------------------------------------------------------------------
// Export packs (to carry saves to another PC or to Windows by hand)
// ---------------------------------------------------------------------------

/// How a pattern base reads on Windows, for the README in export packs.
pub fn windows_path(base: &str) -> String {
    base.replace("<winAppData>", "%APPDATA%")
        .replace("<winLocalAppData>", "%LOCALAPPDATA%")
        .replace("<winDocuments>", "%USERPROFILE%\\Documents")
        .replace("<winPublic>", "C:\\Users\\Public")
        .replace("<winProgramData>", "C:\\ProgramData")
        .replace("<home>", "%USERPROFILE%")
        .replace("<base>", "<game folder>")
        .replace('/', "\\")
}

pub fn export_pack(gs: &GameSaves, dest_parent: &Path) -> Result<String, String> {
    let resolved = gs.resolved();
    if resolved.is_empty() {
        return Err(lg("No saves found to export").to_string());
    }
    let dir = dest_parent.join(format!("{} saves", gs.game.name.replace('/', "_")));
    let info = write_set(&dir, &gs.game.name, "export", &resolved, &gs.ctx)?;
    let mut readme = format!(
        "{} — saves exported by ProtonLaunch\n\nTo import on another Linux PC: ProtonLaunch → the game → Saves → Import from folder, and pick this folder.\n\nTo use them on Windows, copy the contents of each folder below to the Windows path shown:\n\n",
        gs.game.name
    );
    for (n, e) in info.entries.iter().enumerate() {
        readme.push_str(&format!("files\\{n}  →  {}\n", windows_path(&e.base)));
    }
    let _ = fs::write(dir.join("README.txt"), readme);
    Ok(lfmt("Exported to {}", &[&dir.display()]))
}

/// Import a folder: an export pack / backup (has set.json), or plain save
/// files copied from somewhere, placed into the location `target`.
pub fn import_folder(gs: &GameSaves, src: &Path, target: Option<&str>, keep: usize) -> Result<String, String> {
    if src.join("set.json").is_file() {
        return restore_into_game(gs, src, keep);
    }
    let pattern = target.ok_or_else(|| lg("Pick which save location the folder belongs to").to_string())?;
    backup(gs, lg("before import"), keep + 1, false)?;
    // A folder named like the location's last part *is* that folder.
    let last = pattern.rsplit('/').next().unwrap_or("");
    let src_name = src.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let (dest_pat, whole) = if !has_glob(last) && last.to_lowercase() == src_name {
        (pattern_base(pattern), true)
    } else if has_glob(last) {
        (pattern_base(pattern), false)
    } else {
        (pattern.to_string(), false)
    };
    let dest = dir_for_write(&dest_pat, &gs.ctx).ok_or_else(|| lg("Can't create the save folder").to_string())?;
    let mut n = 0;
    if whole {
        copy_tree(src, &dest.join(src.file_name().unwrap_or_default())).map_err(|e| e.to_string())?;
        n += 1;
    } else {
        for e in fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
            copy_tree(&e.path(), &dest.join(e.file_name())).map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(lfmt("Imported {} items into {}", &[&n, &dest.display()]))
}

// ---------------------------------------------------------------------------
// Hooks for the headless runner
// ---------------------------------------------------------------------------

/// State captured before a session starts.
pub struct PreSession {
    snapshot: Snapshot,
}

/// Before launch: pull newer saves from the sync folder, and remember what the
/// save folders look like so new save locations can be learned afterwards.
pub fn before_launch(g: &CustomGame, settings: &Settings) -> PreSession {
    let index = load_index();
    let mut gs = GameSaves::new(g, settings, index.as_deref());
    match sync_state(&gs, settings) {
        SyncState::Newer(_) => match pull_sync(&mut gs, settings) {
            Ok(m) => eprintln!("protonlaunch: {m}"),
            Err(e) => eprintln!("protonlaunch: sync pull failed: {e}"),
        },
        SyncState::Conflict(_) => {
            gs.vault.conflict = true;
            save_vault(&g.id, &gs.vault);
        }
        _ => {}
    }
    PreSession { snapshot: scan_user_dirs(&gs.ctx) }
}

/// After the game exits: learn new save folders, back up, push to sync.
pub fn after_session(g: &CustomGame, settings: &Settings, pre: PreSession) {
    let index = load_index();
    let mut gs = GameSaves::new(g, settings, index.as_deref());
    let after = scan_user_dirs(&gs.ctx);
    let known: Vec<String> = gs.resolved().iter().flat_map(|(_, m)| m.iter()).map(|p| p.to_string_lossy().to_string()).collect();
    let mut changed = false;
    for pat in learn(&pre.snapshot, &after, &gs.ctx) {
        // Skip folders already covered by a known location.
        let covered = expand(&pat, &gs.ctx).is_some_and(|e| known.iter().any(|k| e.starts_with(k.as_str())));
        if !covered && !gs.vault.learned.contains(&pat) && !gs.vault.ignored.contains(&pat) {
            let big = expand(&pat, &gs.ctx).is_some_and(|e| dir_size(Path::new(&e)) > 1_000_000_000);
            if !big {
                gs.vault.learned.push(pat);
                changed = true;
            }
        }
    }
    if changed {
        save_vault(&g.id, &gs.vault);
        gs = GameSaves::new(g, settings, index.as_deref());
    }
    if settings.save_auto_backup() && !gs.vault.no_auto_backup
        && let Err(e) = backup(&gs, lg("after playing"), settings.save_keep(), false)
    {
        eprintln!("protonlaunch: backup failed: {e}");
    }
    if !gs.vault.conflict
        && sync_dir_for(settings, g).is_some()
        && !gs.vault.no_sync
        && !gs.resolved().is_empty()
        && let Err(e) = push_sync(&mut gs, settings)
    {
        eprintln!("protonlaunch: sync push failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = r#"---
"Hollow Knight":
  files:
    "<base>/hollow_knight_Data/Config.ini":
      tags:
        - config
    "<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat":
      tags:
        - save
      when:
        - os: windows
    "<home>/Library/x":
      tags:
        - save
      when:
        - os: mac
    "<xdgConfig>/unity3d/Team Cherry/Hollow Knight/*.dat":
      tags:
        - save
      when:
        - os: linux
    "<winProgramData>/Packages/x":
      tags:
        - save
      when:
        - os: windows
          store: microsoft
  installDir:
    Hollow Knight: {}
  launch:
    "<base>/hollow_knight.exe":
      - when:
          - os: windows
  steam:
    id: 367520
NoSaves:
  steam:
    id: 1
'Quote''s Game':
  files:
    <winDocuments>/Quote/save.sav:
      tags:
        - save
"#;

    #[test]
    fn manifest_is_parsed() {
        let games = parse_manifest(YAML);
        assert_eq!(games.len(), 2);
        let hk = &games[0];
        assert_eq!(hk.name, "Hollow Knight");
        assert_eq!(hk.steam, Some(367520));
        assert_eq!(hk.dirs, vec!["Hollow Knight"]);
        let paths: Vec<(&str, &str)> = hk.files.iter().map(|f| (f.path.as_str(), f.os.as_str())).collect();
        assert_eq!(
            paths,
            vec![
                ("<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat", "windows"),
                ("<xdgConfig>/unity3d/Team Cherry/Hollow Knight/*.dat", "linux"),
            ]
        );
        assert_eq!(games[1].name, "Quote's Game");
        assert_eq!(games[1].files[0].os, "");
    }

    #[test]
    fn wildcards_and_bases() {
        assert!(wild("*.dat", "user1.DAT"));
        assert!(wild("user?.dat", "user2.dat"));
        assert!(!wild("*.dat", "user1.dat.bak"));
        assert!(wild("*", "anything"));
        assert_eq!(pattern_base("<home>/A/B/*.dat"), "<home>/A/B");
        assert_eq!(pattern_base("<home>/A/B"), "<home>/A");
        assert_eq!(windows_path("<winAppData>/Game"), "%APPDATA%\\Game");
    }

    fn fake_prefix(root: &Path) -> Ctx {
        let home = root.join("drive_c/users/steamuser");
        fs::create_dir_all(home.join("AppData/LocalLow/Team Cherry/Hollow Knight")).unwrap();
        prefix_ctx(root, "test".into()).unwrap()
    }

    #[test]
    fn backup_transfer_and_learning_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("plaunch-saves-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let a = fake_prefix(&tmp.join("a"));
        let b = fake_prefix(&tmp.join("b"));
        let hk = a.home.join("AppData/LocalLow/Team Cherry/Hollow Knight");
        fs::write(hk.join("user1.dat"), "progress").unwrap();
        fs::write(hk.join("notes.txt"), "x").unwrap();
        let locs = vec![Location { pattern: "<home>/AppData/LocalLow/team cherry/Hollow Knight/*.dat".into(), origin: Origin::Database }];
        let resolved = resolve_all(&locs, &[], &a);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].1, vec![hk.join("user1.dat")], "case-insensitive folder, *.dat only");
        let set = tmp.join("set");
        let info = write_set(&set, "HK", "t", &resolved, &a).unwrap();
        assert_eq!((info.files, info.size), (1, 8));
        // Restore into a different prefix (different case on disk is fine).
        assert_eq!(restore_set(&set, &b).unwrap(), 1);
        let got = fs::read_to_string(b.home.join("AppData/LocalLow/Team Cherry/Hollow Knight/user1.dat")).unwrap();
        assert_eq!(got, "progress");
        // And into a Windows-style tree with "Users" and no folders yet.
        let win = Ctx { home: tmp.join("win/Users/me"), drive_c: Some(tmp.join("win")), ..a.clone() };
        assert_eq!(restore_set(&set, &win).unwrap(), 1);
        assert!(tmp.join("win/Users/me/AppData/LocalLow/team cherry/Hollow Knight/user1.dat").is_file());
        // Learning: a new file under Documents/My Games is noticed, logs are not.
        let before = scan_user_dirs(&a);
        let mg = a.home.join("Documents/My Games/Skyrim/Saves");
        fs::create_dir_all(&mg).unwrap();
        fs::write(mg.join("save1.ess"), "s").unwrap();
        fs::create_dir_all(a.home.join("AppData/Local/Game/Logs")).unwrap();
        fs::write(a.home.join("AppData/Local/Game/Logs/run.log"), "l").unwrap();
        fs::create_dir_all(a.home.join("AppData/Local/Temp")).unwrap();
        fs::write(a.home.join("AppData/Local/Temp/x.dat"), "t").unwrap();
        let after = scan_user_dirs(&a);
        assert_eq!(learn(&before, &after, &a), vec!["<home>/Documents/My Games/Skyrim".to_string()]);
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Read-only: where would each of your games' saves be, and what other
    /// copies exist? cargo test -- --ignored saves_real --nocapture
    #[test]
    #[ignore]
    fn saves_real() {
        let cfg = load_config();
        let index = load_index().or_else(|| {
            update_manifest().ok()?;
            load_index()
        });
        for g in &cfg.custom_games {
            let gs = GameSaves::new(g, &cfg.settings, index.as_deref());
            eprintln!("== {} -> manifest {:?}", g.name, gs.manifest.as_ref().map(|m| &m.name));
            for (p, m) in gs.resolved() {
                eprintln!("   {p}: {} matches", m.len());
            }
            for f in find_elsewhere(&gs, &cfg.settings) {
                eprintln!("   elsewhere: {} — {} files, {} bytes, newest {}", f.ctx.label, f.files, f.size, f.newest);
            }
        }
    }

    /// Parses the real, downloaded manifest. cargo test -- --ignored manifest_live
    #[test]
    #[ignore]
    fn manifest_live() {
        let text = http_fetch(MANIFEST_URL, None).unwrap();
        let t0 = std::time::Instant::now();
        let games = parse_manifest(&text);
        eprintln!("{} games with saves, parsed in {:?}", games.len(), t0.elapsed());
        assert!(games.len() > 10_000);
        let hk = games.iter().find(|g| g.name == "Hollow Knight").unwrap();
        assert_eq!(hk.steam, Some(367520));
        assert!(hk.files.iter().any(|f| f.path.contains("LocalLow/Team Cherry")));
        let json = serde_json::to_string(&games).unwrap();
        eprintln!("index size {} bytes", json.len());
    }
}
