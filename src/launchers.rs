//! Windows launchers (Battle.net, EA app, Ubisoft Connect, Rockstar), each in
//! its own prefix. Games installed through them are found in the prefix's
//! registry and added to the library; they start through their launcher, and
//! the runner keeps the session open while the game's folder has a process.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::HashMap;

pub struct Def {
    pub key: &'static str,
    pub name: &'static str,
    about: &'static str,
    url: &'static str,
    file: &'static str,
    /// Where the launcher lands, relative to drive_c (first found wins).
    exe: &'static [&'static str],
    color: (u8, u8, u8),
}

pub const ALL: [Def; 4] = [
    Def {
        key: "battlenet",
        name: "Battle.net",
        about: "Blizzard and Activision games: Diablo, Overwatch, World of Warcraft, Call of Duty.",
        url: "https://downloader.battle.net/download/getInstaller?os=win&installer=Battle.net-Setup.exe",
        file: "Battle.net-Setup.exe",
        exe: &["Program Files (x86)/Battle.net/Battle.net.exe", "Program Files (x86)/Battle.net/Battle.net Launcher.exe"],
        color: (0x14, 0x8E, 0xFF),
    },
    Def {
        key: "ea",
        name: "EA app",
        about: "Electronic Arts games: Battlefield, The Sims, Mass Effect, Dragon Age.",
        url: "https://origin-a.akamaihd.net/EA-Desktop-Client-Download/installer-releases/EAappInstaller.exe",
        file: "EAappInstaller.exe",
        exe: &["Program Files/Electronic Arts/EA Desktop/EA Desktop/EALauncher.exe", "Program Files/Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe"],
        color: (0xFF, 0x47, 0x47),
    },
    Def {
        key: "ubisoft",
        name: "Ubisoft Connect",
        about: "Ubisoft games: Assassin's Creed, Far Cry, Rainbow Six, Anno.",
        url: "https://static3.cdn.ubi.com/orbit/launcher_installer/UbisoftConnectInstaller.exe",
        file: "UbisoftConnectInstaller.exe",
        exe: &["Program Files (x86)/Ubisoft/Ubisoft Game Launcher/UbisoftConnect.exe", "Program Files (x86)/Ubisoft/Ubisoft Game Launcher/upc.exe"],
        color: (0x2E, 0x6B, 0xF0),
    },
    Def {
        key: "rockstar",
        name: "Rockstar Games Launcher",
        about: "Grand Theft Auto V and Red Dead Redemption 2 (GTA Online's anti-cheat doesn't allow Linux).",
        url: "https://gamedownloads.rockstargames.com/public/installer/Rockstar-Games-Launcher.exe",
        file: "Rockstar-Games-Launcher.exe",
        exe: &["Program Files/Rockstar Games/Launcher/Launcher.exe"],
        color: (0xFC, 0xAF, 0x17),
    },
];

pub fn def(key: &str) -> Option<&'static Def> {
    ALL.iter().find(|d| d.key == key)
}

/// A game in the library that starts through a launcher.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct LauncherLink {
    pub key: String,
    /// The game's install folder (empty for the launcher itself).
    #[serde(default)]
    pub watch: String,
    /// What identifies the game to its launcher ("ubisoft:4311"…).
    #[serde(default)]
    pub game: String,
}

pub fn prefix(settings: &Settings, d: &Def) -> PathBuf {
    settings.prefix_base().join(format!("launcher-{}", d.key))
}

pub fn launcher_exe(settings: &Settings, d: &Def) -> Option<PathBuf> {
    let c = prefix(settings, d).join("drive_c");
    d.exe.iter().map(|r| c.join(r)).find(|p| p.is_file())
}

/// String values of a Wine registry file, by lower-cased key path and name.
pub fn read_reg(text: &str) -> HashMap<String, HashMap<String, String>> {
    let unescape = |s: &str| s.replace("\\\\", "\u{0}").replace("\\\"", "\"").replace('\u{0}', "\\");
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut key = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            key = rest.rsplit_once(']').map_or(rest, |(k, _)| k).replace("\\\\", "\\").to_lowercase();
            continue;
        }
        let Some(rest) = line.strip_prefix('"') else { continue };
        let Some((name, value)) = rest.split_once("\"=\"") else { continue };
        let value = value.strip_suffix('"').unwrap_or(value);
        out.entry(key.clone()).or_default().insert(name.to_lowercase(), unescape(value));
    }
    out
}

