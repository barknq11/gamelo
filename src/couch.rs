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

/// A row of games in couch mode: a library filter, or one console's ROMs.
#[derive(Clone, PartialEq)]
pub enum Shelf {
    Filter(LibFilter),
    Console(String),
}

pub struct Couch {
    /// The focused game (by id, so it survives re-sorting).
    pub focus: Option<String>,
    /// The shelf shown (the library's own filter for Filter shelves).
    pub console: Option<String>,
    pub details: bool,
    pub since: f64,
    /// Carousel scroll, eased toward the focused poster.
    scroll: Option<Tween>,
    /// Background art cross-fading in: (game id, since), newest last.
    bg: Vec<(String, f64)>,
}

impl Couch {
    pub fn new(now: f64) -> Couch {
        Couch { focus: None, console: None, details: false, since: now, scroll: None, bg: Vec::new() }
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
        let shelves: Vec<Shelf> = self.couch_shelves().into_iter().map(|(s, _)| s).collect();
        let current = self.couch_shelf();
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
            let at = shelves.iter().position(|s| *s == current).unwrap_or(0);
            let n = shelves.len();
            match &shelves[if next { (at + 1) % n } else { (at + n - 1) % n }] {
                Shelf::Filter(f) => {
                    self.lib_filter = f.clone();
                    c.console = None;
                }
                Shelf::Console(k) => c.console = Some(k.clone()),
            }
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

    /// The shelves: the library's filters, then a shelf per console with ROMs.
    pub fn couch_shelves(&self) -> Vec<(Shelf, String)> {
        let mut out: Vec<(Shelf, String)> = self
            .filter_options()
            .into_iter()
            .map(|(f, label)| (Shelf::Filter(f.clone()), if f == LibFilter::All { lg("All games").to_string() } else { label }))
            .collect();
        for p in emulation::PLATFORMS {
            if self.cfg.custom_games.iter().any(|g| g.rom.as_ref().is_some_and(|r| r.platform == p.key)) {
                out.push((Shelf::Console(p.key.to_string()), p.name.to_string()));
            }
        }
        out
    }

    fn couch_shelf(&self) -> Shelf {
        match self.couch.as_ref().and_then(|c| c.console.clone()) {
            Some(k) => Shelf::Console(k),
            None => Shelf::Filter(self.lib_filter.clone()),
        }
    }

    /// The games on the current shelf, in the library's order.
    fn couch_order(&self) -> Vec<usize> {
        match self.couch_shelf() {
            Shelf::Filter(_) => self.library_order_for(""),
            Shelf::Console(k) => {
                let games = &self.cfg.custom_games;
                let mut v: Vec<usize> = (0..games.len()).filter(|&i| games[i].rom.as_ref().is_some_and(|r| r.platform == k)).collect();
                let last = |i: usize| self.stats.get(&games[i].id).map_or(0, |s| s.last_played);
                v.sort_by_key(|&i| (std::cmp::Reverse(last(i)), games[i].name.to_lowercase()));
                v
            }
        }
    }

    pub fn couch_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let order = self.couch_order();
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
        // The design's plain night backdrop, with the art only as a faint glow.
        let black = |a: u8| egui::Color32::from_black_alpha(a);
        shade(&painter, screen, false, pal().sidebar.gamma_multiply(0.82), pal().sidebar);

        let over = theme::OverImage::new();
        let w = screen.width();
        let hgt = screen.height();
        let margin = (w * 0.045).clamp(32.0, 64.0);
        let start_x = if rtl() { screen.right() - margin } else { screen.left() + margin };
        let start_align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };

        // Top bar: the logo, the shelves and the time.
        let bar_y = screen.top() + 40.0 + 24.0;
        let logo = egui::Rect::from_center_size(egui::pos2(if rtl() { start_x - 20.0 } else { start_x + 20.0 }, bar_y), egui::vec2(40.0, 40.0));
        brand_mark(ui, logo);
        let name_x = if rtl() { logo.left() - 14.0 } else { logo.right() + 14.0 };
        painter.text(egui::pos2(name_x, bar_y), start_align, "Gamelo", egui::FontId::new(24.0, heading_family()), pal().text);
        let (clock_pos, clock_align) = if rtl() {
            (egui::pos2(screen.left() + margin, bar_y), egui::Align2::LEFT_CENTER)
        } else {
            (egui::pos2(screen.right() - margin, bar_y), egui::Align2::RIGHT_CENTER)
        };
        painter.text(clock_pos, clock_align, clock(), egui::FontId::new(22.0, heading_family()), pal().subtext);
        // Shelves, centred; as many as fit between the logo and the clock.
        let current = self.couch_shelf();
        let font = egui::FontId::new(17.0, semibold_family());
        let mut tabs: Vec<(Shelf, std::sync::Arc<egui::Galley>, bool)> = Vec::new();
        let mut total = 0.0;
        for (s, label) in self.couch_shelves() {
            let on = s == current;
            let galley = painter.layout_no_wrap(disp(&label), font.clone(), if on { pal().bg } else { pal().subtext });
            let tw = galley.size().x + 44.0;
            if total + tw > w * 0.55 && !on {
                continue;
            }
            total += tw + 8.0;
            tabs.push((s, galley, on));
        }
        let mut x = screen.center().x - total / 2.0;
        let mut pick_shelf = None;
        let ordered: Vec<_> = if rtl() { tabs.into_iter().rev().collect() } else { tabs };
        for (s, galley, on) in ordered {
            let tw = galley.size().x + 44.0;
            let r = egui::Rect::from_min_size(egui::pos2(x, bar_y - 24.0), egui::vec2(tw, 48.0));
            let resp = ui.interact(r, egui::Id::unique(("couch-shelf", x as i32)), egui::Sense::click());
            if on {
                painter.rect_filled(r, rad(24), pal().text);
            } else if resp.hovered() {
                painter.rect_filled(r, rad(24), pal().card2);
            }
            painter.galley(r.center() - galley.size() / 2.0, galley, pal().text);
            if resp.clicked() {
                pick_shelf = Some(s);
            }
            x += tw + 8.0;
        }
        if let Some(s) = pick_shelf {
            let c = self.couch.as_mut().unwrap();
            match s {
                Shelf::Filter(f) => {
                    self.lib_filter = f;
                    c.console = None;
                }
                Shelf::Console(k) => c.console = Some(k),
            }
            c.focus = None;
        }

