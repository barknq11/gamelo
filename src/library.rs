//! The Library page: filters, sorting, the poster grid and keyboard navigation.

use crate::*;

impl App {
    /// Library order: search filter, then the chosen sort.
    pub(crate) fn library_order(&self) -> Vec<usize> {
        self.library_order_for(&self.search)
    }

    /// Does game `i` pass filter `f`?
    pub(crate) fn filter_matches(&self, f: &LibFilter, i: usize) -> bool {
        let g = &self.cfg.custom_games[i];
        match f {
            LibFilter::All => true,
            LibFilter::Favorites => g.favorite,
            LibFilter::Unplayed => self.stats.get(&g.id).is_none_or(|s| s.last_played == 0),
            LibFilter::Status(s) => g.status == *s,
            LibFilter::Collection(c) => g.collections.contains(c),
        }
    }

    /// The filters worth offering: All and Not played yet always; Favorites,
    /// each status and each collection once at least one game is in it.
    /// (filter, logical label)
    pub(crate) fn filter_options(&self) -> Vec<(LibFilter, String)> {
        let mut out = vec![(LibFilter::All, lg("All").to_string())];
        let n = self.cfg.custom_games.len();
        let any = |f: &LibFilter| (0..n).any(|i| self.filter_matches(f, i));
        if any(&LibFilter::Favorites) {
            out.push((LibFilter::Favorites, lg("Favorites").to_string()));
        }
        out.push((LibFilter::Unplayed, lg("Not played yet").to_string()));
        for s in GameStatus::ALL {
            if any(&LibFilter::Status(s)) {
                out.push((LibFilter::Status(s), lg(s.label()).to_string()));
            }
        }
        for c in &self.cfg.settings.collections {
            let f = LibFilter::Collection(c.clone());
            if any(&f) {
                out.push((f, c.clone()));
            }
        }
        out
    }