/// "C:\Program Files\X" → <prefix>/drive_c/Program Files/X.
pub fn win_to_unix(prefix: &Path, p: &str) -> PathBuf {
    let p = p.replace('\\', "/");
    let rest = p.get(2..).filter(|_| p.as_bytes().get(1) == Some(&b':')).unwrap_or(&p);
    let drive = p.chars().next().map(|c| c.to_ascii_lowercase()).unwrap_or('c');
    let root = if drive == 'c' { prefix.join("drive_c") } else { prefix.join("dosdevices").join(format!("{drive}:")) };
    root.join(rest.trim_start_matches('/').trim_end_matches('/'))
}

/// A game found in a launcher's prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub game: String,
    pub name: String,
    pub exe: PathBuf,
    pub args: String,
    pub watch: PathBuf,
}

/// Battle.net's uninstall ids → the product codes `--exec="launch …"` takes.
const BLIZZARD: &[(&str, &str)] = &[
    ("prometheus", "Pro"),
    ("wow", "WoW"),
    ("wow_classic", "WoWC"),
    ("diablo3", "D3"),
    ("fenris", "Fen"),
    ("osi", "OSI"),
    ("hs_beta", "WTCG"),
    ("heroes", "Hero"),
    ("s2", "S2"),
    ("s1", "S1"),
    ("w3", "W3"),
    ("viper", "VIPR"),
    ("odin", "ODIN"),
    ("lazarus", "LAZR"),
    ("zeus", "ZEUS"),
    ("fore", "FORE"),
    ("auks", "AUKS"),
    ("anbs", "ANBS"),
    ("rtro", "RTRO"),
    ("wlby", "WLBY"),
    ("gryphon", "GRY"),
];

/// Games installed through `d`, read from its prefix.
pub fn detect(settings: &Settings, d: &Def) -> Vec<Found> {
    let pfx = prefix(settings, d);
    let Ok(text) = fs::read_to_string(pfx.join("system.reg")) else { return Vec::new() };
    let reg = read_reg(&text);
    let start = pfx.join("drive_c/windows/command/start.exe");
    let uninstall = |f: &mut dyn FnMut(&HashMap<String, String>)| {
        for (k, v) in &reg {
            if k.contains("microsoft\\windows\\currentversion\\uninstall\\") {
                f(v);
            }
        }
    };
    let mut out = Vec::new();
    match d.key {
        "ubisoft" => {
            for (k, v) in &reg {
                let Some(id) = k.strip_prefix("software\\wow6432node\\ubisoft\\launcher\\installs\\") else { continue };
                let Some(dir) = v.get("installdir") else { continue };
                let watch = win_to_unix(&pfx, dir);
                let name = watch.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| id.to_string());
                out.push(Found { game: format!("ubisoft:{id}"), name, exe: start.clone(), args: format!("uplay://launch/{id}/0"), watch });
            }
        }
        "battlenet" => {
            let Some(bnet) = launcher_exe(settings, d) else { return Vec::new() };
            uninstall(&mut |v| {
                let Some(un) = v.get("uninstallstring").filter(|u| u.contains("Blizzard Uninstaller")) else { return };
                let Some(uid) = un.split("--uid=").nth(1).map(|s| s.split([' ', '"']).next().unwrap_or("").to_lowercase()) else { return };
                let Some(&(_, code)) = BLIZZARD.iter().find(|(u, _)| *u == uid) else { return };
                let (Some(name), Some(loc)) = (v.get("displayname"), v.get("installlocation")) else { return };
                out.push(Found { game: format!("battlenet:{code}"), name: name.clone(), exe: bnet.clone(), args: format!("--exec=\"launch {code}\""), watch: win_to_unix(&pfx, loc) });
            });
        }
        "ea" => {
            uninstall(&mut |v| {
                let (Some(name), Some(loc)) = (v.get("displayname"), v.get("installlocation")) else { return };
                let watch = win_to_unix(&pfx, loc);
                let Ok(xml) = fs::read_to_string(watch.join("__Installer/installerdata.xml")) else { return };
                let Some(id) = xml.split("<contentID>").nth(1).and_then(|s| s.split('<').next()) else { return };
                out.push(Found { game: format!("ea:{id}"), name: name.clone(), exe: start.clone(), args: format!("origin2://game/launch?offerIds={id}"), watch });
            });
        }
        "rockstar" => {
            for (k, v) in &reg {
                let Some(title) = k.strip_prefix("software\\wow6432node\\rockstar games\\") else { continue };
                if title.contains('\\') || title == "launcher" || title.contains("social club") {
                    continue;
                }
                let Some(dir) = v.get("installfolder") else { continue };
                let watch = win_to_unix(&pfx, dir);
                // Each game ships a small Play*.exe that starts it through the launcher.
                let play = fs::read_dir(&watch).into_iter().flatten().flatten().map(|e| e.path()).find(|p| {
                    p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy().to_lowercase();
                        n.starts_with("play") && n.ends_with(".exe")
                    })
                });
                let Some(exe) = play else { continue };
                let name = watch.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| title.to_string());
                out.push(Found { game: format!("rockstar:{title}"), name, exe, args: String::new(), watch });
            }
        }
        _ => {}
    }
    out.retain(|f| f.exe.is_file() && f.watch.is_dir());
    out.sort_by(|a, b| a.game.cmp(&b.game));
    out.dedup_by(|a, b| a.game == b.game);
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}

