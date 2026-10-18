//! Keeping the library safe: config saves are atomic and backed up daily, a
//! damaged config is set aside and restored from the newest backup instead
//! of starting empty, and a crash leaves a log and a notice for next time.

use crate::i18n::{fmt_logical as lfmt, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;

fn backups_dir() -> PathBuf {
    data_dir().join("backups")
}

fn crash_log() -> PathBuf {
    data_dir().join("crash.log")
}

/// Write `text` to `path` through a temporary file, so a crash mid-write
/// never leaves half a file. `private` makes it readable by the user only.
pub fn write_atomic(path: &Path, text: &str, private: bool) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    if private {
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)
}

/// Once a day, copy the config about to be replaced into backups/, keeping
/// the newest seven.
pub fn daily_backup(config: &Path) {
    if !config.is_file() {
        return;
    }
    let dir = backups_dir();
    let (y, m, d) = sessions::civil(sessions::day_of(unix_now()));
    let today = dir.join(format!("config-{y:04}-{m:02}-{d:02}.json"));
    if today.exists() || fs::create_dir_all(&dir).is_err() {
        return;
    }
    if fs::copy(config, &today).is_ok() {
        let _ = fs::set_permissions(&today, fs::Permissions::from_mode(0o600));
    }
    let mut all = backups();
    while all.len() > 7 {
        let _ = fs::remove_file(all.remove(0));
    }
}

/// Config backups, oldest first.
pub fn backups() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(backups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("config-")))
        .collect();
    v.sort();
    v
}

/// What happened when the config was read.
pub enum Loaded {
    Fine(AppConfig),
    /// No config yet (first run).
    Fresh,
    /// The config couldn't be read: it was moved aside, and this backup (if
    /// any) was used instead.
    Restored(AppConfig, Option<PathBuf>, PathBuf),
}

pub fn load(path: &Path) -> Loaded {
    let Ok(text) = fs::read_to_string(path) else { return Loaded::Fresh };
    if let Ok(cfg) = serde_json::from_str(&text) {
        return Loaded::Fine(cfg);
    }
    let aside = path.with_extension(format!("broken-{}.json", unix_now()));
    let _ = fs::rename(path, &aside);
    for b in backups().into_iter().rev() {
        if let Some(cfg) = fs::read_to_string(&b).ok().and_then(|t| serde_json::from_str(&t).ok()) {
            let _ = fs::copy(&b, path);
            return Loaded::Restored(cfg, Some(b), aside);
        }
    }
    Loaded::Restored(AppConfig::default(), None, aside)
}

/// Write a crash log when Gamelo panics (the window or a game's runner).
pub fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let what = if std::env::args().any(|a| a == "--run") { "game runner" } else { "window" };
        let text = format!("Gamelo {} crashed ({what}) at unix time {}\n\n{info}\n\n{bt}\n", env!("CARGO_PKG_VERSION"), unix_now());
        let _ = fs::create_dir_all(data_dir());
        let _ = fs::write(crash_log(), text);
        default(info);
    }));
}

/// The crash log, if it's from after the last one the user saw.
pub fn unseen_crash(seen: u64) -> Option<u64> {
    let modified = fs::metadata(crash_log()).ok()?.modified().ok()?;
    let at = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    (at > seen).then_some(at)
}

impl App {
    /// A banner about a crash last time, or a config restored from backup.
    pub fn safety_banner(&mut self, ui: &mut egui::Ui) {
        let Some(text) = self.safety_notice.clone() else { return };
        let crash = self.crash_at.is_some();
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(14, 10))
            .corner_radius(rad(10))
            .fill(pal().warn.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, pal().warn.gamma_multiply(0.5)))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::i18n::disp(&text)).color(pal().warn));
                    ui.with_layout(far_end(), |ui| {
                        if ui.button(tr("Dismiss")).clicked() {
                            self.safety_notice = None;
                            if let Some(at) = self.crash_at.take() {
                                self.cfg.settings.crash_seen = at;
                                save_config(&self.cfg);
                            }
                        }
                        if crash && ui.button(tr("View crash log")).clicked() {
                            let path = crash_log();
                            let (lines, hints) = extras::read_log(&path);
                            self.extras.log = Some(extras::LogView { title: "Gamelo".into(), path, lines, hints, errors_only: false });
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    /// Set the banner for what happened at start-up.
    pub fn startup_notices(&mut self, restored: Option<(Option<PathBuf>, PathBuf)>) {
        if let Some((backup, aside)) = restored {
            let aside = aside.display().to_string();
            self.safety_notice = Some(match backup {
                Some(b) => lfmt("Your settings file was damaged, so Gamelo restored the backup {} (the damaged file is kept as {}).", &[&b.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), &aside]),
                None => lfmt("Your settings file was damaged and there was no backup yet; it's kept as {}.", &[&aside]),
            });
        } else if let Some(at) = unseen_crash(self.cfg.settings.crash_seen) {
            self.crash_at = Some(at);
            self.safety_notice = Some(crate::i18n::logical("Gamelo closed unexpectedly last time. The crash log can help fix it.").to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_config_is_restored_not_lost() {
        let dir = std::env::temp_dir().join(format!("gamelo-safety-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let cfg = AppConfig { custom_games: vec![CustomGame { id: "a".into(), name: "Hades".into(), ..Default::default() }], ..Default::default() };
        let good = serde_json::to_string(&cfg).unwrap();
        write_atomic(&path, &good, true).unwrap();
        assert!(matches!(load(&path), Loaded::Fine(c) if c.custom_games.len() == 1));
        fs::write(&path, &good[..good.len() / 2]).unwrap();
        // No backups in this folder's data dir are assumed: whatever load
        // restores, the damaged file must be kept, never overwritten.
        match load(&path) {
            Loaded::Restored(_, _, aside) => assert!(fs::read_to_string(aside).unwrap().starts_with("{\"custom_games\"")),
            _ => panic!("a damaged config must not load as fine or fresh"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