    /// `library_order` with search text `query`.
    pub(crate) fn library_order_for(&self, query: &str) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let games = &self.cfg.custom_games;
        let mut v: Vec<usize> = (0..games.len())
            .filter(|&i| games[i].rom.is_none())
            .filter(|&i| q.is_empty() || games[i].name.to_lowercase().contains(&q))
            .filter(|&i| self.filter_matches(&self.lib_filter, i))
            .collect();
        let stat = |i: usize| self.stats.get(&games[i].id).cloned().unwrap_or_default();
        let name = |i: usize| games[i].name.to_lowercase();
        match self.sort {
            SortMode::Name => v.sort_by_key(|&i| name(i)),
            SortMode::Recent => v.sort_by(|&a, &b| {
                stat(b)
                    .last_played
                    .cmp(&stat(a).last_played)
                    .then_with(|| name(a).cmp(&name(b)))
            }),
            SortMode::Playtime => v.sort_by(|&a, &b| {
                stat(b)
                    .playtime_secs
                    .cmp(&stat(a).playtime_secs)
                    .then_with(|| name(a).cmp(&name(b)))
            }),
        }
        // Favourites lead whatever the sort (a stable sort keeps their order).
        v.sort_by_key(|&i| !games[i].favorite);
        v
    }

    pub(crate) fn page_library(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let sort_before = self.sort;
        // Right-click on empty space: library-wide actions. Registered first, so
        // posters and controls drawn on top keep their own clicks and menus.
        let bg = ui.interact(ui.clip_rect(), ui.scope_id().with("library-bg"), egui::Sense::click());
        let can_fill = self.sgdb_key().is_some() && self.auto_pending.is_empty() && !self.needs_art().is_empty();
        bg.context_menu(|ui| menu_ui(ui, |ui| {
            if ui.button(tr("Add game…")).clicked() {
                acts.add = true;
                ui.close();
            }
            if ui.button(tr("Scan a folder for games…")).clicked() {
                acts.scan = true;
                ui.close();
            }
            if ui.button(tr("Non-Steam games from Steam…")).clicked() {
                acts.steam_import = true;
                ui.close();
            }
            ui.menu_button(tr("Sort by"), |ui| menu_ui(ui, |ui| {
                for m in [SortMode::Recent, SortMode::Name, SortMode::Playtime] {
                    if ui.selectable_label(self.sort == m, tr(m.label())).clicked() {
                        self.sort = m;
                        ui.close();
                    }
                }
            }));
            ui.menu_button(tr("Tile size"), |ui| menu_ui(ui, |ui| {
                for (key, label, _) in TILE_SIZES {
                    if ui.selectable_label(self.cfg.settings.tile_size_key() == key, tr(label)).clicked() {
                        self.cfg.settings.tile_size = key.into();
                        save_config(&self.cfg);
                        ui.close();
                    }
                }
            }));
            if can_fill && ui.button(tr("Fill missing artwork")).clicked() {
                acts.fill_art = true;
                ui.close();
            }
            ui.separator();
            if ui.button(tr("Refresh")).clicked() {
                acts.rescan = true;
                ui.close();
            }
        }));
        ui.horizontal(|ui| {
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(tr("Library")).size(24.0).strong().color(pal().text));
                let n = self.cfg.custom_games.len();
                let playing = self.running.len();
                let games = if n == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&n]) };
                let sub = match playing {
                    0 => disp(&games),
                    p => disp(&format!("{games} · {}", lfmt("{} playing", &[&p]))),
                };
                ui.label(egui::RichText::new(sub).color(pal().subtext).small());
            });
            ui.with_layout(far_end(), |ui| {
                if primary_button(ui, "+  Add game").clicked() {
                    acts.add = true;
                }
                ui.menu_button(tr("Import"), |ui| menu_ui(ui, |ui| {
                    if ui.button(tr("Scan a folder for games…")).on_hover_text(tr("Find every game in a folder and add them together")).clicked() {
                        acts.scan = true;
                        ui.close();
                    }
                    if ui.button(tr("Non-Steam games from Steam…")).on_hover_text(tr("Bring over games you added to Steam, with their prefixes and artwork")).clicked() {
                        acts.steam_import = true;
                        ui.close();
                    }
                    if ui.button(tr("Games from Lutris…")).on_hover_text(tr("Bring over games you installed with Lutris, with their prefixes")).clicked() {
                        acts.lutris_import = true;
                        ui.close();
                    }
                }));
                if !self.auto_pending.is_empty() {
                    ui.label(egui::RichText::new(tr("Fetching artwork…")).color(pal().subtext).small());
                    ui.spinner();
                } else if self.sgdb_key().is_some()
                    && !self.needs_art().is_empty()
                    && ui.button(tr("Fill missing artwork")).clicked()
                {
                    acts.fill_art = true;
                }
            });
        });
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let (search, _) = text_field(ui, &mut self.search, "Search games…", 280.0);
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
                search.request_focus();
            }
            ui.with_layout(far_end(), |ui| {
                egui::ComboBox::from_id_salt("sort")
                    .selected_text(tr(self.sort.label()))
                    .show_ui(ui, |ui| menu_ui(ui, |ui| {
                        for m in [SortMode::Recent, SortMode::Name, SortMode::Playtime] {
                            ui.selectable_value(&mut self.sort, m, tr(m.label()));
                        }
                    }));
                ui.label(egui::RichText::new(tr("Sort")).color(pal().subtext).small());
                ui.add_space(10.0);
                if let Some(k) = tile_size_toggle(ui, &self.cfg.settings) {
                    self.set_layout(k);
                }
            });
        });
        // Filters: the fixed ones, then statuses and collections that have games.
        let options = self.filter_options();
        if !options.iter().any(|(f, _)| *f == self.lib_filter) {
            self.lib_filter = LibFilter::All;
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            for (f, label) in options {
                let r = choice_chip(ui, self.lib_filter == f, &label);
                if r.clicked() {
                    self.lib_filter = f.clone();
                }
                if let LibFilter::Collection(name) = &f {
                    r.context_menu(|ui| menu_ui(ui, |ui| {
                        if ui.button(tr("Rename collection…")).clicked() {
                            acts.collection = Some(CollectionEdit { game: None, renaming: Some(name.clone()), name: name.clone() });
                            ui.close();
                        }
                        if ui.button(egui::RichText::new(tr("Delete collection")).color(pal().danger)).clicked() {
                            acts.delete_collection = Some(name.clone());
                            ui.close();
                        }
                    }));
                }
            }
        });
        ui.add_space(18.0);

        if self.cfg.custom_games.is_empty() {
            empty_hint(ui, "Your library is empty — click “+ Add game” and pick a game’s executable.");
            return;
        }
        if self.sort != sort_before {
            self.cfg.settings.library_sort = self.sort.key().into();
            save_config(&self.cfg);
        }
        let browsing = self.search.trim().is_empty() && self.lib_filter == LibFilter::All;
        if browsing {
            self.jump_back_in(ui, ctx, acts);
        }
        let order = self.library_order();
        if order.is_empty() {
            let why = match self.lib_filter {
                LibFilter::Unplayed if self.search.trim().is_empty() => "You’ve played every game in your library.",
                _ => "No games match your search.",
            };
            empty_hint(ui, why);
            return;
        }
        let avail = ui.available_width() - 1.0;
        let cols = grid_cols(avail);
        let gap = grid_gap(avail);
        if self.cfg.settings.library_view == "list" {
            self.library_keys(ctx, &order, 1, acts);
            self.library_list(ui, ctx, &order, acts);
            return;
        }
        self.library_keys(ctx, &order, cols as isize, acts);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, 22.0);
            let origin = ui.max_rect().min;
            self.motion.grid_origin = origin;
            for i in order {
                self.library_tile(ui, ctx, i, origin, acts);
            }
            // Removed posters shrink and fade where they stood.
            let (now, secs) = (self.now, self.secs(0.3));
            self.motion.ghosts.retain(|g| now - g.since < f64::from(secs));
            self.motion.busy |= !self.motion.ghosts.is_empty();
            for g in &self.motion.ghosts {
                let t = ease_since(now, g.since, secs);
                let r = g.rel.translate(origin.to_vec2());
                let r = egui::Rect::from_center_size(r.center(), r.size() * (1.0 - 0.15 * t));
                if let Some(tex) = &g.tex {
                    ui.painter().add(cover_fill_shape(tex, r, 10, 1.0 - t));
                }
            }
            if browsing {
                let (slot, resp) =
                    ui.allocate_exact_size(egui::vec2(tile_w(), tile_h() + 46.0), egui::Sense::click());
                let rect = self.glide("+add", slot, origin);
                add_tile(ui, rect, resp, acts);
            }
        });
    }

    /// A pick from the layout buttons: "list", or a grid tile size.
    pub(crate) fn set_layout(&mut self, k: &str) {
        if k == "list" {
            self.cfg.settings.library_view = "list".into();
        } else {
            self.cfg.settings.library_view = "grid".into();
            self.cfg.settings.tile_size = k.into();
        }
        save_config(&self.cfg);
    }

    /// Arrow keys move a focus ring over the grid; Enter opens the game's page
    /// and Space plays it. Off while a text field has focus.
    pub(crate) fn library_keys(&mut self, ctx: &egui::Context, order: &[usize], cols: isize, acts: &mut Acts) {
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
            self.lib_keys = false;
        }
        if ctx.memory(|m| m.focused().is_some()) || overlay_open(ctx) {
            return;
        }
        let (step, enter, space) = ctx.input(|i| {
            let k = |key| i.key_pressed(key) as isize;
            let side = k(egui::Key::ArrowRight) - k(egui::Key::ArrowLeft);
            let side = if rtl() { -side } else { side };
            let step = side + cols * (k(egui::Key::ArrowDown) - k(egui::Key::ArrowUp));
            (step, i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Space))
        });
        let games = &self.cfg.custom_games;
        let cur = self.lib_cursor.as_ref().and_then(|id| order.iter().position(|&i| &games[i].id == id));
        if step != 0 {
            let next = match cur {
                Some(c) if self.lib_keys => (c as isize + step).clamp(0, order.len() as isize - 1) as usize,
                // The first press shows the ring where it was (or on the first game).
                Some(c) => c,
                None => 0,
            };
            self.lib_cursor = Some(games[order[next]].id.clone());
            self.lib_keys = true;
            self.lib_scroll = true;
        } else if self.lib_keys
            && let Some(c) = cur
        {
            let id = games[order[c]].id.clone();
            if enter {
                acts.game.push((id, GameAct::Open));
            } else if space {
                let act = if self.running.contains_key(&id) { GameAct::Stop } else { GameAct::Play };
                acts.game.push((id, act));
            }
        }
    }

    pub(crate) fn library_tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        i: usize,
        origin: egui::Pos2,
        acts: &mut Acts,
    ) {
        let (slot, resp) =
            ui.allocate_exact_size(egui::vec2(tile_w(), tile_h() + 46.0), egui::Sense::click());
        let g = &self.cfg.custom_games[i];
        let (id, name) = (g.id.clone(), g.name.clone());
        let fav = g.favorite;
        let missing = game_missing_files(g);
        let rect = self.glide(&id, slot, origin);
        let rect = rect.translate(egui::vec2(self.shake_dx(&id), 0.0));
        let stat = self.stats.get(&id).cloned().unwrap_or_default();
        // A game added this session pops into the grid the first time it's seen.
        let pop = match self.motion.fresh.get(&id).copied() {
            None => 1.0,
            Some(born) => {
                let born = born.unwrap_or(self.now);
                self.motion.fresh.insert(id.clone(), Some(born));
                let secs = self.secs(0.5);
                let raw = if secs > 0.0 { ((self.now - born) as f32 / secs).clamp(0.0, 1.0) } else { 1.0 };
                if raw >= 1.0 {
                    self.motion.fresh.remove(&id);
                } else {
                    self.motion.busy = true;
                }
                egui::emath::easing::back_out(raw)
            }
        };
        let running = self.running.contains_key(&id);
        let fetching = self.auto_pending.contains(&id);
        let cover = self.game_poster(ctx, i);
        let generated = cover.is_none();
        let tex = cover.or_else(|| self.game_generated_poster(ctx, i));

        let launching = !running && self.motion.launching.contains_key(&id);
        let focused = self.lib_keys && self.lib_cursor.as_deref() == Some(id.as_str());
        if focused && std::mem::take(&mut self.lib_scroll) {
            ui.scroll_to_rect(rect.expand(16.0), None);
        }
        let hovered = resp.hovered() || focused;
        // `h` eases the shading in; `spring` overshoots a little, for the lift
        // and the pop; `press` squashes the poster while the button is held.
        let h = ctx.animate_bool_with_time_and_easing(
            egui::Id::unique(("tile-hover", &id)),
            hovered,
            self.secs(0.16),
            egui::emath::easing::cubic_out,
        );
        let spring = ctx.animate_bool_with_time_and_easing(
            egui::Id::unique(("tile-spring", &id)),
            hovered,
            self.secs(0.34),
            egui::emath::easing::back_out,
        );
        let press = ctx.animate_bool_with_time(
            egui::Id::unique(("tile-press", &id)),
            resp.is_pointer_button_down_on(),
            self.secs(0.07),
        );
        let poster = egui::Rect::from_min_size(rect.min, egui::vec2(tile_w(), tile_h()));
        self.motion.tile_rel.insert(id.clone(), poster.translate(-origin.to_vec2()));
        let poster = egui::Rect::from_center_size(
            poster.center() + egui::vec2(0.0, 14.0 * (1.0 - pop) - 7.0 * spring + 3.0 * press),
            poster.size() * (0.86 + 0.14 * pop) * (1.0 + 0.04 * spring - 0.05 * press),
        );
        // Where the pointer is over the poster (-1..1 each way), eased, so the
        // art can drift toward it and a light can follow it.
        let pointer = resp.hover_pos().filter(|_| resp.hovered());
        let aim = pointer.map_or(egui::Vec2::ZERO, |p| {
            ((p - poster.center()) / (poster.size() / 2.0)).clamp(egui::vec2(-1.0, -1.0), egui::vec2(1.0, 1.0))
        });
        let aim = egui::vec2(
            ctx.animate_value_with_time(egui::Id::unique(("tile-aim-x", &id)), aim.x, self.secs(0.18)),
            ctx.animate_value_with_time(egui::Id::unique(("tile-aim-y", &id)), aim.y, self.secs(0.18)),
        );
        let art_pan = if self.cfg.settings.reduce_motion {
            egui::vec2(0.5, 0.5)
        } else {
            egui::vec2(0.5 + 0.5 * aim.x, 0.5 + 0.5 * aim.y)
        };
        let art_zoom = 1.0 + 0.07 * h;
        let radius = rad(10);
        // Coming back from its page, the poster is still flying home: leave its
        // slot empty (the title and playtime below still show).
        let mut painter = ui.painter().clone();
        if self.fly(ctx, &id, true, poster) {
            painter.set_invisible();
        }
        if h > 0.0 {
            // A few widening layers make a soft shadow that deepens as it lifts.
            for (grow, alpha) in [(2.0, 70.0), (6.0, 40.0), (11.0, 22.0)] {
                painter.rect_filled(
                    poster.translate(egui::vec2(0.0, 5.0 + 7.0 * spring)).expand(grow * spring.max(0.0)),
                    rad(10 + grow as u8),
                    theme::shadow(alpha * h),
                );
            }
        }
        // New cover art dissolves in over the generated poster it replaces.
        let fade = self.motion.art_arrived.get(&id).copied().map(|t| self.progress(t, 0.7));
        if fade == Some(1.0) {
            self.motion.art_arrived.remove(&id);
        }
        match &tex {
            Some(t) => {
                let alpha = pop.min(1.0);
                if let Some(f) = fade.filter(|&f| f < 1.0 && !generated) {
                    if let Some(old) = self.game_generated_poster(ctx, i) {
                        painter.add(cover_fill_shape_pan(&old, poster, 10, alpha, art_zoom, art_pan));
                    }
                    painter.add(cover_fill_shape_pan(t, poster, 10, f * alpha, art_zoom, art_pan));
                } else {
                    painter.add(cover_fill_shape_pan(t, poster, 10, alpha, art_zoom, art_pan));
                }
            }
            None => {
                painter.rect_filled(poster, radius, pal().card2);
            }
        }
        // Text and badges over the poster use a dark palette in every theme.
        let over = theme::OverImage::new();
        if generated {
            // Generated posters carry the name; real covers already show the title.
            let galley = painter.layout(
                disp(&name),
                egui::FontId::proportional(15.0),
                pal().text,
                poster.width() - 24.0,
            );
            let pos = egui::pos2(poster.center().x - galley.size().x / 2.0, poster.top() + poster.height() * 0.62);
            painter.galley(pos, galley, pal().text);
        }

        // While artwork is being fetched, a band of light sweeps the poster.
        if fetching {
            if self.cfg.settings.reduce_motion {
                poster_badge(&painter, poster.left_bottom() + egui::vec2(8.0, -28.0), "Fetching art", pal().accent, 1.0);
            } else {
                let t = self.sweep(1.6);
                sheen(&painter.with_clip_rect(poster), poster, t);
            }
        }

        // Badges, top-left. A running game's dot breathes.
        let pulse = if running { self.pulse(2.4) } else { 1.0 };
        let mut y = poster.top() + 8.0;
        if running {
            y += poster_badge(&painter, egui::pos2(poster.left() + 8.0, y), "Running", pal().ok, pulse);
        }
        if missing {
            poster_badge(&painter, egui::pos2(poster.left() + 8.0, y), "Missing files", pal().warn, 1.0);
        }
        if fav {
            let c = poster.right_top() + egui::vec2(-18.0, 18.0);
            painter.circle_filled(c, 12.0, egui::Color32::from_black_alpha(170));
            star(&painter, c, 7.5, true, GOLD);
        }
        let status = self.cfg.custom_games[i].status;
        if status != GameStatus::None {
            poster_badge(&painter, poster.left_bottom() + egui::vec2(8.0, -28.0), status.label(), status.color(), 1.0);
        }

        let center = poster.center();
        let play_r = 27.0;
        let over_play = hovered
            && !launching
            && !(running && self.stopping.contains(&id))
            && resp
                .hover_pos()
                .is_some_and(|p| p.distance(center) <= play_r + 4.0);
        let stopping = running && self.stopping.contains(&id);
        let working = if launching {
            Some(("Starting…", pal().accent))
        } else if stopping {
            Some(("Stopping…", pal().danger))
        } else {
            None
        };
        if let Some((label, color)) = working {
            // Between a click and the game window opening or closing: a quiet ring.
            painter.rect_filled(poster, radius, egui::Color32::from_black_alpha(130));
            let angle = self.sweep(1.1) * std::f32::consts::TAU;
            spinner_arc(&painter, center, 22.0, angle, color);
            painter.text(
                center + egui::vec2(0.0, 36.0),
                egui::Align2::CENTER_TOP,
                tr(label),
                egui::FontId::proportional(13.0),
                pal().text,
            );
        } else if h > 0.0 {
            // Hover: dim the poster a little, let a soft light follow the
            // pointer, and spring a round play/stop button in.
            painter.rect_filled(poster, radius, egui::Color32::from_black_alpha((70.0 * h) as u8));
            if pointer.is_some() {
                let light = poster.center() + aim * poster.size() / 2.0;
                glow_spot(&painter.with_clip_rect(poster.shrink(1.0)), light, poster.width() * 0.95, egui::Color32::from_white_alpha((80.0 * h) as u8));
            }
            let fill = match (running, over_play) {
                (true, true) => pal().danger,
                (true, false) => pal().danger.gamma_multiply(0.8),
                (false, true) => pal().accent,
                (false, false) => pal().accent.gamma_multiply(0.85),
            };
            // Over the button itself, it swells and gets a halo.
            let swell = ctx.animate_bool_with_time_and_easing(
                egui::Id::unique(("tile-over-play", &id)),
                over_play,
                self.secs(0.22),
                egui::emath::easing::back_out,
            );
            let r = play_r * (0.55 + 0.45 * spring) * (1.0 + 0.12 * swell - 0.08 * press);
            if swell > 0.0 {
                let halo = egui::Stroke::new(2.0, fill.gamma_multiply(0.45 * swell.clamp(0.0, 1.0)));
                if pal().square {
                    painter.rect_stroke(egui::Rect::from_center_size(center, egui::vec2(2.0, 2.0) * (r + 6.0 * swell)), 0, halo, egui::StrokeKind::Outside);
                } else {
                    painter.circle_stroke(center, r + 6.0 * swell, halo);
                }
            }
            if pal().square {
                painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r)), 0, fill.gamma_multiply(h));
            } else {
                painter.circle_filled(center, r, fill.gamma_multiply(h));
            }
            let icon = pal().on_accent.gamma_multiply(h);
            if running {
                painter.rect_filled(
                    egui::Rect::from_center_size(center, egui::vec2(16.0, 16.0) * (r / play_r)),
                    rad(2),
                    icon,
                );
            } else {
                play_triangle(&painter, center, 11.0 * (r / play_r), icon);
            }
        }
        drop(over);
        let border = if running && h < 0.5 {
            egui::Stroke::new(1.5, pal().ok.gamma_multiply(0.3 + 0.5 * pulse))
        } else {
            egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h))
        };
        painter.rect_stroke(poster, radius, border, egui::StrokeKind::Inside);
        // A faint accent glow around a lifted poster.
        if h > 0.0 && !(running && h < 0.5) {
            painter.rect_stroke(poster.expand(2.0), rad(12), egui::Stroke::new(3.0, pal().accent.gamma_multiply(0.18 * h)), egui::StrokeKind::Outside);
        }
        // Play was pressed: two rings burst out from the button.
        if let Some(&since) = self.motion.ripple.get(&id) {
            let secs = self.secs(0.7);
            let t = if secs > 0.0 { ((self.now - since) as f32 / secs).clamp(0.0, 1.0) } else { 1.0 };
            if t >= 1.0 {
                self.motion.ripple.remove(&id);
            } else {
                self.motion.busy = true;
                let ring = painter.with_clip_rect(poster.expand(12.0));
                for delay in [0.0, 0.18] {
                    let k = ((t - delay) / (1.0 - delay)).clamp(0.0, 1.0);
                    if k > 0.0 && k < 1.0 {
                        let e = egui::emath::easing::cubic_out(k);
                        let color = pal().accent.gamma_multiply(1.0 - k);
                        ring.circle_stroke(center, play_r * (0.9 + 2.4 * e), egui::Stroke::new(3.0 * (1.0 - k) + 0.5, color));
                    }
                }
            }
        }

        // Title + playtime under the poster; the title warms to the accent as
        // its poster lifts.
        let mut job = egui::text::LayoutJob::simple_singleline(
            disp(&name),
            egui::FontId::proportional(14.5),
            pal().text.lerp_to_gamma(pal().accent, 0.55 * h),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: tile_w(),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let labels = ui.painter();
        let title = labels.layout_job(job);
        let tx = if rtl() { rect.right() - title.size().x } else { rect.left() };
        labels.galley(egui::pos2(tx, rect.top() + tile_h() + 8.0), title, pal().text);
        // After a session the total counts up to its new value, with a fading "+25m".
        let (playtime, added) = self.shown_playtime(&id, stat.playtime_secs);
        let glow = added.as_ref().map_or(0.0, |(_, a)| *a);
        let (sub, sub_color) = if running {
            (tr("Playing now"), pal().ok)
        } else if stat.last_played == 0 {
            (tr("Never played"), pal().faint)
        } else {
            (
                disp(&format!("{} · {}", format_duration(playtime), format_ago(stat.last_played, unix_now()))),
                pal().subtext.lerp_to_gamma(pal().ok, glow),
            )
        };
        let (sx, salign) = if rtl() { (rect.right(), egui::Align2::RIGHT_TOP) } else { (rect.left(), egui::Align2::LEFT_TOP) };
        let sub_rect = labels.text(
            egui::pos2(sx, rect.top() + tile_h() + 30.0),
            salign,
            sub,
            egui::FontId::proportional(12.0),
            sub_color,
        );
        if let Some((delta, alpha)) = added.filter(|_| !running) {
            let (dpos, dalign) = if rtl() {
                (sub_rect.left_top() - egui::vec2(6.0, 0.0), egui::Align2::RIGHT_TOP)
            } else {
                (sub_rect.right_top() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_TOP)
            };
            labels.text(
                dpos,
                dalign,
                disp(&delta),
                egui::FontId::proportional(12.0),
                pal().ok.gamma_multiply(alpha),
            );
        }

        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            let act = match (over_play, running) {
                (true, true) => GameAct::Stop,
                (true, false) => GameAct::Play,
                (false, _) => GameAct::Open,
            };
            acts.game.push((id.clone(), act));
        }
        let game = self.cfg.custom_games[i].clone();
        resp.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &game, running, false, &self.cfg.settings.collections, acts)));
    }
}
