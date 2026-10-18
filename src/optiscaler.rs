//! OptiScaler: download releases and install them into a game's folder (and
//! take them out again cleanly), the way its own setup script does.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::proton::{self, Release};
use crate::theme::pal;
use crate::*;
use eframe::egui;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

pub const REPO: &str = "optiscaler/OptiScaler";

/// Names OptiScaler.dll can be installed under (the game must load one of them).
pub const DLL_NAMES: &[&str] = &["dxgi.dll", "winmm.dll", "version.dll", "dbghelp.dll", "d3d12.dll", "wininet.dll", "winhttp.dll"];

/// Files OptiScaler creates while running; removed with it.
const RUNTIME_FILES: &[&str] = &["OptiScaler.log", "fakenvapi.log", "dlssg_to_fsr3.log", "DlssOverrides"];

/// Release files that aren't meant to be copied into the game.
fn skip_file(name: &str) -> bool {
    let l = name.to_lowercase();
    l.starts_with("setup_") || l.contains("readme") || l == "remove_optiscaler.sh"
}

fn root() -> PathBuf {
    data_dir().join("optiscaler")
}

/// Downloaded (unpacked) versions, newest first by name.
pub fn downloaded() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root().join("versions"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("OptiScaler.dll").is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.cmp(a));
    v
}

fn version_dir(tag: &str) -> PathBuf {
    root().join("versions").join(tag)
}

