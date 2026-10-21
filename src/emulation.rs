//! Emulation: ROM folders scanned into shelves by console, launched with the
//! emulators already installed (native or Flatpak, RetroArch or standalone).
//! ROM games are library entries (so playtime, art and search work) shown on
//! their own page rather than in the Library.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr};
use crate::theme::pal;
use crate::*;
use eframe::egui;
use std::collections::HashSet;

pub struct Platform {
    pub key: &'static str,
    pub name: &'static str,
    /// Folder names that mean this console (EmulationStation's layout and friends).
    folders: &'static [&'static str],
    /// File types only this console uses: recognised anywhere.
    unique: &'static [&'static str],
    /// File types shared with others: only inside this console's folder.
    shared: &'static [&'static str],
}

const CART: &[&str] = &["zip", "7z"];

pub const PLATFORMS: &[Platform] = &[
    Platform { key: "nes", name: "NES", folders: &["nes", "famicom"], unique: &["nes"], shared: CART },
    Platform { key: "snes", name: "Super Nintendo", folders: &["snes", "sfc", "superfamicom"], unique: &["sfc", "smc"], shared: CART },
    Platform { key: "n64", name: "Nintendo 64", folders: &["n64"], unique: &["n64", "z64", "v64"], shared: CART },
    Platform { key: "gb", name: "Game Boy", folders: &["gb", "gameboy"], unique: &["gb"], shared: CART },
    Platform { key: "gbc", name: "Game Boy Color", folders: &["gbc"], unique: &["gbc"], shared: CART },
    Platform { key: "gba", name: "Game Boy Advance", folders: &["gba"], unique: &["gba"], shared: CART },
    Platform { key: "nds", name: "Nintendo DS", folders: &["nds", "ds"], unique: &["nds"], shared: CART },
    Platform { key: "3ds", name: "Nintendo 3DS", folders: &["3ds", "n3ds"], unique: &["3ds", "cci", "cxi", "3dsx"], shared: &[] },
    Platform { key: "gc", name: "GameCube", folders: &["gc", "gamecube", "ngc"], unique: &["gcm"], shared: &["iso", "rvz", "ciso", "gcz"] },
    Platform { key: "wii", name: "Wii", folders: &["wii"], unique: &["wbfs"], shared: &["iso", "rvz", "ciso", "gcz"] },
    Platform { key: "wiiu", name: "Wii U", folders: &["wiiu"], unique: &["wua", "wux", "wud", "rpx"], shared: &[] },
    Platform { key: "switch", name: "Nintendo Switch", folders: &["switch"], unique: &["nsp", "xci"], shared: &[] },
    Platform { key: "psx", name: "PlayStation", folders: &["psx", "ps1", "playstation"], unique: &[], shared: &["cue", "chd", "pbp", "iso"] },
    Platform { key: "ps2", name: "PlayStation 2", folders: &["ps2"], unique: &[], shared: &["iso", "chd", "cso", "gz"] },
    Platform { key: "ps3", name: "PlayStation 3", folders: &["ps3"], unique: &[], shared: &["iso"] },
    Platform { key: "psp", name: "PSP", folders: &["psp"], unique: &[], shared: &["iso", "cso", "pbp", "chd"] },
    Platform { key: "genesis", name: "Mega Drive / Genesis", folders: &["genesis", "megadrive", "md"], unique: &["md", "gen", "smd"], shared: &["bin", "zip", "7z"] },
    Platform { key: "sms", name: "Master System", folders: &["mastersystem", "sms"], unique: &["sms"], shared: CART },
    Platform { key: "gg", name: "Game Gear", folders: &["gamegear", "gg"], unique: &["gg"], shared: CART },
    Platform { key: "dc", name: "Dreamcast", folders: &["dreamcast", "dc"], unique: &["gdi", "cdi"], shared: &["chd"] },
];

pub fn platform(key: &str) -> Option<&'static Platform> {
    PLATFORMS.iter().find(|p| p.key == key)
}

