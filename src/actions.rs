//! Applying what the user asked for: game actions, artwork commands, collections and page-wide actions.

use crate::*;

impl App {
    pub(crate) fn game_act(&mut self, ctx: &egui::Context, id: &str, act: GameAct) {
        let Some(i) = self.index_of(id) else {
            return;
        };
        let g = self.cfg.custom_games[i].clone();
        let settings = self.cfg.settings.clone();
        let report = |app: &mut App, r: Result<String, String>| match r {
            Ok(msg) => app.set_status(msg, false),
            Err(e) => app.set_status(e, true),
        };
        match act {
            GameAct::Play => {
                self.motion.ripple.insert(id.to_string(), self.now);
                self.play(i);
            }
            GameAct::Stop => self.stop(id),
            GameAct::Open => {
                self.selected = Some(g.id);
                self.page = Page::Game;
            }
            GameAct::Artwork => {
                self.selected = Some(g.id);
                self.open_artwork(ctx, i);
            }
            GameAct::Saves => {
                self.selected = Some(g.id);
                self.saves.view = None;
                self.page = Page::Saves;
            }
            GameAct::Shortcut => {
                let r = write_desktop_entry(&g, &settings)
                    .map(|p| lfmt("Added to the app menu: {}", &[&p.display()]));
                report(self, r);
            }
            GameAct::CopyCommand => match launch_command_string(&g, &settings) {
                Ok(c) => self.copy_text(&c),
                Err(e) => self.set_status(e, true),
            },
            GameAct::InitPrefix | GameAct::Winecfg => {
                let (r, msg) = match act {
                    GameAct::InitPrefix => (init_prefix(&g, &settings), lfmt("Initializing prefix for {}…", &[&g.name])),
                    _ => (run_winecfg(&g, &settings), lg("Opening winecfg…").to_string()),
                };
                let r = r.map(|child| {
                    self.children.push(child);
                    msg
                });
                report(self, r);
            }
            GameAct::OpenPrefix => {
                self.children.extend(xdg_open(game_prefix(&g, &settings)));
                self.set_status(lg("Opened prefix folder").into(), false);
            }
            GameAct::OpenGameFolder => {
                let dir = game_workdir(&g).filter(|d| d.is_dir());
                match dir {
                    Some(d) => {
                        self.children.extend(xdg_open(&d));
                        self.set_status(lfmt("Opened {}", &[&d.display()]), false);
                    }
                    None => self.set_status(lfmt("{}’s folder can’t be found", &[&g.name]), true),
                }
            }
            GameAct::OpenLog => {
                let log = data_dir().join("logs").join(format!("{}.log", slugify(&g.name)));
                if log.is_file() {
                    self.children.extend(xdg_open(&log));
                } else {
                    self.set_status(lfmt("No log yet for {}", &[&g.name]), true);
                }
            }
            GameAct::AskRemove => self.confirm_remove = Some(g.id),
            GameAct::Remove => self.remove_game(i),
            GameAct::SetFlags(f) => {
                self.cfg.custom_games[i].flags = f;
                save_config(&self.cfg);
            }
            GameAct::SetKind(k) => {
                self.cfg.custom_games[i].kind = k;
                save_config(&self.cfg);
                self.forget_art(id);
            }
            GameAct::SetProton(n) => {
                if let Some(t) = self.proton_tools.get(n) {
                    self.cfg.custom_games[i].proton_dir = t.dir.to_string_lossy().into();
                    let label = t.label.clone();
                    save_config(&self.cfg);
                    self.set_status(lfmt("{} now uses {}", &[&g.name, &label]), false);
                }
            }
            GameAct::BrowseExe => {
                if let Some(p) = pick_executable() {
                    self.edit.exe = p.to_string_lossy().into();
                    self.commit_field(i, Field::Exe);
                }
            }
            GameAct::Commit(f) => self.commit_field(i, f),
            GameAct::SetGamescope(o) => {
                self.cfg.custom_games[i].gamescope_opts = o;
                save_config(&self.cfg);
            }
            GameAct::SetTuning(t) => {
                self.cfg.custom_games[i].tuning = t;
                save_config(&self.cfg);
            }
            GameAct::SetStatus(st) => {
                self.cfg.custom_games[i].status = st;
                save_config(&self.cfg);
            }
            GameAct::ToggleCollection(c) => {
                let list = &mut self.cfg.custom_games[i].collections;
                if let Some(at) = list.iter().position(|x| *x == c) {
                    list.remove(at);
                } else {
                    list.push(c);
                }
                save_config(&self.cfg);
            }
            GameAct::Favorite => {
                let fav = !g.favorite;
                self.cfg.custom_games[i].favorite = fav;
                save_config(&self.cfg);
                let msg = if fav { "Added {} to favorites" } else { "Removed {} from favorites" };
                self.set_status(lfmt(msg, &[&g.name]), false);
            }
            GameAct::SetNoFixes(off) => {
                self.cfg.custom_games[i].flags.no_protonfixes = off;
                save_config(&self.cfg);
            }
            GameAct::SetUmuId(umu) => {
                self.cfg.custom_games[i].umu_id = umu.clone();
                if self.edit.id == id {
                    self.edit.umu_id = umu.clone();
                }
                save_config(&self.cfg);
                self.set_status(lfmt("{} now uses {}", &[&g.name, &umu]), false);
            }
        }
    }

