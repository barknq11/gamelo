//! The Disk space page: what ProtonLaunch's files take up, and safe ways to
//! free it. Prefixes left behind by removed games go to the desktop Trash
//! (they can hold save files); caches and leftovers that rebuild themselves
//! are deleted.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, tr, trf};
use crate::theme::pal;
use crate::*;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

pub type Sizes = Arc<Mutex<HashMap<PathBuf, Option<u64>>>>;

pub struct CleanupState {
    /// Measured sizes (None while measuring).
    pub sizes: Sizes,
    /// Paths being trashed or cleared.
    pub busy: HashSet<PathBuf>,
    pub tx: Sender<(PathBuf, Result<String, String>)>,
    pub rx: Receiver<(PathBuf, Result<String, String>)>,
}

impl Default for CleanupState {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        CleanupState { sizes: Arc::default(), busy: HashSet::new(), tx, rx }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn cache_home() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"))
}

/// Move `path` to the desktop Trash: through `gio trash` when available, else
/// by the freedesktop.org Trash layout (same disk only).
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    if find_in_path("gio").is_some()
        && Command::new("gio").arg("trash").arg(path).status().is_ok_and(|s| s.success())
    {
        return Ok(());
    }
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    trash_manually(path, &data.join("Trash"))
}

/// The freedesktop.org Trash by hand: the item under files/, a .trashinfo
/// record of where it came from under info/.
fn trash_manually(path: &Path, trash: &Path) -> Result<(), String> {
    fs::create_dir_all(trash.join("files")).map_err(|e| e.to_string())?;
    fs::create_dir_all(trash.join("info")).map_err(|e| e.to_string())?;
    let base = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "item".into());
    let mut name = base.clone();
    let mut n = 1;
    while trash.join("files").join(&name).exists() || trash.join("info").join(format!("{name}.trashinfo")).exists() {
        n += 1;
        name = format!("{base}.{n}");
    }
    let encoded: String = path
        .to_string_lossy()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    let date = Command::new("date").arg("+%Y-%m-%dT%H:%M:%S").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    fs::write(trash.join("info").join(format!("{name}.trashinfo")), format!("[Trash Info]\nPath={encoded}\nDeletionDate={date}\n")).map_err(|e| e.to_string())?;
    fs::rename(path, trash.join("files").join(&name)).map_err(|e| {
        let _ = fs::remove_file(trash.join("info").join(format!("{name}.trashinfo")));
        lfmt("Couldn't move it to the Trash: {}", &[&e])
    })
}

/// Empty a folder (it is recreated empty).
fn clear_dir(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// When a folder was last touched, as "3 days ago".
fn last_used(path: &Path) -> String {
    let newest = ["pfx.lock", "user.reg", "system.reg"]
        .iter()
        .filter_map(|f| fs::metadata(path.join(f)).ok()?.modified().ok())
        .chain(fs::metadata(path).ok().and_then(|m| m.modified().ok()))
        .max();
    newest
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| format_ago(d.as_secs(), unix_now()))
        .unwrap_or_default()
}

impl App {
    /// A folder's size, measured once in the background.
    fn measured(&mut self, ctx: &egui::Context, path: &Path) -> Option<u64> {
        let mut map = self.cleanup.sizes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = map.get(path) {
            return *v;
        }
        map.insert(path.to_path_buf(), None);
        let (sizes, path, ctx) = (self.cleanup.sizes.clone(), path.to_path_buf(), ctx.clone());
        std::thread::spawn(move || {
            let n = if path.is_dir() { dir_size(&path) } else { fs::metadata(&path).map_or(0, |m| m.len()) };
            sizes.lock().unwrap_or_else(|e| e.into_inner()).insert(path, Some(n));
            ctx.request_repaint();
        });
        None
    }