/// Unpack a .7z with whatever tool is installed.
fn extract_7z(archive: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for tool in ["7z", "7za", "7zz"] {
        if find_in_path(tool).is_some() {
            let ok = Command::new(tool)
                .arg("x")
                .arg("-y")
                .arg(format!("-o{}", dest.display()))
                .arg(archive)
                .stdout(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?
                .success();
            return if ok { Ok(()) } else { Err(lg("Unpacking the archive failed").to_string()) };
        }
    }
    if find_in_path("bsdtar").is_some() {
        let ok = Command::new("bsdtar").arg("-xf").arg(archive).arg("-C").arg(dest).status().map_err(|e| e.to_string())?.success();
        return if ok { Ok(()) } else { Err(lg("Unpacking the archive failed").to_string()) };
    }
    Err(lg("Unpacking OptiScaler needs 7-Zip — install the p7zip (or 7zip) package").to_string())
}

pub fn sevenzip_available() -> bool {
    ["7z", "7za", "7zz", "bsdtar"].iter().any(|t| find_in_path(t).is_some())
}

/// The release's .7z archive.
pub fn archive_of(rel: &Release) -> Option<&proton::Asset> {
    rel.assets.iter().find(|a| a.name.ends_with(".7z"))
}

pub fn archive_path(name: &str) -> PathBuf {
    data_dir().join("downloads").join(name)
}

/// Download and unpack a release into versions/<tag>.
pub fn download(rel: &Release) -> Result<String, String> {
    let asset = archive_of(rel).ok_or_else(|| lg("That release has no download").to_string())?;
    let archive = archive_path(&asset.name);
    fs::create_dir_all(archive.parent().unwrap_or(&archive)).map_err(|e| e.to_string())?;
    http_fetch(&asset.url, Some(&archive))?;
    let staging = root().join(format!(".unpack-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let r = extract_7z(&archive, &staging);
    let _ = fs::remove_file(&archive);
    r?;
    let dll = find_file_recursive(&staging, "OptiScaler.dll").ok_or_else(|| lg("The archive did not contain OptiScaler.dll").to_string())?;
    let top = dll.parent().unwrap_or(&staging).to_path_buf();
    let dest = version_dir(&rel.tag);
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(dest.parent().unwrap_or(&dest)).map_err(|e| e.to_string())?;
    fs::rename(&top, &dest).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&staging);
    Ok(lfmt("Downloaded OptiScaler {}", &[&rel.tag]))
}

/// What was installed into a game, so it can be removed exactly.
#[derive(Clone, Serialize, Deserialize)]
pub struct Install {
    pub version: String,
    pub dll: String,
    pub dir: String,
    /// Files and folders we put in the game folder (relative to `dir`).
    pub files: Vec<String>,
    /// Game files we replaced, saved under the backup folder (relative names).
    pub replaced: Vec<String>,
}

fn install_record(root: &Path, id: &str) -> PathBuf {
    root.join("installs").join(format!("{id}.json"))
}

fn backup_dir(root: &Path, id: &str) -> PathBuf {
    root.join("backups").join(id)
}

fn installed_in(root: &Path, id: &str) -> Option<Install> {
    serde_json::from_str(&fs::read_to_string(install_record(root, id)).ok()?).ok()
}

pub fn installed(id: &str) -> Option<Install> {
    installed_in(&root(), id)
}

/// The DLL override the game needs while OptiScaler is installed.
pub fn dll_override(id: &str) -> Option<String> {
    let i = installed(id)?;
    let stem = i.dll.strip_suffix(".dll")?;
    Some(format!("{stem}=n,b"))
}

/// An NVIDIA GPU is present (no spoofing needed).
pub fn has_nvidia() -> bool {
    let Ok(rd) = fs::read_dir("/sys/bus/pci/devices") else { return false };
    rd.flatten().any(|e| {
        let read = |f: &str| fs::read_to_string(e.path().join(f)).unwrap_or_default();
        read("vendor").trim() == "0x10de" && read("class").trim().starts_with("0x03")
    })
}

/// Where OptiScaler should go: the folder of the game's real exe. For Unreal
/// games picked by their launcher, that's <Project>/Binaries/Win64.
pub fn target_dir(g: &CustomGame) -> Option<PathBuf> {
    let dir = Path::new(&g.exe).parent()?.to_path_buf();
    if dir.join("Engine").is_dir()
        && let Ok(rd) = fs::read_dir(&dir)
    {
        for e in rd.flatten() {
            let win64 = e.path().join("Binaries/Win64");
            let shipping = fs::read_dir(&win64).ok().is_some_and(|r| {
                r.flatten().any(|f| f.file_name().to_string_lossy().to_lowercase().ends_with(".exe"))
            });
            if e.file_name() != "Engine" && shipping {
                return Some(win64);
            }
        }
    }
    Some(dir)
}

/// Copy a downloaded version into the game folder as `dll`.
pub fn install(id: &str, version: &str, dir: &Path, dll: &str, spoof: bool) -> Result<String, String> {
    install_in(&root(), id, version, dir, dll, spoof)
}

fn install_in(root: &Path, id: &str, version: &str, dir: &Path, dll: &str, spoof: bool) -> Result<String, String> {
    if installed_in(root, id).is_some() {
        uninstall_in(root, id)?;
    }
    let src = root.join("versions").join(version);
    if !src.join("OptiScaler.dll").is_file() {
        return Err(lg("That OptiScaler version isn't downloaded").to_string());
    }
    let backups = backup_dir(root, id);
    let _ = fs::remove_dir_all(&backups);
    let mut rec = Install { version: version.into(), dll: dll.into(), dir: dir.to_string_lossy().into(), files: Vec::new(), replaced: Vec::new() };
    for e in fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if skip_file(&name) {
            continue;
        }
        let target_name = if name == "OptiScaler.dll" { dll.to_string() } else { name };
        let target = dir.join(&target_name);
        // Keep the game's own copy of anything we replace (FSR DLLs, a dxgi.dll mod…).
        if target.exists() {
            fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
            fs::rename(&target, backups.join(&target_name)).map_err(|e| format!("{target_name}: {e}"))?;
            rec.replaced.push(target_name.clone());
        }
        copy_path(&e.path(), &target).map_err(|e| format!("{target_name}: {e}"))?;
        rec.files.push(target_name);
    }
    // Same as the official setup: without DLSS inputs, don't pretend to be NVIDIA.
    if !spoof {
        let ini = dir.join("OptiScaler.ini");
        if let Ok(text) = fs::read_to_string(&ini) {
            let _ = fs::write(&ini, text.replace("Dxgi=auto", "Dxgi=false"));
        }
    }
    fs::create_dir_all(root.join("installs")).map_err(|e| e.to_string())?;
    fs::write(install_record(root, id), serde_json::to_string_pretty(&rec).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(lfmt("OptiScaler {} installed as {}", &[&version, &dll]))
}

fn copy_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            copy_path(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Remove what `install` put in and put back what it replaced.
pub fn uninstall(id: &str) -> Result<String, String> {
    uninstall_in(&root(), id)
}

fn uninstall_in(root: &Path, id: &str) -> Result<String, String> {
    let rec = installed_in(root, id).ok_or_else(|| lg("OptiScaler isn't installed for this game").to_string())?;
    let dir = PathBuf::from(&rec.dir);
    for f in rec.files.iter().map(String::as_str).chain(RUNTIME_FILES.iter().copied()) {
        let p = dir.join(f);
        if p.is_dir() {
            let _ = fs::remove_dir_all(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
    let backups = backup_dir(root, id);
    for f in &rec.replaced {
        let _ = fs::rename(backups.join(f), dir.join(f));
    }
    let _ = fs::remove_dir_all(&backups);
    let _ = fs::remove_file(install_record(root, id));
    Ok(lg("OptiScaler removed — the game's own files are back").to_string())
}

pub enum Msg {
    Releases(Result<Vec<Release>, String>),
}

pub struct OptiState {
    pub releases: Option<Result<Vec<Release>, String>>,
    pub loading: bool,
    pub dll: String,
    pub spoof: bool,
    /// Picked version tag (None = newest).
    pub version: Option<String>,
    pub nvidia: bool,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
}

impl Default for OptiState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let nvidia = has_nvidia();
        OptiState { releases: None, loading: false, dll: "dxgi.dll".into(), spoof: !nvidia, version: None, nvidia, tx, rx }
    }
}

impl App {
    pub fn opti_poll(&mut self) {
        while let Ok(Msg::Releases(r)) = self.opti.rx.try_recv() {
            self.opti.loading = false;
            self.opti.releases = Some(r);
        }
    }

    fn opti_load(&mut self, ctx: &egui::Context) {
        if self.opti.loading || self.opti.releases.is_some() {
            return;
        }
        self.opti.loading = true;
        let (tx, ctx) = (self.opti.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Releases(proton::fetch_releases(REPO, false)));
            ctx.request_repaint();
        });
    }

    /// The OptiScaler card on a Windows game's page.
    pub fn opti_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, acts: &mut Acts) {
        let g = self.cfg.custom_games[i].clone();
        if g.kind != GameKind::Windows {
            return;
        }
        let rec = installed(&g.id);
        let task_key = format!("opti:{}", g.id);
        let busy = self.tasks.iter().find(|t| t.key == task_key).map(|t| t.fraction());
        let mut do_install: Option<(Option<Release>, String)> = None;
        let mut do_remove = false;
        section_sub(ui, "OptiScaler", "Swap a game's upscaler: FSR 4, XeSS or DLSS in any game that supports one of them, plus frame generation", |ui| {
            if let Some(f) = busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add(egui::ProgressBar::new(f).desired_width(220.0).show_percentage());
                });
                return;
            }
            match &rec {
                Some(r) => {
                    ui.horizontal(|ui| {
                        status_dot(ui, pal().ok, true);
                        ui.label(egui::RichText::new(trf("Installed: {} as {}", &[&r.version, &r.dll])).color(pal().ok));
                    });
                    ui.add(egui::Label::new(egui::RichText::new(&r.dir).color(pal().subtext).small().monospace()).truncate());
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr("Edit OptiScaler.ini")).clicked() {
                            acts.open = Some(Path::new(&r.dir).join("OptiScaler.ini").to_string_lossy().into());
                        }
                        if ui.button(tr("Open folder")).clicked() {
                            acts.open = Some(r.dir.clone());
                        }
                        if ui.button(egui::RichText::new(tr("Remove")).color(pal().danger)).clicked() {
                            do_remove = true;
                        }
                    });
                    note(ui, 0.0, &lfmt("Launches with WINEDLLOVERRIDES={} so the game loads it. In game, press Insert for the OptiScaler menu.", &[&dll_override(&g.id).unwrap_or_default()]));
                }
                None => {
                    self.opti_load(ctx);
                    let local = downloaded();
                    let remote: Vec<Release> = match &self.opti.releases {
                        Some(Ok(l)) => l.iter().filter(|r| archive_of(r).is_some()).take(8).cloned().collect(),
                        _ => Vec::new(),
                    };
                    let mut tags: Vec<String> = remote.iter().map(|r| r.tag.clone()).collect();
                    for t in &local {
                        if !tags.contains(t) {
                            tags.push(t.clone());
                        }
                    }
                    let chosen = self.opti.version.clone().filter(|v| tags.contains(v)).or_else(|| tags.first().cloned());
                    modal_row(ui, "Version", |ui| {
                        if tags.is_empty() {
                            if self.opti.loading {
                                ui.spinner();
                            } else if let Some(Err(e)) = &self.opti.releases {
                                ui.label(egui::RichText::new(disp(e)).color(pal().warn).small());
                            }
                            return;
                        }
                        let label = |t: &str| {
                            if local.iter().any(|l| l == t) { disp(&format!("{t}  ·  {}", lg("downloaded"))) } else { t.to_string() }
                        };
                        egui::ComboBox::from_id_salt("opti-version")
                            .selected_text(chosen.as_deref().map(label).unwrap_or_default())
                            .width(260.0)
                            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                for t in &tags {
                                    if ui.selectable_label(chosen.as_deref() == Some(t), label(t)).clicked() {
                                        self.opti.version = Some(t.clone());
                                    }
                                }
                            }));
                    });
                    modal_row(ui, "Install as", |ui| {
                        egui::ComboBox::from_id_salt("opti-dll").selected_text(&self.opti.dll).width(160.0).show_ui(ui, |ui| menu_ui(ui, |ui| {
                            for n in DLL_NAMES {
                                ui.selectable_value(&mut self.opti.dll, n.to_string(), *n);
                            }
                        }));
                        ui.label(egui::RichText::new(tr("dxgi.dll works for most games")).color(pal().subtext).small());
                    });
                    modal_row(ui, "GPU", |ui| {
                        if self.opti.nvidia {
                            ui.label(egui::RichText::new(tr("NVIDIA detected — no spoofing needed")).color(pal().subtext));
                        } else {
                            check(ui, &mut self.opti.spoof, tr("Use DLSS inputs (makes the game see an NVIDIA card; needed for DLSS frame generation)"));
                        }
                    });
                    let target = target_dir(&g);
                    if let Some(t) = &target {
                        note(ui, 128.0, &lfmt("Goes into {}", &[&t.display()]));
                    }
                    ui.add_space(6.0);
                    let can = chosen.is_some() && target.is_some() && sevenzip_available();
                    ui.horizontal(|ui| {
                        ui.add_space(128.0);
                        ui.add_enabled_ui(can, |ui| {
                            if primary_button(ui, "Install OptiScaler").clicked()
                                && let Some(tag) = &chosen
                            {
                                let rel = if local.contains(tag) { None } else { remote.iter().find(|r| &r.tag == tag).cloned() };
                                do_install = Some((rel, tag.clone()));
                            }
                        });
                        if !sevenzip_available() {
                            ui.label(egui::RichText::new(tr("Needs 7-Zip to unpack — install the p7zip package")).color(pal().warn).small());
                        }
                    });
                }
            }
        });
        if do_remove {
            let id = g.id.clone();
            self.start_task(TaskKind::OptiScaler, &task_key, lg("Removing OptiScaler"), None, move || uninstall(&id));
        }
        if let Some((rel, tag)) = do_install {
            let (id, dll, spoof) = (g.id.clone(), self.opti.dll.clone(), self.opti.spoof || self.opti.nvidia);
            let Some(dir) = target_dir(&g) else { return };
            let progress = rel.as_ref().and_then(archive_of).map(|a| (archive_path(&a.name), a.size));
            self.start_task(TaskKind::OptiScaler, &task_key, &lfmt("Installing OptiScaler {}", &[&tag]), progress, move || {
                if let Some(rel) = rel {
                    download(&rel)?;
                }
                install(&id, &tag, &dir, &dll, spoof)
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_and_remove_restore_the_game_folder() {
        let tmp = std::env::temp_dir().join(format!("plaunch-opti-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let root = tmp.join("data");
        let ver = root.join("versions/v1");
        fs::create_dir_all(ver.join("Licenses")).unwrap();
        fs::write(ver.join("OptiScaler.dll"), "opti").unwrap();
        fs::write(ver.join("OptiScaler.ini"), "Dxgi=auto\n").unwrap();
        fs::write(ver.join("amd_fidelityfx_dx12.dll"), "new fsr").unwrap();
        fs::write(ver.join("setup_linux.sh"), "x").unwrap();
        fs::write(ver.join("Licenses/a.txt"), "l").unwrap();
        let game = tmp.join("game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("amd_fidelityfx_dx12.dll"), "game fsr").unwrap();
        fs::write(game.join("game.exe"), "").unwrap();

        install_in(&root, "g1", "v1", &game, "dxgi.dll", false).unwrap();
        assert_eq!(fs::read_to_string(game.join("dxgi.dll")).unwrap(), "opti");
        assert!(!game.join("OptiScaler.dll").exists() && !game.join("setup_linux.sh").exists());
        assert_eq!(fs::read_to_string(game.join("amd_fidelityfx_dx12.dll")).unwrap(), "new fsr");
        assert_eq!(fs::read_to_string(game.join("OptiScaler.ini")).unwrap(), "Dxgi=false\n", "no spoofing");
        assert!(game.join("Licenses/a.txt").is_file());
        fs::write(game.join("OptiScaler.log"), "runtime").unwrap();

        uninstall_in(&root, "g1").unwrap();
        let mut left: Vec<String> = fs::read_dir(&game).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        left.sort();
        assert_eq!(left, vec!["amd_fidelityfx_dx12.dll", "game.exe"]);
        assert_eq!(fs::read_to_string(game.join("amd_fidelityfx_dx12.dll")).unwrap(), "game fsr", "original restored");
        assert!(installed_in(&root, "g1").is_none());
        let _ = fs::remove_dir_all(&tmp);
    }
}