struct Emu {
    name: &'static str,
    native: &'static [&'static str],
    flatpak: &'static str,
    platforms: &'static [&'static str],
    /// Arguments, with {rom} for the game.
    args: &'static [&'static str],
}

/// Standalone emulators, preferred over RetroArch for the consoles they cover.
const EMUS: &[Emu] = &[
    Emu { name: "Dolphin", native: &["dolphin-emu"], flatpak: "org.DolphinEmu.dolphin-emu", platforms: &["gc", "wii"], args: &["-b", "-e", "{rom}"] },
    Emu { name: "Cemu", native: &["cemu", "Cemu"], flatpak: "info.cemu.Cemu", platforms: &["wiiu"], args: &["-g", "{rom}"] },
    Emu { name: "Ryubing", native: &["ryujinx", "Ryujinx"], flatpak: "io.github.ryubing.Ryujinx", platforms: &["switch"], args: &["{rom}"] },
    Emu { name: "Azahar", native: &["azahar", "lime3ds", "citra-qt"], flatpak: "org.azahar_emu.Azahar", platforms: &["3ds"], args: &["{rom}"] },
    Emu { name: "melonDS", native: &["melonDS"], flatpak: "net.kuribo64.melonDS", platforms: &["nds"], args: &["{rom}"] },
    Emu { name: "DuckStation", native: &["duckstation-qt", "duckstation"], flatpak: "org.duckstation.DuckStation", platforms: &["psx"], args: &["-batch", "-fullscreen", "{rom}"] },
    Emu { name: "PCSX2", native: &["pcsx2-qt", "pcsx2"], flatpak: "net.pcsx2.PCSX2", platforms: &["ps2"], args: &["-batch", "-fullscreen", "--", "{rom}"] },
    Emu { name: "RPCS3", native: &["rpcs3"], flatpak: "net.rpcs3.RPCS3", platforms: &["ps3"], args: &["--no-gui", "{rom}"] },
    Emu { name: "PPSSPP", native: &["PPSSPPSDL", "ppsspp", "PPSSPPQt"], flatpak: "org.ppsspp.PPSSPP", platforms: &["psp"], args: &["{rom}"] },
    Emu { name: "mGBA", native: &["mgba-qt", "mgba"], flatpak: "io.mgba.mGBA", platforms: &["gb", "gbc", "gba"], args: &["-f", "{rom}"] },
    Emu { name: "Flycast", native: &["flycast"], flatpak: "org.flycast.Flycast", platforms: &["dc"], args: &["{rom}"] },
    Emu { name: "Rosalie's Mupen GUI", native: &["RMG"], flatpak: "com.github.Rosalie241.RMG", platforms: &["n64"], args: &["{rom}"] },
];

/// RetroArch cores to try for each console, best first.
const CORES: &[(&str, &[&str])] = &[
    ("nes", &["mesen", "nestopia", "fceumm"]),
    ("snes", &["snes9x", "bsnes"]),
    ("n64", &["mupen64plus_next", "parallel_n64"]),
    ("gb", &["gambatte", "sameboy", "mgba"]),
    ("gbc", &["gambatte", "sameboy", "mgba"]),
    ("gba", &["mgba", "vba_next"]),
    ("nds", &["melonds", "desmume"]),
    ("genesis", &["genesis_plus_gx", "picodrive"]),
    ("sms", &["genesis_plus_gx", "picodrive"]),
    ("gg", &["genesis_plus_gx"]),
    ("psx", &["swanstation", "mednafen_psx_hw", "pcsx_rearmed"]),
    ("psp", &["ppsspp"]),
    ("dc", &["flycast"]),
];

/// What to suggest installing for a console with no emulator: (name, Flathub id).
pub fn suggestion(platform: &str) -> (&'static str, &'static str) {
    EMUS.iter().find(|e| e.platforms.contains(&platform)).map_or(("RetroArch", "org.libretro.RetroArch"), |e| (e.name, e.flatpak))
}