/// Whether a process is running from `dir` (by its working folder, or its
/// command line naming the folder, Windows-style or not).
fn folder_busy(dir: &Path, win: &str) -> bool {
    let unix = dir.to_string_lossy().to_lowercase();
    fs::read_dir("/proc").into_iter().flatten().flatten().any(|e| {
        let p = e.path();
        if !e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if fs::read_link(p.join("cwd")).is_ok_and(|c| c.starts_with(dir)) {
            return true;
        }
        let cmd = fs::read(p.join("cmdline")).map(|b| String::from_utf8_lossy(&b).to_lowercase().replace('\0', " ")).unwrap_or_default();
        cmd.contains(&unix) || (!win.is_empty() && cmd.replace('/', "\\").contains(win))
    })
}

/// After a launcher-started game's command returns, wait while the game runs:
/// up to two minutes for it to appear, then until it's been gone ~6 seconds.
pub fn wait_for_game(link: &LauncherLink, settings: &Settings) {
    if link.watch.is_empty() {
        return;
    }
    let dir = PathBuf::from(&link.watch);
    let win = def(&link.key)
        .and_then(|d| dir.strip_prefix(prefix(settings, d).join("drive_c")).ok().map(|r| format!("c:\\{}", r.to_string_lossy().replace('/', "\\")).to_lowercase()))
        .unwrap_or_default();
    let appear_by = Instant::now() + Duration::from_secs(120);
    let (mut seen, mut gone) = (false, 0);
    loop {
        std::thread::sleep(Duration::from_secs(2));
        if folder_busy(&dir, &win) {
            seen = true;
            gone = 0;
        } else if seen {
            gone += 1;
            if gone >= 3 {
                return;
            }
        } else if Instant::now() > appear_by {
            return;
        }
    }
}

impl App {
    fn launcher_entry(&self, key: &str) -> Option<usize> {
        self.cfg.custom_games.iter().position(|g| g.launcher.as_ref().is_some_and(|l| l.key == key && l.watch.is_empty()))
    }

