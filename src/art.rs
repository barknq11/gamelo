//! Artwork and icon textures, and the Artwork page's searches and downloads.

use crate::*;

impl App {
    /// Drop a cached icon so it is rebuilt from its (possibly changed) source.
    pub(crate) fn forget_icon(&mut self, key: &str) {
        self.icon_tex.remove(key);
        self.icon_fail.remove(key);
        let _ = fs::remove_file(icons_dir().join(format!("{key}.png")));
    }

    /// Drop cached textures for a game's icon and cover.
    pub(crate) fn forget_art(&mut self, id: &str) {
        for kind in ["icon", "poster", "gen", "hero"] {
            self.forget_icon(&format!("{kind}-{id}"));
        }
    }

    /// Point a game's icon, cover or hero at `path` (empty = back to the default).
    pub(crate) fn set_art(&mut self, id: &str, kind: ArtKind, path: String) -> bool {
        let Some(g) = self.cfg.custom_games.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        if kind == ArtKind::Cover && !path.is_empty() {
            self.motion.art_arrived.insert(id.to_string(), self.now);
        }
        match kind {
            ArtKind::Icon => g.icon = path,
            ArtKind::Cover => g.cover = path,
            ArtKind::Hero => g.hero = path,
        }
        save_config(&self.cfg);
        self.forget_art(id);
        true
    }

    pub(crate) fn sgdb_key(&self) -> Option<String> {
        let k = self.cfg.settings.steamgriddb_key.trim();
        (!k.is_empty()).then(|| k.to_string())
    }

    pub(crate) fn art_game_id(&self) -> Option<String> {
        self.art
            .game
            .and_then(|i| self.cfg.custom_games.get(i))
            .map(|g| g.id.clone())
    }

    /// Show the Artwork page for game `i`.
    pub(crate) fn open_artwork(&mut self, ctx: &egui::Context, i: usize) {
        self.page = Page::Artwork;
        if self.art.game != Some(i) || self.art.matches.is_empty() {
            self.art_select_game(ctx, i);
        }
    }

    /// Switch the browser to game `i` and search SteamGridDB for its name.
    pub(crate) fn art_select_game(&mut self, ctx: &egui::Context, i: usize) {
        let Some(name) = self.cfg.custom_games.get(i).map(|g| g.name.clone()) else {
            return;
        };
        self.art.game = Some(i);
        self.art.query = name;
        self.art.matches.clear();
        self.art.sgdb_id = None;
        self.art.assets.clear();
        self.art.picked = None;
        self.art.more = false;
        self.art.error = None;
        self.art.loading = false;
        self.art.generation += 1;
        if self.sgdb_key().is_some() {
            self.art_search(ctx);
        }
    }