    pub(crate) fn apply_art_cmds(&mut self, ctx: &egui::Context, cmds: Vec<ArtCmd>) {
        for cmd in cmds {
            match cmd {
                ArtCmd::Search => self.art_search(ctx),
                ArtCmd::SelectMatch(id) => {
                    if self.art.sgdb_id != Some(id) {
                        self.art.sgdb_id = Some(id);
                        self.art_list(ctx, 0);
                    }
                }
                ArtCmd::Kind(k) => {
                    if self.art.kind != k {
                        self.art.kind = k;
                        self.art_list(ctx, 0);
                    }
                }
                ArtCmd::More => {
                    let next = self.art.page + 1;
                    self.art_list(ctx, next);
                }
                ArtCmd::Pick(n) => self.art.picked = Some(n),
                ArtCmd::Apply => self.art_apply(ctx),
                ArtCmd::Local(kind) => {
                    if let Some(id) = self.art_game_id()
                        && let Some(p) = pick_image()
                    {
                        self.set_art(&id, kind, p.to_string_lossy().to_string());
                        self.set_status(lfmt("Custom {} set", &[&lg(kind.noun())]), false);
                    }
                }
                ArtCmd::Reset(kind) => {
                    if let Some(id) = self.art_game_id() {
                        self.set_art(&id, kind, String::new());
                        self.set_status(lfmt("{} reset", &[&lg(kind.noun())]), false);
                    }
                }
                ArtCmd::SaveKey => {
                    let key = std::mem::take(&mut self.art.key_input);
                    if key.trim().is_empty() {
                        continue;
                    }
                    self.cfg.settings.steamgriddb_key = key.trim().to_string();
                    save_config(&self.cfg);
                    self.set_status(lg("SteamGridDB key saved").into(), false);
                    if let Some(i) = self.art.game {
                        self.art_select_game(ctx, i);
                    }
                }
            }
        }
    }

