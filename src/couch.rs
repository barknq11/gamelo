//! Couch mode: a fullscreen, controller-first home screen. One row of posters
//! along the bottom; the focused game's art fills the screen behind its name,
//! playtime and a big Play button. Toggled with F11, the controller's Guide
//! button or the sidebar.
//!
//! Keys (a controller sends the same through `gamepad`): Left/Right move,
//! Enter (A) plays or stops, I (Y/X) shows details, F (View) stars the game,
//! [ / ] (LB/RB) change the filter, Space (Start) changes the sort, Escape (B)
//! closes details or leaves couch mode.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;

pub struct Couch {
    /// The focused game (by id, so it survives re-sorting).
    pub focus: Option<String>,
    pub details: bool,
    pub since: f64,
    /// Carousel scroll, eased toward the focused poster.
    scroll: Option<Tween>,
    /// Background art cross-fading in: (game id, since), newest last.
    bg: Vec<(String, f64)>,
}

impl Couch {
    pub fn new(now: f64) -> Couch {
        Couch { focus: None, details: false, since: now, scroll: None, bg: Vec::new() }
    }
}

/// "21:45", local time.
fn clock() -> String {
    // SAFETY: localtime_r only writes into the struct we pass.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// A full-screen dark gradient: `a` at one edge to `b` at the other.
fn shade(painter: &egui::Painter, rect: egui::Rect, horizontal: bool, a: egui::Color32, b: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    let (c0, c1, c2, c3) = if horizontal { (a, b, b, a) } else { (a, a, b, b) };
    mesh.colored_vertex(rect.left_top(), c0);
    mesh.colored_vertex(rect.right_top(), c1);
    mesh.colored_vertex(rect.right_bottom(), c2);
    mesh.colored_vertex(rect.left_bottom(), c3);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

impl App {
    pub fn couch_toggle(&mut self, ctx: &egui::Context) {
        let on = self.couch.is_none();
        self.couch = on.then(|| Couch::new(self.now));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    /// Keyboard/controller input for the home screen.
    fn couch_keys(&mut self, ctx: &egui::Context, order: &[usize], acts: &mut Acts) {
        if ctx.memory(|m| m.focused().is_some()) || overlay_open(ctx) {
            return;
        }
        let k = |key| ctx.input(|i| i.key_pressed(key));
        let (left, right, enter, info, fav, prev, next, sort, back) = (
            k(egui::Key::ArrowLeft),
            k(egui::Key::ArrowRight),
            k(egui::Key::Enter),
            k(egui::Key::I),
            k(egui::Key::F),
            k(egui::Key::OpenBracket),
            k(egui::Key::CloseBracket),
            k(egui::Key::Space),
            k(egui::Key::Escape),
        );
        let wheel = ctx.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        let options: Vec<LibFilter> = self.filter_options().into_iter().map(|(f, _)| f).collect();
        let Some(c) = self.couch.as_mut() else { return };
        if back {
            if c.details {
                c.details = false;
            } else {
                self.couch_toggle(ctx);
            }
            return;
        }
        let games = &self.cfg.custom_games;
        let cur = c.focus.as_ref().and_then(|id| order.iter().position(|&i| &games[i].id == id)).unwrap_or(0);
        if !c.details {
            // In Arabic the row runs right to left, so Left moves forward.
            let mut step = right as isize - left as isize;
            if rtl() {
                step = -step;
            }
            if wheel.abs() > 30.0 {
                step += if wheel < 0.0 { 1 } else { -1 };
            }
            if step != 0 && !order.is_empty() {
                let n = (cur as isize + step).clamp(0, order.len() as isize - 1) as usize;
                c.focus = Some(games[order[n]].id.clone());
            }
        }
        if prev != next {
            let at = options.iter().position(|f| *f == self.lib_filter).unwrap_or(0);
            let n = options.len();
            self.lib_filter = options[if next { (at + 1) % n } else { (at + n - 1) % n }].clone();
            c.focus = None;
        }
        if sort {
            self.sort = match self.sort {
                SortMode::Recent => SortMode::Name,
                SortMode::Name => SortMode::Playtime,
                SortMode::Playtime => SortMode::Recent,
            };
            self.cfg.settings.library_sort = self.sort.key().into();
            save_config(&self.cfg);
        }
        if let Some(&i) = order.get(cur) {
            let id = games[i].id.clone();
            if info {
                c.details = !c.details;
            }
            // View stars the game; inside the details panel it steps the status instead.
            if fav && c.details {
                acts.game.push((id.clone(), GameAct::SetStatus(games[i].status.next())));
            } else if fav {
                acts.game.push((id.clone(), GameAct::Favorite));
            }
            if enter {
                let act = if self.running.contains_key(&id) { GameAct::Stop } else { GameAct::Play };
                acts.game.push((id, act));
            }
        }
    }

    pub fn couch_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let order = self.library_order_for("");
        self.couch_keys(ctx, &order, acts);
        let Some(c) = self.couch.as_mut() else { return };
        let games = &self.cfg.custom_games;
        let cur = c.focus.as_ref().and_then(|id| order.iter().position(|&i| &games[i].id == id)).unwrap_or(0);
        if let Some(&i) = order.get(cur) {
            c.focus = Some(games[i].id.clone());
        }
        let focus = order.get(cur).copied();
        let screen = ui.max_rect();
        let painter = ui.painter().clone();
        let now = self.now;
        let enter = self.progress(self.couch.as_ref().map_or(now, |c| c.since), 0.45);

        // Background: the focused game's art, cross-fading when focus moves.
        if let Some(i) = focus {
            let id = self.cfg.custom_games[i].id.clone();
            let fade = self.secs(0.45);
            let c = self.couch.as_mut().unwrap();
            if c.bg.last().is_none_or(|(b, _)| *b != id) {
                c.bg.push((id, now));
            }
            // Keep only what's still visible: the newest layer once it has fully faded in.
            if let Some(pos) = c.bg.iter().rposition(|(_, t)| now - t >= f64::from(fade)) {
                c.bg.drain(..pos);
            }
            let layers = c.bg.clone();
            for (bid, since) in layers {
                let Some(bi) = self.index_of(&bid) else { continue };
                let a = ease_since(now, since, fade);
                if a < 1.0 {
                    self.motion.busy = true;
                }
                if let Some(t) = self.game_hero(ctx, bi) {
                    let zoom = 1.06 - 0.04 * a;
                    painter.add(cover_fill_shape_pan(&t, screen, 0, a * enter, zoom, egui::vec2(0.5, 0.35)));
                }
            }
        }
        let black = |a: u8| egui::Color32::from_black_alpha(a);
        let (side_a, side_b) = if rtl() { (black(40), black(230)) } else { (black(230), black(40)) };
        shade(&painter, screen, true, side_a, side_b);
        shade(&painter, screen, false, black(60), black(250));

        let over = theme::OverImage::new();
        let w = screen.width();
        let hgt = screen.height();
        let margin = (w * 0.05).clamp(32.0, 80.0);
        let start_x = if rtl() { screen.right() - margin } else { screen.left() + margin };
        let anchor = if rtl() { egui::Align2::RIGHT_TOP } else { egui::Align2::LEFT_TOP };

        // Top bar: filter, sort and the time.
        let top = screen.top() + 28.0;
        // The filters, as many as fit before the clock; the current one is underlined.
        let limit = w * 0.62;
        let mut x = start_x;
        for (f, label) in self.filter_options() {
            let on = self.lib_filter == f;
            let color = if on { pal().text } else { pal().subtext };
            let galley = painter.layout_no_wrap(disp(&label), egui::FontId::proportional(18.0), color);
            let gw = galley.size().x;
            if (x - start_x).abs() + gw > limit {
                break;
            }
            let gx = if rtl() { x - gw } else { x };
            painter.galley(egui::pos2(gx, top), galley, color);
            if on {
                painter.rect_filled(egui::Rect::from_min_size(egui::pos2(gx, top + 28.0), egui::vec2(gw, 3.0)), rad(2), pal().accent);
            }
            x += if rtl() { -(gw + 28.0) } else { gw + 28.0 };
        }
        let sort = lfmt("Sort: {}", &[&lg(self.sort.label())]);
        let sort_pos = if rtl() { egui::pos2(x - 12.0, top + 2.0) } else { egui::pos2(x + 12.0, top + 2.0) };
        painter.text(sort_pos, anchor, disp(&sort), egui::FontId::proportional(15.0), pal().faint);
        let (clock_pos, clock_align) = if rtl() {
            (egui::pos2(screen.left() + margin, top - 2.0), egui::Align2::LEFT_TOP)
        } else {
            (egui::pos2(screen.right() - margin, top - 2.0), egui::Align2::RIGHT_TOP)
        };
        painter.text(clock_pos, clock_align, clock(), egui::FontId::proportional(22.0), pal().text);

        // The posters along the bottom.
        let ph = (hgt * 0.3).clamp(170.0, 330.0);
        let pw = ph * 2.0 / 3.0;
        let gap = (pw * 0.14).max(16.0);
        let row_y = screen.bottom() - ph - 70.0;
        let target = cur as f32 * (pw + gap);
        let scroll = {
            let secs = self.secs(0.32);
            let c = self.couch.as_mut().unwrap();
            let tw = c.scroll.get_or_insert(Tween::at(target, now));
            let v = tw.follow(target, now, secs);
            if !tw.settled(now, secs) {
                self.motion.busy = true;
            }
            v
        };
        let mut clicked: Option<(usize, bool)> = None;
        for (k, &gi) in order.iter().enumerate() {
            let offset = k as f32 * (pw + gap) - scroll;
            let x0 = if rtl() { start_x - offset - pw } else { start_x + offset };
            if x0 > screen.right() + pw || x0 + pw < screen.left() - pw {
                continue;
            }
            let id = self.cfg.custom_games[gi].id.clone();
            let focused = k == cur;
            let f = ctx.animate_bool_with_time_and_easing(
                egui::Id::unique(("couch-focus", &id)),
                focused,
                self.secs(0.28),
                egui::emath::easing::back_out,
            );
            let base = egui::Rect::from_min_size(egui::pos2(x0, row_y), egui::vec2(pw, ph));
            let rect = egui::Rect::from_center_size(base.center() - egui::vec2(0.0, 14.0 * f), base.size() * (1.0 + 0.16 * f));
            let resp = ui.interact(base, egui::Id::unique(("couch-tile", &id)), egui::Sense::click());
            if resp.clicked() {
                clicked = Some((k, focused));
            }
            let alpha = (0.55 + 0.45 * f.clamp(0.0, 1.0)) * enter;
            if f > 0.0 {
                painter.rect_filled(rect.translate(egui::vec2(0.0, 10.0)).expand(6.0 * f), rad(16), black((110.0 * f.min(1.0)) as u8));
            }
            if let Some(t) = self.game_poster(ctx, gi).or_else(|| self.game_generated_poster(ctx, gi)) {
                painter.add(cover_fill_shape(&t, rect, 12, alpha));
            }
            if f > 0.0 {
                let glow = pal().accent.gamma_multiply(f.clamp(0.0, 1.0));
                painter.rect_stroke(rect, rad(12), egui::Stroke::new(3.0, glow), egui::StrokeKind::Outside);
                painter.rect_stroke(rect.expand(5.0), rad(16), egui::Stroke::new(4.0, pal().accent.gamma_multiply(0.2 * f.clamp(0.0, 1.0))), egui::StrokeKind::Outside);
            }
            if self.running.contains_key(&id) {
                poster_badge(&painter, rect.left_top() + egui::vec2(8.0, 8.0), "Running", pal().ok, 1.0);
            }
            if self.cfg.custom_games[gi].favorite {
                let c = rect.right_top() + egui::vec2(-18.0, 18.0);
                painter.circle_filled(c, 13.0, black(170));
                star(&painter, c, 8.0, true, GOLD);
            }
        }
        if let Some((k, focused)) = clicked {
            let id = self.cfg.custom_games[order[k]].id.clone();
            if focused {
                let act = if self.running.contains_key(&id) { GameAct::Stop } else { GameAct::Play };
                acts.game.push((id, act));
            } else if let Some(c) = self.couch.as_mut() {
                c.focus = Some(id);
            }
        }

        // The focused game: name, facts and the Play button above the row.
        match focus {
            // Clear of the focused poster, which grows and rises above the row.
            Some(gi) => self.couch_info(ui, ctx, gi, screen, margin, row_y - 0.16 * ph - 24.0, enter, acts),
            None => {
                painter.text(
                    screen.center(),
                    egui::Align2::CENTER_CENTER,
                    tr("No games here."),
                    egui::FontId::proportional(28.0),
                    pal().subtext,
                );
            }
        }

        // What the buttons do.
        let hints = tr("A Play · Y Details · View Favorite · LB/RB Filter · Start Sort · B Exit");
        painter.text(
            egui::pos2(screen.center().x, screen.bottom() - 26.0),
            egui::Align2::CENTER_CENTER,
            hints,
            egui::FontId::proportional(15.0),
            pal().faint,
        );
        drop(over);

        if let (Some(gi), true) = (focus, self.couch.as_ref().is_some_and(|c| c.details)) {
            self.couch_details(ui, ctx, gi, screen, acts);
        }
    }

    /// Name, playtime, badges, the description and the Play button.
    #[allow(clippy::too_many_arguments)]
    fn couch_info(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, gi: usize, screen: egui::Rect, margin: f32, row_y: f32, enter: f32, acts: &mut Acts) {
        let g = self.cfg.custom_games[gi].clone();
        let stat = self.stats.get(&g.id).cloned().unwrap_or_default();
        let info = self.store.cache.get(&g.id).and_then(|e| e.info.clone());
        if info.is_none() && !self.cfg.settings.no_store {
            self.store_ensure(ctx, gi);
        }
        let width = (screen.width() * 0.5).max(420.0);
        let area = if rtl() {
            egui::Rect::from_min_max(egui::pos2(screen.right() - margin - width, screen.top() + 90.0), egui::pos2(screen.right() - margin, row_y - 30.0))
        } else {
            egui::Rect::from_min_max(egui::pos2(screen.left() + margin, screen.top() + 90.0), egui::pos2(screen.left() + margin + width, row_y - 30.0))
        };
        let (play_state, phase) = self.play_state(&g.id);
        let align = if rtl() { egui::Align::Max } else { egui::Align::Min };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::bottom_up(align)));
        child.multiply_opacity(enter);
        // Bottom-up: the Play button first, the name last.
        if play_button(&mut child, play_state, phase, 0.0).clicked() {
            let act = if self.running.contains_key(&g.id) { GameAct::Stop } else { GameAct::Play };
            acts.game.push((g.id.clone(), act));
        }
        child.add_space(18.0);
        if let Some(info) = &info
            && !info.about.is_empty()
        {
            let short: String = info.about.chars().take(260).collect();
            let short = if info.about.chars().count() > 260 { format!("{short}…") } else { short };
            child.scope(|ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::proportional(17.0));
                para(ui, &short, pal().subtext, true);
            });
            child.add_space(14.0);
        }
        child.horizontal_wrapped(|ui| {
            if g.status != GameStatus::None {
                pill(ui, g.status.label(), g.status.color());
            }
            if let Some((label, color)) = info.as_ref().and_then(|i| crate::store::tier_style(&i.protondb)) {
                pill(ui, &lfmt("ProtonDB: {}", &[&lg(label)]), color);
            }
            if let Some(t) = self.hltb.cache.get(&g.id).and_then(|e| e.times.clone()).filter(|t| t.main > 0) {
                pill(ui, &lfmt("Main story: {}", &[&hltb::hours(t.main)]), pal().info());
            }
            for genre in info.iter().flat_map(|i| i.genres.iter().take(3)) {
                pill(ui, genre, pal().subtext);
            }
        });
        child.add_space(10.0);
        let played = if stat.last_played == 0 {
            tr("Never played")
        } else {
            crate::i18n::trf("{} played · last played {}", &[&format_duration(stat.playtime_secs), &format_ago(stat.last_played, unix_now())])
        };
        child.label(egui::RichText::new(played).size(18.0).color(pal().subtext));
        child.add_space(4.0);
        child.add(egui::Label::new(egui::RichText::new(disp(&g.name)).size(54.0).strong().color(pal().text)).truncate());
    }

    /// The details panel over the home screen.
    fn couch_details(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, gi: usize, screen: egui::Rect, acts: &mut Acts) {
        let g = self.cfg.custom_games[gi].clone();
        let info = self.store.cache.get(&g.id).and_then(|e| e.info.clone());
        let stat = self.stats.get(&g.id).cloned().unwrap_or_default();
        let painter = ui.painter().clone();
        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(200));
        let panel = screen.shrink2(egui::vec2((screen.width() * 0.12).max(40.0), (screen.height() * 0.1).max(30.0)));
        painter.rect_filled(panel, rad(20), pal().card);
        painter.rect_stroke(panel, rad(20), egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel.shrink(36.0)).layout(egui::Layout::top_down(if rtl() { egui::Align::Max } else { egui::Align::Min })));
        child.label(egui::RichText::new(disp(&g.name)).size(40.0).strong().color(pal().text));
        child.add_space(6.0);
        let status = lfmt("Status: {} (View changes it)", &[&lg(g.status.label())]);
        child.label(egui::RichText::new(disp(&status)).size(18.0).color(g.status.color()));
        child.add_space(14.0);
        egui::ScrollArea::vertical().id_salt(("couch-details", &g.id)).show(&mut child, |ui| {
            ui.style_mut().override_text_style = Some(egui::TextStyle::Heading);
            if let Some(info) = &info {
                if !info.about.is_empty() {
                    para(ui, &info.about, pal().text, true);
                    ui.add_space(16.0);
                }
                for (label, value) in [("Developer", info.developers.join(", ")), ("Publisher", info.publishers.join(", ")), ("Released", info.released.clone())] {
                    if !value.is_empty() {
                        ui.label(egui::RichText::new(disp(&format!("{}: {value}", lg(label)))).size(19.0).color(pal().subtext));
                    }
                }
                ui.add_space(16.0);
            }
            if stat.last_played > 0 {
                ui.label(egui::RichText::new(crate::i18n::trf("{} played · last played {}", &[&format_duration(stat.playtime_secs), &format_ago(stat.last_played, unix_now())])).size(19.0).color(pal().text));
            }
            if let Some(t) = self.hltb.cache.get(&g.id).and_then(|e| e.times.clone()) {
                let line = lfmt("Main story {} · Main + extras {} · Completionist {}", &[&hltb::hours(t.main), &hltb::hours(t.plus), &hltb::hours(t.full)]);
                ui.label(egui::RichText::new(disp(&line)).size(19.0).color(pal().subtext));
            }
            if let Some(info) = &info {
                ui.add_space(18.0);
                ui.horizontal_wrapped(|ui| {
                    for (thumb, _) in info.shots.iter().take(6) {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(320.0, 180.0), egui::Sense::hover());
                        match self.store_image(ctx, thumb) {
                            Some(t) => {
                                ui.painter().add(cover_fill_shape(&t, rect, 10, 1.0));
                            }
                            None => {
                                ui.painter().rect_filled(rect, rad(10), pal().card2);
                            }
                        }
                    }
                });
            }
        });
        let hint = tr("A Play · B Back");
        painter.text(egui::pos2(panel.center().x, panel.bottom() - 22.0), egui::Align2::CENTER_CENTER, hint, egui::FontId::proportional(15.0), pal().faint);
        let _ = acts;
    }
}