    pub(crate) fn art_search(&mut self, ctx: &egui::Context) {
        let Some(key) = self.sgdb_key() else {
            self.art.error = Some(lg("Add your SteamGridDB API key first").into());
            return;
        };
        let query = self.art.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.art.generation += 1;
        let generation = self.art.generation;
        self.art.matches.clear();
        self.art.sgdb_id = None;
        self.art.assets.clear();
        self.art.picked = None;
        self.art.more = false;
        self.art.error = None;
        self.art.loading = true;
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = sgdb_search(&query, &key);
            let _ = tx.send(ArtMsg::Games { generation, result });
            ctx.request_repaint();
        });
    }

    /// Load page `page` of the selected match's artwork (0 = start over).
    pub(crate) fn art_list(&mut self, ctx: &egui::Context, page: u32) {
        let (Some(key), Some(id)) = (self.sgdb_key(), self.art.sgdb_id) else {
            return;
        };
        if page == 0 {
            self.art.generation += 1;
            self.art.assets.clear();
            self.art.picked = None;
            self.art.more = false;
        }
        let generation = self.art.generation;
        let kind = self.art.kind;
        self.art.loading = true;
        self.art.error = None;
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = sgdb_assets(kind, id, page, &key);
            let _ = tx.send(ArtMsg::Assets {
                generation,
                page,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Download the picked image and set it on the current game.
    pub(crate) fn art_apply(&mut self, ctx: &egui::Context) {
        let (Some(id), Some(asset)) = (
            self.art_game_id(),
            self.art.picked.and_then(|p| self.art.assets.get(p)),
        ) else {
            return;
        };
        let (url, kind) = (asset.url.clone(), self.art.kind);
        self.art.applying = true;
        self.set_status(lfmt("Downloading {}…", &[&lg(kind.noun())]), false);
        let (tx, ctx) = (self.art_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = download_art(&url, &id, kind);
            let _ = tx.send(ArtMsg::Applied { id, kind, result });
            ctx.request_repaint();
        });
    }

    /// Handle replies from SteamGridDB worker threads.
    pub(crate) fn art_poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.art_rx.try_recv() {
            match msg {
                ArtMsg::Games { generation, result } => {
                    if generation != self.art.generation {
                        continue;
                    }
                    self.art.loading = false;
                    match result {
                        Ok(games) => {
                            if games.is_empty() {
                                self.art.error = Some(lfmt(
                                    "No SteamGridDB match for “{}” — try another spelling",
                                    &[&self.art.query.trim()],
                                ));
                            }
                            self.art.sgdb_id = games.first().map(|g| g.id);
                            self.art.matches = games;
                            self.art_list(ctx, 0);
                        }
                        Err(e) => self.art.error = Some(e),
                    }
                }
                ArtMsg::Assets {
                    generation,
                    page,
                    result,
                } => {
                    if generation != self.art.generation {
                        continue;
                    }
                    self.art.loading = false;
                    match result {
                        Ok((assets, more)) => {
                            if assets.is_empty() && page == 0 {
                                self.art.error = Some(lfmt(
                                    "SteamGridDB has no {} for this game yet",
                                    &[&lg(self.art.kind.noun_plural())],
                                ));
                            }
                            self.art.page = page;
                            self.art.more = more;
                            let urls: Vec<String> = assets
                                .iter()
                                .map(|a| a.thumb.clone())
                                .filter(|u| {
                                    !self.art.thumbs.contains_key(u)
                                        && !self.art.thumb_failed.contains(u)
                                        && !self.art.thumb_pending.contains(u)
                                })
                                .collect();
                            self.art.thumb_pending.extend(urls.iter().cloned());
                            self.art.assets.extend(assets);
                            spawn_thumb_workers(urls, self.art_tx.clone(), ctx.clone());
                        }
                        Err(e) => self.art.error = Some(e),
                    }
                }
                ArtMsg::Thumb { url, img } => {
                    self.art.thumb_pending.remove(&url);
                    match img {
                        Some(img) => {
                            let tex = load_rgba(ctx, format!("sgdb:{url}"), &img);
                            self.motion.thumb_loaded.insert(url.clone(), self.now);
                            self.art.thumbs.insert(url, tex);
                        }
                        None => {
                            self.art.thumb_failed.insert(url);
                        }
                    }
                }
                ArtMsg::Applied { id, kind, result } => {
                    self.art.applying = false;
                    match result {
                        Ok(path) => {
                            if self.set_art(&id, kind, path.to_string_lossy().to_string()) {
                                self.set_status(lfmt("New {} applied", &[&lg(kind.noun())]), false);
                            } else {
                                self.set_status(lg("Downloaded, but that game no longer exists").into(), true);
                            }
                        }
                        Err(e) => self.set_status(lfmt("Artwork download failed: {}", &[&e]), true),
                    }
                }
                ArtMsg::KeyChecked(result) => {
                    self.key_checking = false;
                    self.key_check = Some(result);
                }
                ArtMsg::AutoFetched { id, result } => {
                    self.auto_pending.remove(&id);
                    let Some(i) = self.index_of(&id) else {
                        continue;
                    };
                    let name = self.cfg.custom_games[i].name.clone();
                    match result {
                        Ok(art) => {
                            let (got_cover, got_hero) = (art.cover.is_some(), art.hero.is_some());
                            let g = &mut self.cfg.custom_games[i];
                            g.art_checked = true;
                            if let Some(p) = art.cover {
                                g.cover = p.to_string_lossy().into();
                                self.motion.art_arrived.insert(id.clone(), self.now);
                            }
                            if let Some(p) = art.hero {
                                g.hero = p.to_string_lossy().into();
                            }
                            save_config(&self.cfg);
                            self.forget_art(&id);
                            let msg = match (got_cover, got_hero) {
                                (true, true) => lfmt("Artwork added for {}", &[&name]),
                                (true, false) => lfmt("Cover added for {}", &[&name]),
                                (false, true) => lfmt("Banner added for {}", &[&name]),
                                (false, false) => lfmt("No artwork found for {} on SteamGridDB", &[&name]),
                            };
                            self.set_status(msg, false);
                        }
                        Err(e) => self.set_status(lfmt("Artwork for {}: {}", &[&name, &e]), true),
                    }
                }
            }
        }
    }

    pub(crate) fn icon_finish(&mut self, ctx: &egui::Context, key: &str, img: image::RgbaImage) -> egui::TextureHandle {
        let dir = icons_dir();
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{key}.png"));
        let _ = img.save(&path);
        let tex = load_rgba(ctx, key.to_string(), &img);
        self.icon_tex.insert(key.to_string(), tex.clone());
        tex
    }

    pub(crate) fn get_icon(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        source: &IconSource,
    ) -> Option<egui::TextureHandle> {
        self.get_tex(ctx, key, || match source {
            IconSource::Image(p) => decode_image(p).map(|i| square_icon(&i, 64)),
            IconSource::Exe(p) => extract_exe_icon(p).map(|i| square_icon(&i, 64)),
            IconSource::None => None,
        })
    }

    /// The icon for game `i`: its custom image, else the exe's embedded icon.
    pub(crate) fn game_icon(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        let key = format!("icon-{}", g.id);
        let source = if !g.icon.is_empty() && Path::new(&g.icon).is_file() {
            IconSource::Image(PathBuf::from(&g.icon))
        } else if g.kind == GameKind::Windows {
            IconSource::Exe(PathBuf::from(&g.exe))
        } else {
            IconSource::None
        };
        self.get_icon(ctx, &key, &source)
    }

    /// The cover poster for game `i` (300×450), if one is set.
    pub(crate) fn game_poster(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        if g.cover.is_empty() || !Path::new(&g.cover).is_file() {
            return None;
        }
        let key = format!("poster-{}", g.id);
        let path = PathBuf::from(&g.cover);
        self.get_tex(ctx, &key, || decode_image(&path).map(|i| cover_fit(&i, 300, 450)))
    }

    /// The game page banner: the hero image, else a blurred cover, else the
    /// generated poster's colours.
    pub(crate) fn game_hero(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureHandle> {
        let g = self.cfg.custom_games.get(i)?;
        let key = format!("hero-{}", g.id);
        let pick = |p: &str| (!p.is_empty() && Path::new(p).is_file()).then(|| PathBuf::from(p));
        let (hero, cover) = (pick(&g.hero), pick(&g.cover));
        let (seed, exe, windows) = (g.name.clone(), g.exe.clone(), g.kind == GameKind::Windows);
        self.get_tex(ctx, &key, || {
            if let Some(img) = hero.as_deref().and_then(decode_image) {
                return Some(hero_from(&img, false));
            }
            if let Some(img) = cover.as_deref().and_then(decode_image) {
                return Some(hero_from(&img, true));
            }
            let icon = windows.then(|| extract_exe_icon(Path::new(&exe))).flatten();
            Some(hero_from(&generated_poster(icon.as_ref(), &seed), true))
        })
    }

    /// A cached texture: from memory, else the on-disk PNG cache, else `make`.
    pub(crate) fn get_tex(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        make: impl FnOnce() -> Option<image::RgbaImage>,
    ) -> Option<egui::TextureHandle> {
        if let Some(t) = self.icon_tex.get(key) {
            return Some(t.clone());
        }
        if self.icon_fail.contains(key) {
            return None;
        }
        let dir = icons_dir();
        let cache = dir.join(format!("{key}.png"));
        if cache.is_file()
            && let Some(img) = decode_image(&cache)
        {
            let tex = load_rgba(ctx, key.to_string(), &img);
            self.icon_tex.insert(key.to_string(), tex.clone());
            return Some(tex);
        }
        match make() {
            Some(img) => Some(self.icon_finish(ctx, key, img)),
            None => {
                self.icon_fail.insert(key.to_string());
                None
            }
        }
    }
}
