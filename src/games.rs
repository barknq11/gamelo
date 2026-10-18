//! Running, adding, editing and removing games.

use crate::*;

impl App {
    /// Poll running markers (about once a second) and report games that ended.
    pub(crate) fn poll_running(&mut self) {
        self.children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
        if self.last_scan.elapsed() < Duration::from_millis(900) {
            return;
        }
        self.last_scan = Instant::now();
        let now = scan_running();
        let ended: Vec<RunMarker> = self
            .running
            .values()
            .filter(|m| !now.contains_key(&m.id))
            .cloned()
            .collect();
        let started: Vec<String> = now
            .values()
            .filter(|m| !self.running.contains_key(&m.id))
            .map(|m| m.name.clone())
            .collect();
        self.running = now;
        for name in started {
            self.set_status(lfmt("{} is running", &[&name]), false);
        }
        // Started games stop "starting"; a runner that exited (and was reaped
        // above) without ever writing its marker means the launch failed.
        let failed: Vec<String> = self
            .motion
            .launching
            .iter()
            .filter(|(id, (_, pid))| !self.running.contains_key(*id) && !pid_alive(*pid))
            .map(|(id, _)| id.clone())
            .collect();
        self.motion.launching.retain(|id, _| !self.running.contains_key(id));
        for id in failed {
            self.motion.launching.remove(&id);
            if let Some(i) = self.index_of(&id) {
                let name = self.cfg.custom_games[i].name.clone();
                self.set_status(lfmt("{} failed to start — see its log", &[&name]), true);
            }
        }
        if ended.is_empty() {
            return;
        }
        let before = std::mem::replace(&mut self.stats, load_stats());
        self.sessions = sessions::load();
        for m in ended {
            self.forget_disk_use(&m.id);
            if let Some(l) = self.cfg.custom_games.iter().find(|g| g.id == m.id).and_then(|g| g.launcher.as_ref()) {
                self.launchers_due.push(l.key.clone());
            }
            let s = self.stats.get(&m.id).cloned().unwrap_or_default();
            let old = before.get(&m.id).map_or(0, |b| b.playtime_secs);
            if s.playtime_secs > old {
                self.motion.played.insert(m.id.clone(), (old, s.playtime_secs, None));
            }
            if self.stopping.remove(&m.id) {
                self.set_status(lfmt("{} stopped", &[&m.name]), false);
            } else if s.last_exit != Some(0) && s.last_session_secs < 30 {
                self.set_status(
                    lfmt("{} exited after {}s — see {}", &[&m.name, &s.last_session_secs, &m.log]),
                    true,
                );
            } else {
                self.set_status(
                    lfmt("{} closed · played {}", &[&m.name, &format_duration(s.last_session_secs)]),
                    false,
                );
            }
        }
    }

    pub(crate) fn play(&mut self, i: usize) {
        let g = &self.cfg.custom_games[i];
        if self.running.contains_key(&g.id) {
            return;
        }
        if let Some(r) = g.rom.as_ref().filter(|_| g.exe.is_empty()) {
            let (emu, _) = emulation::suggestion(&r.platform);
            let console = emulation::platform(&r.platform).map_or(r.platform.as_str(), |p| p.name);
            self.set_status(lfmt("No emulator for {} yet — install {} (see the Emulation page)", &[&console, &emu]), true);
            return;
        }
        // Surface problems (missing exe, no Proton…) here; the runner has no window.
        if let Err(e) = build_launch(g, &self.cfg.settings) {
            let id = g.id.clone();
            self.set_status(e, true);
            self.motion.shake.insert(id, self.now);
            return;
        }
        let (id, name) = (g.id.clone(), g.name.clone());
        match spawn_runner(&id) {
            Ok(child) => {
                // Launching an unsorted or backlog game makes it Playing.
                if matches!(self.cfg.custom_games[i].status, GameStatus::None | GameStatus::Backlog) {
                    self.cfg.custom_games[i].status = GameStatus::Playing;
                    save_config(&self.cfg);
                }
                self.motion.launching.insert(id, (self.now, child.id()));
                self.children.push(child);
                self.set_status(lfmt("Launching {}…", &[&name]), false);
            }
            Err(e) => {
                self.set_status(e, true);
                self.motion.shake.insert(id, self.now);
            }
        }
    }

