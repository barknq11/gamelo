//! One frame of the window: sidebar, page area, dialogs and repaint pacing.

use crate::*;

impl App {
    /// The normal window: sidebar, status bar and the current page.
    pub(crate) fn draw_desktop(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let ctx = ctx.clone();
        let game_count = self.cfg.custom_games.iter().filter(|g| g.rom.is_none()).count();

        let nav = if rtl() { egui::Panel::right("nav") } else { egui::Panel::left("nav") };
        nav.exact_size(232.0)
            .resizable(false)
            .show_separator_line(true)
            .frame(
                egui::Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(egui::Margin::symmetric(14, 16)),
            )
            .show(ui, |ui| {
                let panel_h = ui.available_height();
                egui::ScrollArea::vertical().id_salt("nav-scroll").auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| dir_ui(ui, |ui| {
                let top = ui.min_rect().top();
                ui.spacing_mut().item_spacing.y = 2.0;
                // Brand
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                    brand_mark(ui, rect);
                    ui.add_space(6.0);
                    ui.label(title_text("Gamelo", 20.0));
                });
                ui.add_space(14.0);
                // Quick search (Ctrl+K)
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
                {
                    let p = ui.painter();
                    let hov = resp.hovered();
                    p.rect_filled(rect, rad(10), pal().field);
                    p.rect_stroke(rect, rad(10), egui::Stroke::new(1.0, if hov { pal().accent.gamma_multiply(0.7) } else { pal().border }), egui::StrokeKind::Inside);
                    let x = |off: f32| if rtl() { rect.right() - off } else { rect.left() + off };
                    glyph(p, Glyph::Search, egui::Rect::from_center_size(egui::pos2(x(20.0), rect.center().y), egui::vec2(16.0, 16.0)), pal().faint);
                    let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
                    p.text(egui::pos2(x(36.0), rect.center().y), align, tr("Quick search"), egui::FontId::proportional(14.0), pal().faint);
                    let chip = p.layout_no_wrap("Ctrl K".into(), egui::FontId::proportional(11.5), pal().faint);
                    let cw = chip.size().x + 12.0;
                    let cx = if rtl() { rect.left() + 8.0 } else { rect.right() - 8.0 - cw };
                    let cr = egui::Rect::from_min_size(egui::pos2(cx, rect.center().y - 10.0), egui::vec2(cw, 20.0));
                    p.rect_stroke(cr, rad(6), egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);
                    p.galley(cr.center() - chip.size() / 2.0, chip, pal().faint);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.quick = Some(quick::Quick { fresh: true, ..Default::default() });
                }
                ui.add_space(14.0);

                // The selection highlight is drawn underneath and slides between items.
                let highlight = ui.painter().add(egui::Shape::Noop);
                let mut selected: Option<egui::Rect> = None;
                let mut item = |ui: &mut egui::Ui, on: bool, icon: NavMark, label: &str, count: Option<usize>, big: bool| {
                    let r = nav_item(ui, on, icon, label, count, big);
                    if on {
                        selected = Some(r.rect);
                    }
                    r.clicked()
                };
                if item(ui, self.page == Page::Home, NavMark::Icon(Glyph::Home), "Home", None, true) {
                    self.page = Page::Home;
                }
                let in_library = matches!(self.page, Page::Library | Page::Game | Page::Artwork | Page::Saves);
                if item(ui, in_library, NavMark::Icon(Glyph::Library), "Library", Some(game_count), true) {
                    self.lib_filter = LibFilter::All;
                    self.page = Page::Library;
                }
                if item(ui, self.page == Page::Stats, NavMark::Icon(Glyph::Stats), "Stats", None, true) {
                    self.page = Page::Stats;
                }

                nav_label(ui, "Sources");
                let on_store = |s: stores::Store| self.page == Page::Stores && self.stores.tab == s;
                let (epic, gog) = (on_store(stores::Store::Epic), on_store(stores::Store::Gog));
                if item(ui, epic, NavMark::Dot(egui::Color32::from_rgb(0xF7, 0xEE, 0xF4)), "Epic Games", None, false) {
                    self.stores.tab = stores::Store::Epic;
                    self.page = Page::Stores;
                }
                if item(ui, gog, NavMark::Dot(egui::Color32::from_rgb(0xB4, 0x8C, 0xFF)), "GOG", None, false) {
                    self.stores.tab = stores::Store::Gog;
                    self.page = Page::Stores;
                }
                if item(ui, self.page == Page::Launchers, NavMark::Dot(egui::Color32::from_rgb(0x6F, 0xB8, 0xFF)), "Launchers", None, false) {
                    self.page = Page::Launchers;
                }
                if item(ui, self.page == Page::Emulation, NavMark::Dot(egui::Color32::from_rgb(0xE6, 0xB5, 0x66)), "Emulation", None, false) {
                    self.page = Page::Emulation;
                }
                let mine = self.page == Page::Library && self.lib_filter == LibFilter::Mine;
                if item(ui, mine, NavMark::Dot(egui::Color32::from_rgb(0x6F, 0xD0, 0x8C)), "Installed by me", None, false) {
                    self.lib_filter = LibFilter::Mine;
                    self.page = Page::Library;
                }

                let collections: Vec<(String, usize)> = self
                    .cfg
                    .settings
                    .collections
                    .iter()
                    .map(|c| (c.clone(), self.cfg.custom_games.iter().filter(|g| g.collections.contains(c)).count()))
                    .collect();
                if !collections.is_empty() {
                    nav_label(ui, "Collections");
                    for (name, n) in collections.iter().take(6) {
                        let on = self.page == Page::Library && self.lib_filter == LibFilter::Collection(name.clone());
                        if item(ui, on, NavMark::None, name, Some(*n), false) {
                            self.lib_filter = LibFilter::Collection(name.clone());
                            self.page = Page::Library;
                        }
                    }
                    if ui.add(egui::Label::new(egui::RichText::new(format!("  + {}", tr("New collection"))).color(pal().faint).small()).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        acts.collection = Some(CollectionEdit { game: None, renaming: None, name: String::new() });
                    }
                }

                // Tools, Couch mode and Settings at the bottom.
                let missing = self.deps.iter().filter(|r| !r.found && r.action.is_some()).count();
                // A newer GE-Proton than any installed one (for people who use GE).
                let uses_ge = self.proton_tools.iter().any(|t| t.label.starts_with("GE-Proton"));
                if uses_ge && !self.proton.releases.contains_key("ge") {
                    self.load_releases(&ctx, 0, false);
                }
                let update = match self.proton.releases.get("ge") {
                    Some(Ok(list)) if uses_ge => list
                        .iter()
                        .find(|r| !r.prerelease)
                        .filter(|r| !self.proton_tools.iter().any(|t| t.label.starts_with(&r.tag)))
                        .map(|r| r.tag.clone()),
                    _ => None,
                };
                let mut playing: Vec<String> = self.running.values().map(|m| m.name.clone()).collect();
                playing.sort_unstable();
                let umu_ok = self.deps.iter().any(|r| r.name == "umu-run" && r.found);
                let footer_h = 36.0 + 3.0 * 38.0 + 6.0 + 2.0 * 44.0 + 4.0
                    + if umu_ok { 0.0 } else { 28.0 }
                    + 24.0 * playing.len() as f32
                    + 46.0 * self.tasks.len() as f32;
                let used = ui.min_rect().bottom() - top;
                if panel_h > used + footer_h {
                    ui.add_space(panel_h - used - footer_h);
                }
                nav_label(ui, "Tools");
                let r = nav_item(ui, self.page == Page::Proton, NavMark::None, "Proton", None, false);
                if let Some(tag) = &update {
                    let dot = if rtl() { egui::pos2(r.rect.left() + 16.0, r.rect.center().y) } else { egui::pos2(r.rect.right() - 16.0, r.rect.center().y) };
                    ui.painter().circle_filled(dot, 4.0, pal().accent);
                    let _ = r.clone().on_hover_text(trf("{} is available", &[tag]));
                }
                if self.page == Page::Proton {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Proton;
                }
                let deps_count = (missing > 0).then_some(missing);
                let r = nav_item(ui, self.page == Page::Dependencies, NavMark::None, "Dependencies", deps_count, false);
                if self.page == Page::Dependencies {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Dependencies;
                }
                let r = nav_item(ui, self.page == Page::Cleanup, NavMark::None, "Disk space", None, false);
                if self.page == Page::Cleanup {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Cleanup;
                }
                ui.add_space(6.0);
                if nav_item(ui, false, NavMark::Icon(Glyph::Couch), "Couch mode", None, true)
                    .on_hover_text(tr("Fullscreen, for the sofa and a controller (F11 or the Guide button)"))
                    .clicked()
                {
                    self.couch_toggle(&ctx);
                }
                let r = nav_item(ui, self.page == Page::Settings, NavMark::Icon(Glyph::Settings), "Settings", None, true);
                if self.page == Page::Settings {
                    selected = Some(r.rect);
                }
                if r.clicked() {
                    self.page = Page::Settings;
                }
                if let Some(r) = selected {
                    let (now, secs) = (self.now, self.secs(0.22));
                    let tw = self.motion.nav_y.get_or_insert(Tween::at(r.top(), now));
                    let y = tw.follow(r.top(), now, secs);
                    self.motion.busy |= !tw.settled(now, secs);
                    let hr = egui::Rect::from_min_size(egui::pos2(r.left(), y), r.size());
                    ui.painter().set(highlight, egui::Shape::rect_filled(hr, rad(10), pal().card2));
                }

                // Status footer: only what needs attention.
                if !umu_ok {
                    status_line(ui, true, "umu-run", "missing");
                }
                for name in &playing {
                    status_line(ui, false, &tr("Playing"), &disp(name));
                }
                for t in &self.tasks {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(egui::RichText::new(tr(&t.label)).color(pal().subtext).small()).truncate());
                    let bar = egui::ProgressBar::new(t.fraction()).desired_height(6.0).corner_radius(rad(3));
                    ui.add(if t.progress.is_some() { bar } else { bar.animate(true) });
                }
            }));
            });

        egui::Panel::bottom("status")
            .show_separator_line(true)
            .frame(
                egui::Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| dir_ui(ui, |ui| {
                ui.horizontal(|ui| {
                    let color = if self.status_err {
                        pal().warn
                    } else if self.status.is_empty() {
                        pal().subtext
                    } else {
                        pal().info()
                    };
                    // New messages fade and slide in, so a change is noticed.
                    let t = self.progress(self.status_since, 0.3);
                    status_dot(ui, color, !self.status.is_empty() || self.status_err);
                    ui.add_space((1.0 - t) * 8.0);
                    let msg = if self.status.is_empty() { tr("Ready") } else { tr(&self.status) };
                    let label = ui.add(
                        egui::Label::new(
                            egui::RichText::new(msg).color(color.gamma_multiply(0.3 + 0.7 * t)).small(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    // While a controller is in use, a reminder of what its buttons do.
                    let pad_recent = self.pad.as_ref().and_then(|p| p.nav.last_used).is_some_and(|t| self.now - t < 20.0);
                    if pad_recent {
                        ui.with_layout(far_end(), |ui| {
                            ui.label(egui::RichText::new(tr("A open · B back · Start play · LB/RB tabs")).color(pal().faint).small());
                        });
                    }
                    if !self.status.is_empty() {
                        // Errors often end in "— see <log>": offer to open it.
                        let log = self
                            .status
                            .split_whitespace()
                            .rev()
                            .map(|w| w.trim_matches(|c| matches!(c, '\u{2066}'..='\u{2069}')))
                            .find(|w| w.ends_with(".log"))
                            .map(str::to_string);
                        let message = self.status.clone();
                        label.context_menu(|ui| menu_ui(ui, |ui| {
                            if ui.button(tr("Copy message")).clicked() {
                                acts.copy = Some(message.clone());
                                ui.close();
                            }
                            if let Some(log) = &log
                                && ui.button(tr("Open log")).clicked()
                            {
                                acts.open = Some(log.clone());
                                ui.close();
                            }
                        }));
                    }
                });
                ui.add_space(5.0);
            }));

        // Pages fade and rise into place when you switch to them.
        if self.motion.page != Some(self.page) {
            let prev = self.motion.page.replace(self.page);
            self.cache_bytes = None;
            self.motion.page_since = self.now;
            self.start_flight(&ctx, prev);
        }
        let enter = self.progress(self.motion.page_since, 0.24);
        egui::CentralPanel::default().show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin { left: 26, right: 14, top: 22, bottom: 22 })
                .show(ui, |ui| {
                    // Each page (and each game) keeps its own scroll position.
                    egui::ScrollArea::vertical()
                        .id_salt((self.page, self.selected.clone()))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.multiply_opacity(enter);
                            ui.add_space((1.0 - enter) * 14.0);
                            // The right stick scrolls; so do Up/Down outside the library grid.
                            let mut dy = -self.pad_scroll;
                            if self.page != Page::Library && ctx.memory(|m| m.focused().is_none()) && !overlay_open(&ctx) {
                                let (up, down) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                                dy += 120.0 * (up as i32 - down as i32) as f32;
                            }
                            if dy != 0.0 {
                                ui.scroll_with_delta(egui::vec2(0.0, dy));
                            }
                            // Keep content clear of the floating scroll bar on the right
                            // (right-to-left pages are flush against it otherwise).
                            let edge = egui::Margin { right: 12, ..Default::default() };
                            egui::Frame::new().inner_margin(edge).show(ui, |ui| dir_ui(ui, |ui| {
                                self.safety_banner(ui);
                                match self.page {
                                Page::Home => self.page_home(ui, &ctx, acts),
                                Page::Launchers => self.page_launchers(ui, &ctx, acts),
                                Page::Emulation => self.page_emulation(ui, &ctx, acts),
                                Page::Stats => self.page_stats(ui, &ctx, acts),
                                Page::Library => self.page_library(ui, &ctx, acts),
                                Page::Game => self.page_game(ui, &ctx, acts),
                                Page::Artwork => self.page_artwork(ui, &ctx, acts),
                                Page::Settings => self.page_settings(ui, acts),
                                Page::Dependencies => self.page_dependencies(ui, acts),
                                Page::Proton => self.page_proton(ui, &ctx, acts),
                                Page::Saves => self.page_saves(ui, &ctx, acts),
                                Page::Stores => self.page_stores(ui, &ctx, acts),
                                Page::Cleanup => self.page_cleanup(ui, &ctx, acts),
                                }
                            }));
                        });
                });
        });
    }

    /// One frame of the whole window (kept separate from eframe so it can be
    /// rendered offscreen by the screenshot test).
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.motion.busy = false;
        self.motion.looping = false;
        self.motion.focused = ctx.input(|i| i.focused);
        // egui's own transitions (hover fades, collapsing) follow "Reduce motion" too.
        let anim = if self.cfg.settings.reduce_motion { 0.0 } else { 1.0 / 12.0 };
        if (ctx.global_style().animation_time - anim).abs() > 1e-6 {
            ctx.all_styles_mut(|s| s.animation_time = anim);
        }
        self.sync_theme(&ctx);
        set_tile_size(&self.cfg.settings.tile_size);
        self.proton_poll();
        self.saves_poll();
        self.scan_poll();
        self.hltb_poll();
        self.store_poll(&ctx);
        self.stores_poll(&ctx);
        self.cleanup_poll();
        self.opti_poll();
        self.poll_tasks(&ctx);
        self.art_poll(&ctx);
        self.poll_running();

        // Keep the configured UI scale in sync with egui's zoom. If the user
        // changed zoom via the built-in shortcuts (Ctrl +/- or Ctrl+scroll),
        // adopt that value; otherwise apply the configured one.
        let cur = ctx.zoom_factor();
        let want = self.cfg.settings.zoom();
        if (cur - self.last_zoom).abs() > 1e-4 {
            // Ctrl +/−, Ctrl 0 or Ctrl+scroll: adopt it, and save once it settles.
            self.cfg.settings.zoom = cur;
            self.last_zoom = cur;
            self.zoom_save_at = Some(self.now + 0.8);
        } else if (want - cur).abs() > 1e-4 {
            ctx.set_zoom_factor(want);
            self.last_zoom = want;
        }

        // Poll any in-flight download job.
        let mut dl_result: Option<Result<String, String>> = None;
        let mut dl_label = String::new();
        if let Some(dl) = &self.download {
            match dl.rx.try_recv() {
                Ok(r) => dl_result = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    dl_result = Some(Err(lg("download task ended unexpectedly").into()));
                }
            }
            if dl_result.is_some() {
                dl_label = dl.label.clone();
            }
        }
        if let Some(r) = dl_result {
            self.download = None;
            match r {
                Ok(msg) => {
                    self.refresh();
                    self.set_status(msg, false);
                }
                Err(e) => self.set_status(format!("{dl_label}: {e}"), true),
            }
        }

        let mut acts = Acts::default();
        // F11 (a controller's Guide button) toggles couch mode.
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) && ctx.memory(|m| m.focused().is_none()) {
            self.couch_toggle(&ctx);
        }
        if self.couch.is_some() {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
                .show(ui, |ui| self.couch_ui(ui, &ctx, &mut acts));
        } else {
            self.draw_desktop(ui, &ctx, &mut acts);
        }

        self.setup_modal(&ctx, &mut acts);
        if let Some(m) = self.add_modal_ui(&ctx) {
            self.add_game(&ctx, m);
        }
        self.remove_modal_ui(&ctx, &mut acts);
        self.proton_delete_modal(&ctx);
        self.launcher_remove_modal(&ctx);
        for key in std::mem::take(&mut self.launchers_due) {
            if let Some(d) = launchers::def(&key) {
                let n = self.launchers_sync(&ctx, d);
                if n > 0 {
                    self.set_status(lfmt("Added {} from {} to your library", &[&n, &d.name]), false);
                }
            }
        }
        self.saves_modals(&ctx);
        self.hltb_modal(&ctx);
        self.quick_modal(&ctx, &mut acts);
        self.tray_poll(&ctx, &mut acts);
        self.store_picker(&ctx);
        self.screenshot_viewer(&ctx);
        self.log_viewer(&ctx);
        if let Some(list) = self.scan_modal(&ctx) {
            self.add_games(&ctx, list);
        }
        self.steam_import_modal(&ctx);
        self.collection_modal(&ctx);
        self.stores_modals(&ctx);
        self.apply_acts(&ctx, acts);

        if let Some(at) = self.zoom_save_at {
            if self.now >= at {
                self.zoom_save_at = None;
                save_config(&self.cfg);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(at - self.now));
            }
        }
        // A flight whose destination wasn't drawn (filtered out, say) just ends.
        if self.motion.flight.as_ref().is_some_and(|f| self.now - f.since > 0.5) {
            self.motion.flight = None;
        }
        if self.motion.busy {
            ctx.request_repaint();
        } else if self.motion.looping && ctx.input(|i| i.focused) {
            // Loops only run while the window has focus, so they cost nothing mid-game.
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            // Notice games started or stopped outside the window (app-menu shortcuts).
            let tick = if self.running.is_empty() { 3 } else { 1 };
            ctx.request_repaint_after(Duration::from_secs(tick));
        }
    }
}