    fn cleanup_job(&mut self, path: PathBuf, work: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if !self.cleanup.busy.insert(path.clone()) {
            return;
        }
        let tx = self.cleanup.tx.clone();
        let ctx = self.stores_ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((path, work()));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    pub fn cleanup_poll(&mut self) {
        while let Ok((path, r)) = self.cleanup.rx.try_recv() {
            self.cleanup.busy.remove(&path);
            self.cleanup.sizes.lock().unwrap_or_else(|e| e.into_inner()).remove(&path);
            match r {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(e, true),
            }
        }
    }

    /// Prefixes in the prefix folder that no game uses any more.
    pub(crate) fn orphan_prefixes(&self) -> Vec<PathBuf> {
        let used: HashSet<PathBuf> = self.cfg.custom_games.iter().map(|g| game_prefix(g, &self.cfg.settings)).collect();
        let mut out: Vec<PathBuf> = fs::read_dir(self.cfg.settings.prefix_base())
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !used.contains(p)).collect())
            .unwrap_or_default();
        out.sort();
        out
    }

    /// Proton builds we installed that no game (nor the default) uses.
    fn unused_protons(&self) -> Vec<(String, PathBuf)> {
        self.proton_tools
            .iter()
            .filter(|t| t.dir.to_string_lossy().contains("compatibilitytools.d"))
            .filter(|t| {
                let d = t.dir.to_string_lossy();
                !self.cfg.custom_games.iter().any(|g| g.proton_dir == d) && self.cfg.settings.default_proton != d
            })
            .map(|t| (t.label.clone(), t.dir.clone()))
            .collect()
    }

    pub fn page_cleanup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        page_header(ui, "Disk space", "What ProtonLaunch's files take up, and what's safe to free");
        let playing = !self.running.is_empty();
        let orphans = self.orphan_prefixes();
        let protons = self.unused_protons();
        let mut freeable = 0u64;

        // Left-behind prefixes.
        let mut trash: Option<PathBuf> = None;
        section_sub(ui, "Prefixes no game uses", "Left behind by games you removed. They can hold save files, so they go to the Trash and can be restored from there.", |ui| {
            if orphans.is_empty() {
                ui.label(egui::RichText::new(tr("None — every prefix belongs to a game.")).color(pal().subtext));
            }
            for p in &orphans {
                let size = self.measured(ctx, p);
                freeable += size.unwrap_or(0);
                let busy = self.cleanup.busy.contains(p);
                ui.horizontal(|ui| {
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).color(pal().text));
                        let detail = format!("{} · {}", size.map_or_else(|| "…".into(), format_bytes), last_used(p));
                        ui.label(egui::RichText::new(disp(&detail)).small().color(pal().subtext));
                    });
                    ui.with_layout(far_end(), |ui| {
                        if busy {
                            ui.spinner();
                        } else {
                            if ui.button(tr("Move to Trash")).clicked() {
                                trash = Some(p.clone());
                            }
                            if ui.button(tr("Open")).clicked() {
                                acts.open = Some(p.to_string_lossy().into());
                            }
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(p) = trash {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.cleanup_job(p.clone(), move || move_to_trash(&p).map(|_| lfmt("Moved the {} prefix to the Trash", &[&name])));
        }
        ui.add_space(12.0);

        // Proton builds nothing uses.
        let mut delete: Option<PathBuf> = None;
        section_sub(ui, "Proton builds no game uses", "They can be downloaded again any time from the Proton page.", |ui| {
            if protons.is_empty() {
                ui.label(egui::RichText::new(tr("None — every installed build is in use.")).color(pal().subtext));
            }
            for (label, dir) in &protons {
                let size = self.measured(ctx, dir);
                freeable += size.unwrap_or(0);
                ui.horizontal(|ui| {
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(label).color(pal().text));
                        ui.label(egui::RichText::new(size.map_or_else(|| "…".into(), format_bytes)).small().color(pal().subtext));
                    });
                    ui.with_layout(far_end(), |ui| {
                        if ui.button(tr("Delete…")).clicked() {
                            delete = Some(dir.clone());
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(dir) = delete {
            // The Proton page's own confirmation dialog.
            self.proton.confirm_delete = Some(dir);
        }
        ui.add_space(12.0);

        // Caches that rebuild themselves.
        let prefix_caches: Vec<PathBuf> = fs::read_dir(self.cfg.settings.prefix_base())
            .map(|rd| rd.flatten().map(|e| e.path().join("shadercache")).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        let caches: Vec<(&str, Vec<PathBuf>)> = vec![
            ("Mesa (AMD/Intel) shader cache", vec![cache_home().join("mesa_shader_cache"), cache_home().join("mesa_shader_cache_db")]),
            ("NVIDIA shader cache", vec![cache_home().join("nvidia/GLCache")]),
            ("DXVK/VKD3D caches in prefixes", prefix_caches),
        ];
        let mut clear: Option<Vec<PathBuf>> = None;
        section_sub(ui, "Shader caches", "Rebuilt as you play; the first launch after clearing may stutter for a while.", |ui| {
            for (label, paths) in &caches {
                let existing: Vec<PathBuf> = paths.iter().filter(|p| p.is_dir()).cloned().collect();
                if existing.is_empty() {
                    continue;
                }
                let sizes: Vec<Option<u64>> = existing.iter().map(|p| self.measured(ctx, p)).collect();
                let total: u64 = sizes.iter().flatten().sum();
                let known = sizes.iter().all(Option::is_some);
                freeable += total;
                let busy = existing.iter().any(|p| self.cleanup.busy.contains(p));
                ui.horizontal(|ui| {
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(tr(label)).color(pal().text));
                        ui.label(egui::RichText::new(if known { format_bytes(total) } else { "…".into() }).small().color(pal().subtext));
                    });
                    ui.with_layout(far_end(), |ui| {
                        if busy {
                            ui.spinner();
                        } else {
                            ui.add_enabled_ui(!playing && total > 0, |ui| {
                                if ui.button(tr("Clear")).on_disabled_hover_text(tr("Not while a game is running")).clicked() {
                                    clear = Some(existing.clone());
                                }
                            });
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(paths) = clear {
            for p in paths {
                let q = p.clone();
                self.cleanup_job(p, move || clear_dir(&q).map(|_| lg("Cleared").to_string()));
            }
        }
        ui.add_space(12.0);

        // ProtonLaunch's own leftovers.
        let leftovers = [
            ("Interrupted downloads", data_dir().join("downloads")),
            ("Game logs", data_dir().join("logs")),
            ("Store screenshots and images", data_dir().join("cache/store")),
        ];
        let mut wipe: Option<PathBuf> = None;
        section(ui, "Leftovers", |ui| {
            for (label, p) in &leftovers {
                if !p.is_dir() {
                    continue;
                }
                let size = self.measured(ctx, p);
                freeable += size.unwrap_or(0);
                let busy = self.cleanup.busy.contains(p);
                ui.horizontal(|ui| {
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(tr(label)).color(pal().text));
                        ui.label(egui::RichText::new(size.map_or_else(|| "…".into(), format_bytes)).small().color(pal().subtext));
                    });
                    ui.with_layout(far_end(), |ui| {
                        if busy {
                            ui.spinner();
                        } else {
                            ui.add_enabled_ui(!playing && size.is_some_and(|s| s > 0), |ui| {
                                if ui.button(tr("Clear")).on_disabled_hover_text(tr("Not while a game is running")).clicked() {
                                    wipe = Some(p.clone());
                                }
                            });
                        }
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(p) = wipe {
            let q = p.clone();
            self.cleanup_job(p, move || clear_dir(&q).map(|_| lg("Cleared").to_string()));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(trf("Up to {} can be freed here.", &[&format_bytes(freeable)])).strong().color(pal().text));
            ui.with_layout(far_end(), |ui| {
                if ui.button(tr("Measure again")).clicked() {
                    self.cleanup.sizes.lock().unwrap_or_else(|e| e.into_inner()).clear();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_keeps_a_restorable_record() {
        let root = std::env::temp_dir().join(format!("plaunch-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let item = root.join("old prefix");
        fs::create_dir_all(item.join("drive_c")).unwrap();
        trash_manually(&item, &root.join("Trash")).unwrap();
        assert!(!item.exists());
        assert!(root.join("Trash/files/old prefix/drive_c").is_dir());
        let info = fs::read_to_string(root.join("Trash/info/old prefix.trashinfo")).unwrap();
        assert!(info.contains("Path=") && info.contains("old%20prefix"), "{info}");
        let _ = fs::remove_dir_all(&root);
    }
}