    pub(crate) fn stop(&mut self, id: &str) {
        let Some(m) = self.running.get(id).cloned() else {
            return;
        };
        let g = self.index_of(id).map(|i| self.cfg.custom_games[i].clone());
        self.children.extend(stop_game(&m, g.as_ref(), &self.cfg.settings));
        self.stopping.insert(id.to_string());
        self.set_status(lfmt("Stopping {}…", &[&m.name]), false);
    }

    /// Games that still lack a cover or hero and haven't been auto-fetched.
    pub(crate) fn needs_art(&self) -> Vec<String> {
        self.cfg
            .custom_games
            .iter()
            .filter(|g| !g.art_checked && (g.cover.is_empty() || g.hero.is_empty()))
            .map(|g| g.id.clone())
            .collect()
    }

    /// Fetch missing covers/heroes for these games in the background, one at a time.
    pub(crate) fn auto_fetch(&mut self, ctx: &egui::Context, ids: Vec<String>) {
        let Some(key) = self.sgdb_key() else {
            return;
        };
        let jobs: Vec<(String, String, bool, bool)> = ids
            .iter()
            .filter(|id| !self.auto_pending.contains(*id))
            .filter_map(|id| {
                let g = self.cfg.custom_games.iter().find(|g| &g.id == id)?;
                let (want_cover, want_hero) = (g.cover.is_empty(), g.hero.is_empty());
                (want_cover || want_hero)
                    .then(|| (g.id.clone(), g.name.clone(), want_cover, want_hero))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        for j in &jobs {
            self.auto_pending.insert(j.0.clone());
        }
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            for (id, name, want_cover, want_hero) in jobs {
                let result = auto_fetch_art(&name, &id, &key, want_cover, want_hero);
                let _ = tx.send(ArtMsg::AutoFetched { id, result });
                ctx.request_repaint();
            }
        });
    }

    /// Load the game page's text buffers when a different game is shown.
    pub(crate) fn sync_edit(&mut self, i: usize) {
        let g = &self.cfg.custom_games[i];
        if self.edit.id == g.id {
            return;
        }
        self.edit = EditBuf {
            id: g.id.clone(),
            name: g.name.clone(),
            args: g.args.clone(),
            workdir: g.workdir.clone(),
            prefix: g.prefix.clone(),
            umu_id: g.umu_id.clone(),
            exe: g.exe.clone(),
            env: g.env.clone(),
        };
    }

    /// Save one edited text field of game `i`.
    pub(crate) fn commit_field(&mut self, i: usize, field: Field) {
        let old = self.cfg.custom_games[i].clone();
        let mut g = old.clone();
        let e = self.edit.clone();
        let (slot, value, current) = match field {
            Field::Name => (&mut g.name, e.name.trim(), &old.name),
            Field::Args => (&mut g.args, e.args.trim(), &old.args),
            Field::Workdir => (&mut g.workdir, e.workdir.trim(), &old.workdir),
            Field::Prefix => (&mut g.prefix, e.prefix.trim(), &old.prefix),
            Field::UmuId => (&mut g.umu_id, e.umu_id.trim(), &old.umu_id),
            Field::Exe => (&mut g.exe, e.exe.trim(), &old.exe),
            Field::Env => (&mut g.env, e.env.trim(), &old.env),
        };
        if value == current.as_str() || (matches!(field, Field::Name) && value.is_empty()) {
            // Unchanged (or an empty name): put the saved value back in the box.
            let saved = current.clone();
            self.edit_slot(field).clone_from(&saved);
            return;
        }
        *slot = value.to_string();
        let mut msg = lg("Saved").to_string();
        if matches!(field, Field::Name) && keep_prefix_on_rename(&old, &mut g, &self.cfg.settings) {
            msg = lfmt("Renamed — still using its prefix {}", &[&g.prefix]);
            self.edit.prefix = g.prefix.clone();
        }
        if matches!(field, Field::Exe) {
            self.forget_art(&g.id);
        }
        let saved = value.to_string();
        *self.edit_slot(field) = saved;
        self.cfg.custom_games[i] = g;
        save_config(&self.cfg);
        self.set_status(msg, false);
    }