        // The carousel, centred: the focused game larger with an accent outline,
        // one game before it so the row reads as a row.
        let ph = (hgt * 0.36).clamp(200.0, 300.0);
        let pw = ph * 2.0 / 3.0;
        let grow = 1.37;
        let gap = 28.0;
        let row_mid = screen.top() + hgt * 0.43;
        let target = cur as f32 * (pw + gap) - if cur > 0 { pw + gap } else { 0.0 };
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
        let mut focus_rect = None;
        for (k, &gi) in order.iter().enumerate() {
            let id = self.cfg.custom_games[gi].id.clone();
            let f = ctx.animate_bool_with_time_and_easing(egui::Id::unique(("couch-focus", &id)), k == cur, self.secs(0.28), egui::emath::easing::cubic_out);
            // Tiles after the focused one move along to make room for it.
            let extra = if k > cur { pw * (grow - 1.0) } else { 0.0 };
            let offset = k as f32 * (pw + gap) - scroll + extra;
            let size = egui::vec2(pw, ph) * (1.0 + (grow - 1.0) * f);
            let x0 = if rtl() { start_x - offset - size.x } else { start_x + offset };
            if x0 > screen.right() + pw || x0 + size.x < screen.left() - pw {
                continue;
            }
            let rect = egui::Rect::from_min_size(egui::pos2(x0, row_mid - size.y / 2.0), size);
            let resp = ui.interact(rect, egui::Id::unique(("couch-tile", &id)), egui::Sense::click());
            if resp.clicked() {
                clicked = Some((k, k == cur));
            }
            let alpha = (0.55 + 0.45 * f.clamp(0.0, 1.0)) * enter;
            if f > 0.0 {
                painter.rect_filled(rect.translate(egui::vec2(0.0, 12.0)).expand(4.0 * f), rad(18), black((120.0 * f.min(1.0)) as u8));
            }
            let cover = self.game_poster(ctx, gi);
            let generated = cover.is_none();
            if let Some(t) = cover.or_else(|| self.game_generated_poster(ctx, gi)) {
                painter.add(cover_fill_shape(&t, rect, 16, alpha));
            }
            if generated {
                let fs = 19.0 + 7.0 * f;
                let galley = painter.layout(disp(&self.cfg.custom_games[gi].name), egui::FontId::new(fs, heading_family()), pal().text.gamma_multiply(alpha), rect.width() - 36.0);
                let gx = if rtl() { rect.right() - 18.0 - galley.size().x } else { rect.left() + 18.0 };
                painter.galley(egui::pos2(gx, rect.bottom() - 18.0 - galley.size().y), galley, pal().text);
            }
            if f > 0.0 {
                painter.rect_stroke(rect.expand(6.0 + 2.0), rad(20), egui::Stroke::new(4.0, pal().accent.gamma_multiply(f.clamp(0.0, 1.0))), egui::StrokeKind::Middle);
            }
            if k == cur {
                focus_rect = Some(rect);
            }
            let mut y = rect.top() + 10.0;
            if self.cfg.custom_games[gi].favorite {
                let bx = if rtl() { rect.right() - 40.0 } else { rect.left() + 10.0 };
                let badge = egui::Rect::from_min_size(egui::pos2(bx, y), egui::vec2(30.0, 30.0));
                painter.rect_filled(badge, rad(8), pal().bg);
                glyph(&painter, Glyph::Heart, badge.shrink(7.0), pal().accent);
                y += 36.0;
            }
            if self.running.contains_key(&id) {
                let bx = if rtl() { rect.right() - 90.0 } else { rect.left() + 10.0 };
                poster_badge(&painter, egui::pos2(bx, y), "Running", pal().ok, 1.0);
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

        // The focused game's name and a line about it, under its poster.
        match (focus, focus_rect) {
            (Some(gi), Some(fr)) => {
                // The details panel (Y) shows the store's description.
                if !self.cfg.settings.no_store && !self.store.cache.contains_key(&self.cfg.custom_games[gi].id) {
                    self.store_ensure(ctx, gi);
                }
                let g = self.cfg.custom_games[gi].clone();
                let stat = self.stats.get(&g.id).cloned().unwrap_or_default();
                let mut facts = Vec::new();
                if let Some(m) = self.running.get(&g.id) {
                    facts.push(lfmt("Running · {}", &[&format_duration(unix_now().saturating_sub(m.started))]));
                } else if stat.last_played > 0 {
                    facts.push(lfmt("{} played", &[&format_duration(stat.playtime_secs)]));
                    facts.push(lfmt("last played {}", &[&format_ago(stat.last_played, unix_now())]));
                } else {
                    facts.push(lg("Never played").to_string());
                }
                facts.push(game_source(&g));
                let tx = if rtl() { fr.right() - 8.0 } else { fr.left() + 8.0 };
                let align_top = if rtl() { egui::Align2::RIGHT_TOP } else { egui::Align2::LEFT_TOP };
                let title_w = (if rtl() { tx - screen.left() } else { screen.right() - tx }) - margin;
                let mut job = egui::text::LayoutJob::simple_singleline(disp(&g.name), egui::FontId::new(48.0, heading_family()), pal().text);
                job.wrap = egui::text::TextWrapping { max_width: title_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let title = painter.layout_job(job);
                let title_y = fr.bottom() + 28.0;
                let pos = if rtl() { egui::pos2(tx - title.size().x, title_y) } else { egui::pos2(tx, title_y) };
                painter.galley(pos, title, pal().text);
                painter.text(egui::pos2(tx, title_y + 66.0), align_top, disp(&facts.join(" · ")), egui::FontId::proportional(18.0), pal().subtext);
            }
            _ => {
                painter.text(screen.center(), egui::Align2::CENTER_CENTER, tr("No games here."), egui::FontId::new(28.0, heading_family()), pal().subtext);
            }
        }

        // What the buttons do, in the controller's colours.
        let foot_y = screen.bottom() - 40.0 - 16.0;
        let hints: [(&str, egui::Color32, &str); 3] = [
            ("A", egui::Color32::from_rgb(0x6F, 0xD0, 0x8C), "Play"),
            ("Y", egui::Color32::from_rgb(0xE6, 0xB5, 0x66), "Details"),
            ("B", egui::Color32::from_rgb(0xEF, 0x6F, 0x7B), "Back"),
        ];
        let label_font = egui::FontId::proportional(17.0);
        let mut x = if rtl() { screen.left() + margin } else { screen.right() - margin };
        // From the far end inwards: F11 first, then B, Y, A.
        let exit = painter.layout_no_wrap(tr("Exit couch mode"), label_font.clone(), pal().subtext);
        let key = painter.layout_no_wrap("F11".into(), egui::FontId::proportional(14.0), pal().subtext);
        let mut place = |w: f32| -> f32 {
            let left = if rtl() { x } else { x - w };
            x = if rtl() { x + w + 32.0 } else { x - w - 32.0 };
            left
        };
        let kw = key.size().x + 20.0;
        let left = place(kw + 10.0 + exit.size().x);
        let (key_x, text_x) = if rtl() { (left + exit.size().x + 10.0, left) } else { (left, left + kw + 10.0) };
        let kr = egui::Rect::from_min_size(egui::pos2(key_x, foot_y - 16.0), egui::vec2(kw, 32.0));
        painter.rect_stroke(kr, rad(8), egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);
        painter.galley(kr.center() - key.size() / 2.0, key, pal().subtext);
        painter.galley(egui::pos2(text_x, foot_y - exit.size().y / 2.0), exit, pal().subtext);
        for (letter, color, label) in hints.iter().rev() {
            let text = painter.layout_no_wrap(tr(label), label_font.clone(), pal().subtext);
            let left = place(32.0 + 10.0 + text.size().x);
            let (cx, tx) = if rtl() { (left + text.size().x + 10.0 + 16.0, left) } else { (left + 16.0, left + 42.0) };
            painter.circle_filled(egui::pos2(cx, foot_y), 16.0, *color);
            painter.text(egui::pos2(cx, foot_y), egui::Align2::CENTER_CENTER, *letter, egui::FontId::new(15.0, semibold_family()), pal().sidebar);
            painter.galley(egui::pos2(tx, foot_y - text.size().y / 2.0), text, pal().subtext);
        }
        // The rest, quietly, at the reading start.
        let more = tr("LB/RB shelves · View favourite · Start sort");
        let (mx, malign) = if rtl() { (screen.right() - margin, egui::Align2::RIGHT_CENTER) } else { (screen.left() + margin, egui::Align2::LEFT_CENTER) };
        painter.text(egui::pos2(mx, foot_y), malign, more, egui::FontId::proportional(14.0), pal().faint);
        drop(over);

        if let (Some(gi), true) = (focus, self.couch.as_ref().is_some_and(|c| c.details)) {
            self.couch_details(ui, ctx, gi, screen, acts);
        }
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
        child.label(title_text(disp(&g.name), 40.0));
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