/// The emulators on this machine (looked up once per scan).
pub struct Installed {
    flatpaks: HashSet<String>,
    flatpak_bin: Option<PathBuf>,
    retroarch: Option<(PathBuf, Vec<String>)>,
    cores: Vec<PathBuf>,
}

impl Installed {
    pub fn detect() -> Installed {
        let flatpak_bin = find_in_path("flatpak");
        let flatpaks: HashSet<String> = flatpak_bin
            .as_ref()
            .and_then(|f| Command::new(f).args(["list", "--app", "--columns=application"]).output().ok())
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).collect())
            .unwrap_or_default();
        let h = home();
        let retroarch = find_in_path("retroarch").map(|p| (p, Vec::new())).or_else(|| {
            (flatpaks.contains("org.libretro.RetroArch"))
                .then(|| flatpak_bin.clone().map(|f| (f, vec!["run".to_string(), "org.libretro.RetroArch".to_string()])))
                .flatten()
        });
        let core_dirs = [
            h.join(".config/retroarch/cores"),
            h.join(".var/app/org.libretro.RetroArch/config/retroarch/cores"),
            PathBuf::from("/usr/lib/libretro"),
            PathBuf::from("/usr/lib64/libretro"),
            PathBuf::from("/usr/lib/x86_64-linux-gnu/libretro"),
        ];
        let cores = core_dirs.iter().flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path())).collect();
        Installed { flatpaks, flatpak_bin, retroarch, cores }
    }

    /// The command for `rom` on `platform`: (program, arguments, emulator name).
    pub fn command(&self, platform: &str, rom: &Path) -> Option<(PathBuf, String, &'static str)> {
        let rom_s = rom.to_string_lossy().to_string();
        let fill = |args: &[&str]| args.iter().map(|a| if *a == "{rom}" { shell_quote(&rom_s) } else { a.to_string() }).collect::<Vec<_>>().join(" ");
        for e in EMUS.iter().filter(|e| e.platforms.contains(&platform)) {
            if let Some(bin) = e.native.iter().find_map(|n| find_in_path(n)) {
                return Some((bin, fill(e.args), e.name));
            }
            if self.flatpaks.contains(e.flatpak)
                && let Some(f) = &self.flatpak_bin
            {
                return Some((f.clone(), format!("run {} {}", e.flatpak, fill(e.args)), e.name));
            }
        }
        let (bin, pre) = self.retroarch.as_ref()?;
        let names = CORES.iter().find(|(p, _)| *p == platform)?.1;
        let core = names.iter().find_map(|n| self.cores.iter().find(|c| c.file_name().is_some_and(|f| f.to_string_lossy() == format!("{n}_libretro.so"))))?;
        let mut args = pre.clone();
        args.extend(["-f".to_string(), "-L".to_string(), shell_quote(&core.to_string_lossy()), shell_quote(&rom_s)]);
        Some((bin.clone(), args.join(" "), "RetroArch"))
    }

    /// Every emulator found, by name (for the page's summary).
    pub fn names(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = EMUS
            .iter()
            .filter(|e| e.native.iter().any(|n| find_in_path(n).is_some()) || self.flatpaks.contains(e.flatpak))
            .map(|e| e.name)
            .collect();
        if self.retroarch.is_some() {
            out.push("RetroArch");
        }
        out
    }
}

/// A ROM in the library.
#[derive(Clone, Default, Serialize, Deserialize, Debug, PartialEq)]
pub struct RomLink {
    pub platform: String,
    pub rom: String,
}

/// "Super Mario World (USA) [!]" → "Super Mario World".
pub fn clean_name(stem: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in stem.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(if c == '_' { ' ' } else { c }),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The console for a file under `root`, from its folders or its file type.
pub fn platform_for(root: &Path, file: &Path) -> Option<&'static Platform> {
    let ext = file.extension()?.to_string_lossy().to_lowercase();
    let folders: Vec<String> = file.strip_prefix(root).ok()?.parent()?.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    for f in folders.iter().rev() {
        if let Some(p) = PLATFORMS.iter().find(|p| p.folders.contains(&f.as_str())) {
            return (p.unique.contains(&ext.as_str()) || p.shared.contains(&ext.as_str())).then_some(p);
        }
    }
    PLATFORMS.iter().find(|p| p.unique.contains(&ext.as_str()))
}