    pub(crate) fn edit_slot(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.edit.name,
            Field::Args => &mut self.edit.args,
            Field::Workdir => &mut self.edit.workdir,
            Field::Prefix => &mut self.edit.prefix,
            Field::UmuId => &mut self.edit.umu_id,
            Field::Exe => &mut self.edit.exe,
            Field::Env => &mut self.edit.env,
        }
    }

    pub(crate) fn remove_game(&mut self, i: usize) {
        let g = self.cfg.custom_games.remove(i);
        // Leave its poster behind to fade out while the grid closes the gap.
        if let Some(rel) = self.motion.tile_rel.remove(&g.id) {
            let tex = self
                .icon_tex
                .get(&format!("poster-{}", g.id))
                .or_else(|| self.icon_tex.get(&format!("gen-{}", g.id)))
                .cloned();
            self.motion.ghosts.push(Ghost { rel, tex, since: self.now });
        }
        // Keep the artwork browser pointing at the same game.
        match self.art.game {
            Some(a) if a == i => {
                self.art.game = None;
                self.art.generation += 1;
                self.art.matches.clear();
                self.art.assets.clear();
                self.art.picked = None;
            }
            Some(a) if a > i => self.art.game = Some(a - 1),
            _ => {}
        }
        if self.selected.as_deref() == Some(g.id.as_str()) {
            self.selected = None;
            if matches!(self.page, Page::Game | Page::Artwork) {
                self.page = Page::Library;
            }
        }
        save_config(&self.cfg);
        self.set_status(lfmt("Removed {} from the library — its files are kept", &[&g.name]), false);
    }

    pub(crate) fn new_add_modal(&self, exe: PathBuf) -> AddModal {
        let is_exe = exe.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        // The Proton build chosen in Settings, else the one the last Windows game uses.
        let tool = |dir: &str| self.proton_tools.iter().position(|t| t.dir.to_string_lossy() == dir);
        let proton_idx = tool(&self.cfg.settings.default_proton)
            .or_else(|| {
                self.cfg
                    .custom_games
                    .iter()
                    .rev()
                    .find(|g| g.kind == GameKind::Windows)
                    .and_then(|g| tool(&g.proton_dir))
            })
            .unwrap_or(0);
        AddModal {
            name: guess_game_name(&exe),
            kind: if is_exe { GameKind::Windows } else { GameKind::Linux },
            exe: exe.to_string_lossy().into(),
            proton_idx,
        }
    }

    pub(crate) fn add_game(&mut self, ctx: &egui::Context, m: AddModal) {
        let (id, name) = self.insert_game(m);
        save_config(&self.cfg);
        self.selected = Some(id.clone());
        self.page = Page::Game;
        if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
            self.auto_fetch(ctx, vec![id]);
            self.set_status(lfmt("Added {} — fetching artwork…", &[&name]), false);
        } else {
            self.set_status(lfmt("Added {}", &[&name]), false);
        }
    }

    /// Add several games at once (from a folder scan), staying on the library.
    pub(crate) fn add_games(&mut self, ctx: &egui::Context, list: Vec<AddModal>) {
        let n = list.len();
        let ids: Vec<String> = list.into_iter().map(|m| self.insert_game(m).0).collect();
        save_config(&self.cfg);
        self.page = Page::Library;
        if self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch {
            self.auto_fetch(ctx, ids);
        }
        self.set_status(lfmt("Added {} games", &[&n]), false);
    }

    /// Put a new game in the library (unsaved). Returns its id and name.
    pub(crate) fn insert_game(&mut self, m: AddModal) -> (String, String) {
        let proton_dir = match m.kind {
            GameKind::Windows => self
                .proton_tools
                .get(m.proton_idx)
                .map(|t| t.dir.to_string_lossy().to_string())
                .unwrap_or_default(),
            GameKind::Linux => String::new(),
        };
        // A known umu-database entry turns protonfixes on from the first launch.
        let umu_id = match m.kind {
            GameKind::Windows => self.umu_match(m.name.trim()).map(|e| e.umu_id).unwrap_or_default(),
            GameKind::Linux => String::new(),
        };
        let g = CustomGame {
            id: new_game_id(),
            name: m.name.trim().to_string(),
            kind: m.kind,
            exe: m.exe,
            proton_dir,
            flags: self.cfg.settings.new_game_flags(),
            umu_id,
            added: unix_now(),
            ..Default::default()
        };
        let (id, name) = (g.id.clone(), g.name.clone());
        self.cfg.custom_games.push(g);
        self.motion.fresh.insert(id.clone(), None);
        (id, name)
    }
}