    /// Add the launcher itself and any new games it installed. Returns how
    /// many games were added.
    pub fn launchers_sync(&mut self, ctx: &egui::Context, d: &Def) -> usize {
        let Some(exe) = launcher_exe(&self.cfg.settings, d) else { return 0 };
        let pfx = prefix(&self.cfg.settings, d).to_string_lossy().to_string();
        let mut new_ids = Vec::new();
        let mut add = |app: &mut App, name: &str, exe: PathBuf, args: String, link: LauncherLink| {
            let mut m = app.new_add_modal(exe);
            m.name = name.to_string();
            m.kind = GameKind::Windows;
            let (id, _) = app.insert_game(m);
            if let Some(g) = app.cfg.custom_games.iter_mut().find(|g| g.id == id) {
                g.prefix = pfx.clone();
                g.args = args;
                g.launcher = Some(link);
            }
            new_ids.push(id);
        };
        if self.launcher_entry(d.key).is_none() {
            add(self, d.name, exe, String::new(), LauncherLink { key: d.key.into(), ..Default::default() });
        }
        let mut games = 0;
        for f in detect(&self.cfg.settings, d) {
            let known = self.cfg.custom_games.iter().any(|g| g.launcher.as_ref().is_some_and(|l| l.game == f.game));
            if !known {
                add(self, &f.name, f.exe, f.args, LauncherLink { key: d.key.into(), watch: f.watch.to_string_lossy().into(), game: f.game });
                games += 1;
            }
        }
        if !new_ids.is_empty() {
            save_config(&self.cfg);
            if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
                self.auto_fetch(ctx, new_ids);
            }
        }
        games
    }

    fn install_launcher(&mut self, d: &'static Def) {
        let key = format!("launcher:{}", d.key);
        if self.tasks.iter().any(|t| t.key == key) {
            return;
        }
        let m = self.new_add_modal(PathBuf::from(d.file));
        let Some(proton) = self.proton_tools.get(m.proton_idx).map(|t| t.dir.to_string_lossy().to_string()) else {
            self.set_status(lg("Install GE-Proton first (Proton page)").into(), true);
            return;
        };
        let settings = self.cfg.settings.clone();
        let dl = data_dir().join("downloads").join(d.file);
        let label = lfmt("Installing {}", &[&d.name]);
        self.start_task(TaskKind::Launcher, &key, &label, Some((dl.clone(), 0)), move || {
            let _ = fs::create_dir_all(dl.parent().unwrap_or(&dl));
            http_fetch(d.url, Some(&dl)).map_err(|e| lfmt("Couldn't download {}: {}", &[&d.name, &e]))?;
            let pfx = prefix(&settings, d);
            fs::create_dir_all(&pfx).map_err(|e| e.to_string())?;
            let setup = CustomGame {
                id: format!("launcher-setup-{}", d.key),
                name: format!("{} setup", d.name),
                kind: GameKind::Windows,
                exe: dl.to_string_lossy().into(),
                proton_dir: proton,
                prefix: pfx.to_string_lossy().into(),
                ..Default::default()
            };
            let (mut child, log) = launch_game(&setup, &settings)?;
            let _ = child.wait();
            if launcher_exe(&settings, d).is_some() {
                Ok(lfmt("{} is installed", &[&d.name]))
            } else {
                Err(lfmt("{} setup ended before it finished — see {}", &[&d.name, &log.display()]))
            }
        });
    }

    pub fn page_launchers(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        page_header(ui, "Launchers", "Install Battle.net, the EA app, Ubisoft Connect or Rockstar into their own prefix. Games you install through them appear in your library and start through their launcher.");
        // Pick up new installs every few seconds while this page is open.
        if self.now - self.launchers_scanned > 5.0 {
            self.launchers_scanned = self.now;
            let mut added = Vec::new();
            for d in &ALL {
                let n = self.launchers_sync(ctx, d);
                if n > 0 {
                    added.push(if n == 1 { lfmt("1 game from {}", &[&d.name]) } else { lfmt("{} games from {}", &[&n, &d.name]) });
                }
            }
            if !added.is_empty() {
                self.set_status(lfmt("Added {} to your library", &[&added.join(", ")]), false);
            }
        }
        for d in &ALL {
            self.launcher_card(ui, ctx, d, acts);
            ui.add_space(12.0);
        }
        note(ui, 0.0, "Signing in happens inside each launcher, on its own window. Games with kernel anti-cheat (Valorant, some Call of Duty modes, GTA Online) don't run on Linux.");
    }

    fn launcher_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, d: &'static Def, acts: &mut Acts) {
        let installed = launcher_exe(&self.cfg.settings, d).is_some();
        let busy = self.tasks.iter().any(|t| t.key == format!("launcher:{}", d.key));
        let entry = self.launcher_entry(d.key);
        let games: Vec<usize> = (0..self.cfg.custom_games.len())
            .filter(|&i| self.cfg.custom_games[i].launcher.as_ref().is_some_and(|l| l.key == d.key && !l.watch.is_empty()))
            .collect();
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                let (cr, cg, cb) = d.color;
                ui.painter().rect_filled(r, rad(12), egui::Color32::from_rgb(cr, cg, cb));
                let letter: String = d.name.chars().next().map(String::from).unwrap_or_default();
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, letter, egui::FontId::proportional(24.0), egui::Color32::WHITE);
                ui.add_space(8.0);
                vcol(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(d.name).size(16.0).strong().color(pal().text));
                        if installed {
                            pill(ui, "Ready", pal().ok);
                        }
                    });
                    ui.label(egui::RichText::new(tr(d.about)).small().color(pal().subtext));
                });
                ui.with_layout(far_end(), |ui| {
                    if busy {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Installing… finish the setup window")).small().color(pal().subtext));
                    } else if let (true, Some(i)) = (installed, entry) {
                        let id = self.cfg.custom_games[i].id.clone();
                        ui.menu_button(tr("More"), |ui| menu_ui(ui, |ui| {
                            if ui.button(tr("Open prefix folder")).clicked() {
                                acts.game.push((id.clone(), GameAct::OpenPrefix));
                                ui.close();
                            }
                            if ui.button(tr("Look for new games")).clicked() {
                                self.launchers_scanned = 0.0;
                                ui.close();
                            }
                            ui.separator();
                            if ui.button(egui::RichText::new(tr("Remove…")).color(pal().danger)).clicked() {
                                self.launcher_remove = Some(d.key);
                                ui.close();
                            }
                        }));
                        let running = self.running.contains_key(&id);
                        let (label, act) = if running { ("Stop", GameAct::Stop) } else { ("Open", GameAct::Play) };
                        if primary_button(ui, label).clicked() {
                            acts.game.push((id, act));
                        }
                    } else if primary_button(ui, "Install").clicked() {
                        self.install_launcher(d);
                    }
                });
            });
            if installed && !games.is_empty() {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for &i in &games {
                        let g = &self.cfg.custom_games[i];
                        let r = ui.add(egui::Button::new(disp(&g.name)).small());
                        if r.clicked() {
                            acts.game.push((g.id.clone(), GameAct::Open));
                        }
                    }
                });
            } else if installed {
                ui.add_space(6.0);
                para(ui, "No games yet. Install some in the launcher; they show up here and in your library.", pal().faint, true);
            }
        });
        let _ = ctx;
    }

    /// Confirm removing a launcher: its prefix (with its games) goes to the Trash.
    pub fn launcher_remove_modal(&mut self, ctx: &egui::Context) {
        let Some(key) = self.launcher_remove else { return };
        let Some(d) = def(key) else { return };
        let (mut go, mut cancel) = (false, false);
        egui::Modal::new(egui::Id::unique("launcher-remove")).show(ctx, |ui| {
            dir_ui(ui, |ui| {
                ui.set_width(460.0);
                ui.label(egui::RichText::new(trf("Remove {}?", &[&d.name])).size(18.0).strong().color(pal().text));
                para(ui, "Its prefix moves to the Trash, with every game installed through it, and they leave your library. Saves the launcher keeps in the cloud stay there.", pal().subtext, true);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    go = danger_button(ui, "Remove").clicked();
                    cancel = ui.button(tr("Cancel")).clicked();
                });
            })
        });
        if cancel {
            self.launcher_remove = None;
        }
        if go {
            self.launcher_remove = None;
            let pfx = prefix(&self.cfg.settings, d);
            match cleanup::move_to_trash(&pfx) {
                Ok(()) => {
                    self.cfg.custom_games.retain(|g| g.launcher.as_ref().is_none_or(|l| l.key != key));
                    save_config(&self.cfg);
                    self.set_status(lfmt("{} removed", &[&d.name]), false);
                }
                Err(e) => self.set_status(e, true),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_folder_is_noticed() {
        let dir = std::env::temp_dir().join(format!("gamelo-watch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(!folder_busy(&dir, ""));
        let mut child = Command::new("sleep").arg("5").current_dir(&dir).spawn().unwrap();
        assert!(folder_busy(&dir, ""), "a process working in the folder counts");
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn registry_and_paths() {
        let reg = "[Software\\\\Wow6432Node\\\\Ubisoft\\\\Launcher\\\\Installs\\\\4311] 1700000000\n\"InstallDir\"=\"C:/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Far Cry 5/\"\n\n[Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\Overwatch] 1\n\"DisplayName\"=\"Overwatch\"\n\"UninstallString\"=\"\\\"C:\\\\ProgramData\\\\Battle.net\\\\Agent\\\\Blizzard Uninstaller.exe\\\" --uid=prometheus\"\n";
        let r = read_reg(reg);
        let ubi = &r["software\\wow6432node\\ubisoft\\launcher\\installs\\4311"];
        assert_eq!(ubi["installdir"], "C:/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Far Cry 5/");
        let ow = &r["software\\microsoft\\windows\\currentversion\\uninstall\\overwatch"];
        assert!(ow["uninstallstring"].contains("Blizzard Uninstaller.exe\" --uid=prometheus"));
        let p = Path::new("/pfx");
        assert_eq!(win_to_unix(p, "C:\\Program Files\\Rockstar Games\\GTAV\\"), PathBuf::from("/pfx/drive_c/Program Files/Rockstar Games/GTAV"));
        assert_eq!(win_to_unix(p, "D:/Games/X"), PathBuf::from("/pfx/dosdevices/d:/Games/X"));
    }
}