/// ROMs under the folders: (console, file, name). PS3 games are folders with
/// PS3_GAME/USRDIR/EBOOT.BIN.
pub fn scan(dirs: &[String]) -> Vec<(&'static Platform, PathBuf, String)> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<(&'static Platform, PathBuf, String)>) {
        let eboot = dir.join("PS3_GAME/USRDIR/EBOOT.BIN");
        if eboot.is_file() {
            let name = dir.file_name().map(|n| clean_name(&n.to_string_lossy())).unwrap_or_default();
            out.push((platform("ps3").unwrap_or(&PLATFORMS[0]), eboot, name));
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                if depth < 4 {
                    walk(root, &p, depth + 1, out);
                }
            } else if let Some(pl) = platform_for(root, &p) {
                let name = p.file_stem().map(|s| clean_name(&s.to_string_lossy())).unwrap_or_default();
                out.push((pl, p, name));
            }
        }
    }
    for d in dirs {
        let root = Path::new(d);
        walk(root, root, 0, &mut out);
    }
    out
}

#[derive(Default)]
pub struct EmuState {
    pub scanned: bool,
    pub emulators: Vec<&'static str>,
    /// Consoles with games but no emulator.
    pub missing: Vec<&'static str>,
}

impl App {
    /// Scan the ROM folders: add new games, refresh every ROM game's command
    /// (an emulator may have been installed since).
    pub fn emulation_sync(&mut self, ctx: &egui::Context) {
        let inst = Installed::detect();
        let found = scan(&self.cfg.settings.rom_dirs);
        let mut new_ids = Vec::new();
        let mut missing: Vec<&'static str> = Vec::new();
        for (pl, rom, name) in found {
            let rom_s = rom.to_string_lossy().to_string();
            let cmd = inst.command(pl.key, &rom);
            if cmd.is_none() && !missing.contains(&pl.key) {
                missing.push(pl.key);
            }
            let (exe, args) = cmd.map(|(b, a, _)| (b.to_string_lossy().to_string(), a)).unwrap_or_default();
            match self.cfg.custom_games.iter_mut().find(|g| g.rom.as_ref().is_some_and(|r| r.rom == rom_s)) {
                Some(g) => {
                    g.exe = exe;
                    g.args = args;
                }
                None => {
                    let g = CustomGame {
                        id: new_game_id(),
                        name,
                        kind: GameKind::Linux,
                        exe,
                        args,
                        rom: Some(RomLink { platform: pl.key.into(), rom: rom_s }),
                        added: unix_now(),
                        ..Default::default()
                    };
                    new_ids.push(g.id.clone());
                    self.cfg.custom_games.push(g);
                }
            }
        }
        save_config(&self.cfg);
        if !new_ids.is_empty() {
            self.set_status(if new_ids.len() == 1 { lg("Added 1 game from your ROM folders").to_string() } else { lfmt("Added {} games from your ROM folders", &[&new_ids.len()]) }, false);
            if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
                self.auto_fetch(ctx, new_ids);
            }
        }
        self.emu = EmuState { scanned: true, emulators: inst.names(), missing };
    }

    pub fn page_emulation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        if !self.emu.scanned {
            self.emulation_sync(ctx);
        }
        ui.horizontal(|ui| {
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(tr("Emulation")).size(24.0).strong().color(pal().text));
                let n = self.cfg.custom_games.iter().filter(|g| g.rom.is_some()).count();
                let sub = if n == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&n]) };
                ui.label(egui::RichText::new(disp(&sub)).color(pal().subtext).small());
            });
            ui.with_layout(far_end(), |ui| {
                if primary_button(ui, "Add ROM folder…").clicked()
                    && let Some(p) = pick_folder(lg("Folder with your ROMs"))
                {
                    let p = p.to_string_lossy().to_string();
                    if !self.cfg.settings.rom_dirs.contains(&p) {
                        self.cfg.settings.rom_dirs.push(p);
                    }
                    self.emulation_sync(ctx);
                }
                if !self.cfg.settings.rom_dirs.is_empty() && ui.button(tr("Scan again")).clicked() {
                    self.emulation_sync(ctx);
                }
            });
        });
        ui.add_space(14.0);
        if self.cfg.settings.rom_dirs.is_empty() {
            empty_hint(ui, "Add a folder with your ROMs. Games are sorted by console using subfolders named like snes, ps2 or gba (the usual EmulationStation layout), or by file type when only one console uses it.");
            return;
        }
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Emulators")).strong().color(pal().text));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if self.emu.emulators.is_empty() {
                    ui.label(egui::RichText::new(tr("None found yet.")).color(pal().subtext));
                }
                for e in &self.emu.emulators {
                    pill(ui, e, pal().ok);
                }
            });
            for key in self.emu.missing.clone() {
                let (emu, id) = suggestion(key);
                let pname = platform(key).map_or(key, |p| p.name);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(disp(&lfmt("No emulator for {}: install {}", &[&pname, &emu]))).color(pal().warn).small());
                    let cmd = format!("flatpak install flathub {id}");
                    if ui.small_button(tr("Copy command")).on_hover_text(cmd.clone()).clicked() {
                        acts.copy = Some(cmd);
                    }
                });
            }
            ui.add_space(8.0);
            ui.label(egui::RichText::new(tr("ROM folders")).small().color(pal().faint));
            let mut remove = None;
            for (k, d) in self.cfg.settings.rom_dirs.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(d).monospace().small().color(pal().subtext));
                    if ui.small_button(tr("Remove")).clicked() {
                        remove = Some(k);
                    }
                });
            }
            if let Some(k) = remove {
                let dir = self.cfg.settings.rom_dirs.remove(k);
                // Its games leave too (their playtime stays in the stats).
                self.cfg.custom_games.retain(|g| g.rom.as_ref().is_none_or(|r| !r.rom.starts_with(&dir)));
                save_config(&self.cfg);
            }
        });
        ui.add_space(18.0);
        for pl in PLATFORMS {
            let mut games: Vec<usize> = (0..self.cfg.custom_games.len()).filter(|&i| self.cfg.custom_games[i].rom.as_ref().is_some_and(|r| r.platform == pl.key)).collect();
            if games.is_empty() {
                continue;
            }
            games.sort_by_key(|&i| self.cfg.custom_games[i].name.to_lowercase());
            let count = if games.len() == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&games.len()]) };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(pl.name).size(17.0).strong().color(pal().text));
                ui.label(egui::RichText::new(disp(&count)).small().color(pal().faint));
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(16.0, 14.0);
                for &i in &games {
                    self.small_poster(ui, ctx, i, egui::vec2(124.0, 186.0), "", acts);
                }
            });
            ui.add_space(22.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roms_are_sorted_by_console() {
        assert_eq!(clean_name("Super Mario World (USA) [!]"), "Super Mario World");
        assert_eq!(clean_name("Metroid_Fusion (Europe) (En,Fr)"), "Metroid Fusion");
        let root = Path::new("/roms");
        let key = |p: &str| platform_for(root, Path::new(p)).map(|p| p.key);
        assert_eq!(key("/roms/smw.sfc"), Some("snes"));
        assert_eq!(key("/roms/ps2/God of War.iso"), Some("ps2"));
        assert_eq!(key("/roms/Wii/Mario Kart.rvz"), Some("wii"));
        assert_eq!(key("/roms/loose.iso"), None, "a shared type needs its console's folder");
        assert_eq!(key("/roms/snes/readme.txt"), None);
        assert_eq!(key("/roms/gba/pack/Zelda.zip"), Some("gba"));
    }
}