    /// Name a new collection (adding the game it was opened from), or rename one.
    pub(crate) fn collection_modal(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.collection_edit.take() else { return };
        let mut done = false;
        let mut cancel = false;
        let title = if edit.renaming.is_some() { "Rename collection" } else { "New collection" };
        let resp = egui::Modal::new(egui::Id::unique("collection-edit")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new(tr(title)).size(18.0).strong().color(pal().text));
            ui.add_space(10.0);
            let (r, _) = text_field(ui, &mut edit.name, "e.g. Co-op, Soulslikes…", f32::INFINITY);
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = !edit.name.trim().is_empty();
                ui.add_enabled_ui(ok, |ui| {
                    if primary_button(ui, if edit.renaming.is_some() { "Rename" } else { "Create" }).clicked() {
                        done = true;
                    }
                });
                done |= ok && enter;
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        }));
        if done {
            let name = edit.name.trim().to_string();
            let settings = &mut self.cfg.settings;
            match &edit.renaming {
                Some(old) => {
                    for c in settings.collections.iter_mut().filter(|c| *c == old) {
                        c.clone_from(&name);
                    }
                    for g in &mut self.cfg.custom_games {
                        for c in g.collections.iter_mut().filter(|c| *c == old) {
                            c.clone_from(&name);
                        }
                    }
                    if self.lib_filter == LibFilter::Collection(old.clone()) {
                        self.lib_filter = LibFilter::Collection(name.clone());
                    }
                }
                None => {
                    if !settings.collections.contains(&name) {
                        settings.collections.push(name.clone());
                    }
                    if let Some(i) = edit.game.as_deref().and_then(|id| self.index_of(id))
                        && !self.cfg.custom_games[i].collections.contains(&name)
                    {
                        self.cfg.custom_games[i].collections.push(name.clone());
                    }
                }
            }
            save_config(&self.cfg);
            return;
        }
        if !(cancel || resp.should_close()) {
            self.collection_edit = Some(edit);
        }
    }

    pub(crate) fn apply_acts(&mut self, ctx: &egui::Context, acts: Acts) {
        if let Some(t) = acts.copy {
            self.copy_text(&t);
        }
        if let Some(target) = acts.open {
            // Folders we own may not exist yet (no logs before the first launch…).
            if target.starts_with('/') && !Path::new(&target).exists() {
                let _ = fs::create_dir_all(&target);
            }
            self.children.extend(xdg_open(&target));
        }
        if acts.rescan {
            self.refresh();
        }
        if acts.save_settings {
            save_config(&self.cfg);
            // Tool paths may have changed.
            self.deps = self.dep_rows();
            self.set_status(lg("Settings saved").into(), false);
        }
        if acts.test_key
            && let Some(key) = self.sgdb_key()
        {
            self.key_checking = true;
            self.key_check = None;
            let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let result = sgdb_search("Portal", &key).map(|_| ());
                let _ = tx.send(ArtMsg::KeyChecked(result));
                ctx.request_repaint();
            });
        }
        if acts.clear_cache {
            let freed = self.cache_bytes.unwrap_or(0);
            // Only derived images: downloaded artwork (art_dir) is referenced by the config.
            let _ = fs::remove_dir_all(thumb_cache_dir());
            let _ = fs::remove_dir_all(icons_dir());
            self.icon_tex.clear();
            self.icon_fail.clear();
            self.art.thumbs.clear();
            self.art.thumb_failed.clear();
            self.cache_bytes = None;
            self.set_status(lfmt("Cleared {} of cached images", &[&format_bytes(freed)]), false);
        }
        if let Some(action) = acts.dep {
            let steam_root = self.steam_root.clone();
            match action {
                DepAction::Proton => self.start_download(lg("Downloading GE-Proton"), move || {
                    install_ge_proton(steam_root)
                }),
                DepAction::Umu => self.start_download(lg("Downloading umu-run"), install_umu),
            }
        }
        if acts.back {
            self.page = match self.page {
                Page::Artwork | Page::Saves if self.selected_index().is_some() => Page::Game,
                _ => Page::Library,
            };
        }
        if let Some(p) = acts.goto {
            self.page = p;
        }
        if acts.add
            && let Some(p) = pick_executable()
        {
            self.add_modal = Some(self.new_add_modal(p));
        }
        if acts.scan
            && let Some(dir) = pick_folder(lg("Folder with your games"))
        {
            self.start_scan(ctx, dir);
        }
        if acts.steam_import {
            self.open_steam_import();
        }
        if acts.lutris_import {
            self.open_lutris_import();
        }
        if let Some(edit) = acts.collection {
            self.collection_edit = Some(edit);
        }
        if let Some(name) = acts.delete_collection {
            self.cfg.settings.collections.retain(|c| *c != name);
            for g in &mut self.cfg.custom_games {
                g.collections.retain(|c| *c != name);
            }
            if self.lib_filter == LibFilter::Collection(name.clone()) {
                self.lib_filter = LibFilter::All;
            }
            save_config(&self.cfg);
            self.set_status(lfmt("Deleted the collection “{}”", &[&name]), false);
        }
        if acts.fill_art {
            let ids = self.needs_art();
            self.auto_fetch(ctx, ids);
        }
        self.apply_art_cmds(ctx, acts.art);
        // Edits first, so a Play in the same frame launches with them saved.
        let (plays, rest): (Vec<_>, Vec<_>) = acts
            .game
            .into_iter()
            .partition(|(_, a)| matches!(a, GameAct::Play | GameAct::Stop));
        for (id, act) in rest.into_iter().chain(plays) {
            self.game_act(ctx, &id, act);
        }
        // Left the game page (sidebar, back, remove…): save any field still being
        // edited, since its widget won't be drawn again to report the focus loss.
        if self.page != Page::Game && !self.edit.id.is_empty() {
            if let Some(i) = self.index_of(&self.edit.id) {
                for f in [Field::Name, Field::Args, Field::Workdir, Field::Prefix, Field::UmuId, Field::Exe, Field::Env] {
                    self.commit_field(i, f);
                }
            }
            self.edit = EditBuf::default();
        }
    }
}
