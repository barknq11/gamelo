//! Home, the start page: jump back into the last game, this week at a glance,
//! a suggestion when you can't decide, recent games, and store activity.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr};
use crate::sessions;
use crate::stats_ui::{self, Range};
use crate::stores::Store;
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::sync::mpsc::Receiver;

/// A game Epic gives away (or soon will).
#[derive(Clone)]
pub struct FreeGame {
    pub title: String,
    pub url: String,
    /// Free from/until this day.
    pub day: i64,
    pub upcoming: bool,
}

#[derive(Default)]
pub struct HomeState {
    /// The Stats chart's range.
    pub range: Range,
    /// "Pick something for me": the current suggestion.
    pub pick: Option<String>,
    /// Picks already shown, so "Another one" doesn't repeat itself.
    pub picked: Vec<String>,
    pub free: Option<Vec<FreeGame>>,
    pub free_rx: Option<Receiver<Vec<FreeGame>>>,
    /// Height of the cards beside Home's banner, last frame.
    pub side_h: f32,
    /// Scroll Stats to its recaps (the recap button was pressed).
    pub show_recaps: bool,
}

/// "2026-10-22T15:00:00.000Z" → its UTC day number.
fn iso_day(s: &str) -> Option<i64> {
    let mut it = s.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    Some(sessions::days_from_civil(y, m as u32, d as u32))
}

/// This week's and next week's free games on the Epic store (public feed).
fn fetch_free() -> Vec<FreeGame> {
    let url = "https://store-site-backend-static-ipv4.ak.epicgames.com/freeGamesPromotions?locale=en-US&country=US&allowCountries=US";
    let Ok(body) = http_fetch(url, None) else { return Vec::new() };
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let mut out = Vec::new();
    for e in v["data"]["Catalog"]["searchStore"]["elements"].as_array().into_iter().flatten() {
        let offers = |k: &str| -> Vec<serde_json::Value> {
            e["promotions"][k].as_array().into_iter().flatten().flat_map(|g| g["promotionalOffers"].as_array().cloned().unwrap_or_default()).collect()
        };
        let free = |o: &serde_json::Value| o["discountSetting"]["discountPercentage"].as_i64() == Some(0);
        let (offer, upcoming) = match (offers("promotionalOffers").into_iter().find(free), offers("upcomingPromotionalOffers").into_iter().find(free)) {
            (Some(o), _) => (o, false),
            (None, Some(o)) => (o, true),
            _ => continue,
        };
        let Some(day) = iso_day(offer[if upcoming { "startDate" } else { "endDate" }].as_str().unwrap_or_default()) else { continue };
        let slug = e["catalogNs"]["mappings"].as_array().and_then(|m| m.first()).and_then(|m| m["pageSlug"].as_str()).or_else(|| e["productSlug"].as_str()).unwrap_or_default();
        out.push(FreeGame {
            title: e["title"].as_str().unwrap_or_default().to_string(),
            url: if slug.is_empty() { "https://store.epicgames.com/free-games".into() } else { format!("https://store.epicgames.com/p/{}", slug.trim_end_matches("/home")) },
            day,
            upcoming,
        });
    }
    out.sort_by_key(|f| f.upcoming);
    out
}

impl App {
    pub fn page_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let now = unix_now();
        let today = sessions::day_of(now);
        let days = sessions::by_day(&self.sessions);
        // "Tuesday evening", then a welcome.
        let part = match sessions::hour_of(now) {
            5..=11 => "morning",
            12..=17 => "afternoon",
            18..=22 => "evening",
            _ => "night",
        };
        const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
        let when = lfmt("{} {}", &[&lg(WEEKDAYS[sessions::weekday(today)]), &lg(part)]);
        ui.horizontal(|ui| {
            vcol(ui, |ui| {
                ui.label(egui::RichText::new(disp(&when)).size(14.0).color(pal().faint));
                ui.label(title_text(tr("Welcome back"), 32.0));
            });
            ui.with_layout(far_end(), |ui| {
                if big_button(ui, "Couch mode").clicked() {
                    self.couch_toggle(ctx);
                }
                if big_button(ui, "Add game").clicked() {
                    acts.add = true;
                }
            });
        });
        ui.add_space(24.0);

        if self.cfg.custom_games.is_empty() {
            empty_hint(ui, "Nothing here yet. Add a game, scan a folder, or connect Epic Games or GOG to get started.");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "+  Add game").clicked() {
                    acts.add = true;
                }
                if ui.button(tr("Scan a folder for games…")).clicked() {
                    acts.scan = true;
                }
                if ui.button(tr("Stores")).clicked() {
                    self.page = Page::Stores;
                }
            });
            return;
        }

        // The last game, big; this week and a suggestion beside it.
        let recent = self.recently_played();
        let w = ui.available_width();
        let column = egui::Layout::top_down(if rtl() { egui::Align::Max } else { egui::Align::Min });
        if w > 900.0 {
            let side = (w - 20.0) / 3.0;
            // The banner matches the cards beside it (measured last frame).
            let hero_h = self.home.side_h.max(300.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                let hero_w = w - side - 20.0;
                ui.allocate_ui_with_layout(egui::vec2(hero_w, 0.0), column, |ui| {
                    self.hero_card(ui, ctx, recent.first().copied(), egui::vec2(hero_w, hero_h), acts);
                });
                let r = ui.allocate_ui_with_layout(egui::vec2(side, 0.0), column, |ui| {
                    self.week_card(ui, &days, today);
                    ui.add_space(14.0);
                    self.pick_card(ui, ctx, acts);
                });
                let h = r.response.rect.height();
                if (h - self.home.side_h).abs() > 0.5 {
                    self.home.side_h = h;
                    ctx.request_repaint();
                }
            });
        } else {
            self.hero_card(ui, ctx, recent.first().copied(), egui::vec2(w, 300.0), acts);
            ui.add_space(14.0);
            self.week_card(ui, &days, today);
            ui.add_space(14.0);
            self.pick_card(ui, ctx, acts);
        }
        ui.add_space(28.0);

        if !recent.is_empty() {
            self.poster_row(ui, ctx, "Recently played", &recent, acts);
        }
        let mut added: Vec<usize> = (0..self.cfg.custom_games.len()).filter(|&i| self.cfg.custom_games[i].rom.is_none()).collect();
        // Newest first; games from before "added" was recorded keep their list order.
        added.sort_by_key(|&i| std::cmp::Reverse((self.cfg.custom_games[i].added, i)));
        self.poster_row(ui, ctx, "Recently added", &added, acts);

        self.store_activity(ui, ctx, acts);
    }

    /// "Jump back in": the last game played, as a big card with its banner.
    fn hero_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, game: Option<usize>, size: egui::Vec2, acts: &mut Acts) {
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let p = ui.painter().clone();
        let hover = ctx.animate_bool_with_time(ui.scope_id().with("hero-hover"), resp.hovered(), self.secs(0.16));
        let art = game.and_then(|i| self.game_hero(ctx, i));
        match &art {
            Some(t) => {
                p.add(cover_fill_shape_ex(t, rect, 18, 1.0, 1.0 + 0.03 * hover, 0.35));
            }
            None => {
                // The design's stand-in: a plum card with a ring and a moon.
                p.rect_filled(rect, rad(18), pal().accent.gamma_multiply(0.18).lerp_to_gamma(pal().card, 0.5));
                let clip = p.with_clip_rect(rect.shrink(1.0));
                let c = egui::pos2(rect.right() - 130.0, rect.top() + 110.0);
                clip.circle_stroke(c, 160.0, egui::Stroke::new(56.0, pal().accent.gamma_multiply(0.14)));
                clip.circle_filled(c - egui::vec2(60.0, -30.0), 60.0, pal().accent.gamma_multiply(0.2));
            }
        }
        // Dark from the bottom, so the text holds up over any banner.
        let mut mesh = egui::Mesh::default();
        let (top, bottom) = (egui::Color32::from_black_alpha(20), egui::Color32::from_black_alpha(215));
        for (pos, c) in [(rect.left_top(), top), (rect.right_top(), top), (rect.right_bottom(), bottom), (rect.left_bottom(), bottom)] {
            mesh.colored_vertex(pos, c);
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        p.with_clip_rect(rect.shrink(if pal().square { 0.0 } else { 3.0 })).add(mesh);
        p.rect_stroke(rect, rad(18), egui::Stroke::new(1.0, pal().border.lerp_to_gamma(pal().accent, hover)), egui::StrokeKind::Inside);
        let over = theme::OverImage::new();
        let inner = rect.shrink(32.0);
        let layout = egui::Layout::bottom_up(if rtl() { egui::Align::Max } else { egui::Align::Min });
        let mut clicked_play = false;
        let mut open_page = false;
        {
            let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(layout));
            let Some(i) = game else {
                ui.label(egui::RichText::new(tr("Play something and it shows up here.")).color(pal().subtext));
                ui.label(title_text(tr("Ready when you are"), 36.0));
                drop(over);
                return;
            };
            let g = self.cfg.custom_games[i].clone();
            let stat = self.stats.get(&g.id).cloned().unwrap_or_default();
            ui.horizontal(|ui| {
                let (state, phase) = self.play_state(&g.id);
                let dx = self.shake_dx(&g.id);
                clicked_play = play_button(ui, state, phase, dx).clicked();
                ui.add_space(4.0);
                open_page = dark_button(ui, "Game page").clicked();
            });
            ui.add_space(10.0);
            let meta = format!(
                "{} · {} · {}",
                lfmt("Played {}", &[&format_ago(stat.last_played, unix_now())]),
                lfmt("{} total", &[&format_duration(stat.playtime_secs)]),
                game_source(&g)
            );
            ui.label(egui::RichText::new(disp(&meta)).size(15.0).color(pal().subtext));
            ui.add(egui::Label::new(title_text(disp(&g.name), 44.0)).truncate());
            tag(ui, &tr("Jump back in").to_uppercase(), pal().bg, pal().text);
        }
        drop(over);
        let Some(i) = game else { return };
        let g = self.cfg.custom_games[i].clone();
        if clicked_play {
            let act = if self.running.contains_key(&g.id) { GameAct::Stop } else { GameAct::Play };
            acts.game.push((g.id.clone(), act));
        } else if open_page || resp.clicked() {
            acts.game.push((g.id.clone(), GameAct::Open));
        }
        let running = self.running.contains_key(&g.id);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, running, false, &self.cfg.settings.collections, acts)));
    }

    /// Played games, most recent first.
    fn recently_played(&self) -> Vec<usize> {
        let mut v: Vec<(usize, u64)> = self
            .cfg
            .custom_games
            .iter()
            .enumerate()
            .filter_map(|(i, g)| self.stats.get(&g.id).filter(|s| s.last_played > 0).map(|s| (i, s.last_played)))
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.1));
        v.into_iter().map(|(i, _)| i).collect()
    }

    fn week_card(&mut self, ui: &mut egui::Ui, days: &std::collections::HashMap<i64, u64>, today: i64) {
        let monday = today - sessions::weekday(today) as i64;
        let sessions_this_week = self.sessions.iter().filter(|s| sessions::day_of(s.start) >= monday).count();
        big_card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr("This week")).family(semibold_family()).size(15.0).color(pal().subtext));
                ui.with_layout(far_end(), |ui| {
                    if ui.add(egui::Label::new(egui::RichText::new(tr("All stats")).size(13.0).color(pal().subtext)).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.page = Page::Stats;
                    }
                });
            });
            ui.add_space(4.0);
            let week = stats_ui::this_week(days, today);
            let big = if week > 0 { format_duration(week) } else { lg("Nothing yet").to_string() };
            ui.label(title_text(disp(&big), 34.0));
            ui.add_space(8.0);
            stats_ui::mini_week(ui, days, today);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let n = if sessions_this_week == 1 { lg("1 session").to_string() } else { lfmt("{} sessions", &[&sessions_this_week]) };
                ui.label(egui::RichText::new(disp(&n)).size(13.0).color(pal().subtext));
                if let Some(s) = stats_ui::streak_text(days, today) {
                    ui.add_space(12.0);
                    ui.label(egui::RichText::new(disp(&s)).size(13.0).color(pal().subtext));
                }
            });
        });
    }

    /// Backlog games first, then ones never played; skips what's running.
    fn pick_pool(&self) -> Vec<usize> {
        let games = &self.cfg.custom_games;
        let unplayed = |i: usize| self.stats.get(&games[i].id).is_none_or(|s| s.last_played == 0);
        let mut pool: Vec<usize> = (0..games.len()).filter(|&i| games[i].status == GameStatus::Backlog).collect();
        pool.extend((0..games.len()).filter(|&i| unplayed(i) && games[i].status != GameStatus::Backlog && games[i].status != GameStatus::Finished && games[i].status != GameStatus::Dropped));
        pool.retain(|&i| !self.running.contains_key(&games[i].id));
        pool
    }

    /// A new suggestion, avoiding recent ones while there are others.
    fn reroll(&mut self) {
        let pool = self.pick_pool();
        if pool.is_empty() {
            self.home.pick = None;
            return;
        }
        let ids: Vec<String> = pool.iter().map(|&i| self.cfg.custom_games[i].id.clone()).collect();
        let fresh: Vec<&String> = ids.iter().filter(|id| !self.home.picked.contains(id)).collect();
        let from: Vec<&String> = if fresh.is_empty() {
            self.home.picked.clear();
            ids.iter().collect()
        } else {
            fresh
        };
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize);
        let id = from[seed % from.len()].clone();
        self.home.picked.push(id.clone());
        self.home.pick = Some(id);
    }

    fn pick_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let valid = self.home.pick.as_ref().is_some_and(|id| self.pick_pool().iter().any(|&i| &self.cfg.custom_games[i].id == id));
        if !valid {
            self.reroll();
        }
        let pick = self.home.pick.clone().and_then(|id| self.cfg.custom_games.iter().position(|g| g.id == id));
        big_card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Not sure what to play?")).family(semibold_family()).size(15.0).color(pal().subtext));
            ui.add_space(8.0);
            let Some(i) = pick else {
                para(ui, "You've started everything in your library. Mark games as Backlog to get suggestions here.", pal().subtext, true);
                return;
            };
            let g = self.cfg.custom_games[i].clone();
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(56.0, 76.0), egui::Sense::click());
                match self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
                    Some(t) => {
                        ui.painter().add(cover_fill_shape(&t, rect, 8, 1.0));
                    }
                    None => {
                        ui.painter().rect_filled(rect, rad(8), pal().card2);
                    }
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.game.push((g.id.clone(), GameAct::Open));
                }
                ui.add_space(6.0);
                vcol(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(disp(&g.name)).family(semibold_family()).size(16.0).color(pal().text)).truncate());
                    let why = if g.status == GameStatus::Backlog { lg("From your backlog") } else { lg("Never played") };
                    let sub = match self.hltb.cache.get(&g.id).and_then(|e| e.times.clone()).filter(|t| t.main > 0) {
                        Some(t) => format!("{why} · {}", lfmt("~{} to beat", &[&hltb::hours(t.main)])),
                        None => why.to_string(),
                    };
                    ui.label(egui::RichText::new(disp(&sub)).size(13.0).color(pal().subtext));
                });
            });
            ui.add_space(10.0);
            let half = (ui.available_width() - 8.0) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let play = egui::Button::new(egui::RichText::new(tr("Play it")).family(semibold_family())).fill(pal().card2).stroke(egui::Stroke::NONE).corner_radius(rad(10)).min_size(egui::vec2(half, 40.0));
                if ui.add(play).clicked() {
                    acts.game.push((g.id.clone(), GameAct::Play));
                }
                let another = egui::Button::new(egui::RichText::new(tr("Pick another")).color(pal().subtext)).fill(egui::Color32::TRANSPARENT).corner_radius(rad(10)).min_size(egui::vec2(half, 40.0));
                if ui.add(another).clicked() {
                    self.reroll();
                }
            });
        });
    }

    /// A small poster with its title under it: click opens the game's page,
    /// right-click has the game's menu.
    pub fn small_poster(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, i: usize, size: egui::Vec2, caption: &str, acts: &mut Acts) {
        let (pw, ph) = (size.x, size.y);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(pw, ph + 26.0), egui::Sense::click());
        let g = self.cfg.custom_games[i].clone();
        let h = ctx.animate_bool_with_time(ui.scope_id().with(("small-poster", &g.id)), resp.hovered(), self.secs(0.16));
        let poster = egui::Rect::from_min_size(rect.min - egui::vec2(0.0, 4.0 * h), egui::vec2(pw, ph));
        let painter = ui.painter().clone();
        let cover = self.game_poster(ctx, i);
        let generated = cover.is_none();
        match cover.or_else(|| self.game_generated_poster(ctx, i)) {
            Some(t) => {
                painter.add(cover_fill_shape(&t, poster, 12, 1.0));
            }
            None => {
                painter.rect_filled(poster, rad(12), pal().card2);
            }
        }
        if generated {
            // The design's poster: the name in the bottom corner.
            let over = theme::OverImage::new();
            let galley = painter.layout(disp(&g.name), egui::FontId::new(17.0, heading_family()), pal().text, pw - 24.0);
            let x = if rtl() { poster.right() - 12.0 - galley.size().x } else { poster.left() + 12.0 };
            painter.galley(egui::pos2(x, poster.bottom() - 12.0 - galley.size().y), galley, pal().text);
            drop(over);
        }
        painter.rect_stroke(poster, rad(12), egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h)), egui::StrokeKind::Inside);
        if self.running.contains_key(&g.id) {
            let over = theme::OverImage::new();
            poster_badge(&painter, poster.left_top() + egui::vec2(8.0, 8.0), "Running", pal().ok, 1.0);
            drop(over);
        }
        let text = if caption.is_empty() { disp(&g.name) } else { disp(caption) };
        let color = if caption.is_empty() { pal().text.lerp_to_gamma(pal().accent, 0.5 * h) } else { pal().subtext };
        let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(13.0), color);
        job.wrap = egui::text::TextWrapping { max_width: pw, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let galley = painter.layout_job(job);
        let x = if rtl() { rect.right() - galley.size().x } else { rect.left() };
        painter.galley(egui::pos2(x, rect.top() + ph + 8.0), galley, pal().text);
        let running = self.running.contains_key(&g.id);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(disp(&g.name));
        if resp.clicked() {
            acts.game.push((g.id.clone(), GameAct::Open));
        }
        resp.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, running, false, &self.cfg.settings.collections, acts)));
    }

    /// A titled row of posters spread over the width (one row; "Open
    /// library" for the rest).
    fn poster_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, title: &str, games: &[usize], acts: &mut Acts) {
        let gap = 16.0;
        let w = ui.available_width();
        let cols = (((w + gap) / (150.0 + gap)).floor() as usize).clamp(1, 8);
        let pw = (w - gap * (cols as f32 - 1.0)) / cols as f32;
        ui.horizontal(|ui| {
            ui.label(title_text(tr(title), 20.0));
            ui.with_layout(far_end(), |ui| {
                if ui.add(egui::Label::new(egui::RichText::new(tr("Open library")).size(14.0).color(pal().subtext)).sense(egui::Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.page = Page::Library;
                }
            });
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &i in games.iter().take(cols) {
                let last = self.stats.get(&self.cfg.custom_games[i].id).map_or(0, |s| s.last_played);
                let caption = if title == "Recently played" && last > 0 { format_ago(last, unix_now()) } else { String::new() };
                self.small_poster(ui, ctx, i, egui::vec2(pw, pw * 1.5), &caption, acts);
            }
        });
        ui.add_space(26.0);
    }

    /// Installs in progress, store updates, and Epic's free games.
    fn store_activity(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        if self.home.free.is_none() && self.home.free_rx.is_none() && !self.cfg.settings.no_store {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(fetch_free());
                ctx2.request_repaint();
            });
            self.home.free_rx = Some(rx);
        }
        if let Some(free) = self.home.free_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.home.free = Some(free);
            self.home.free_rx = None;
        }
        let installs: Vec<(String, f32)> = self.tasks.iter().filter(|t| t.kind == TaskKind::Store).map(|t| (t.label.clone(), t.fraction())).collect();
        let updates: Vec<(Store, usize)> = [Store::Epic, Store::Gog].into_iter().map(|s| (s, self.store_update_count(s))).filter(|&(_, n)| n > 0).collect();
        let free = self.home.free.clone().unwrap_or_default();
        if installs.is_empty() && updates.is_empty() && free.is_empty() {
            return;
        }
        egui::Frame::new()
            .inner_margin(egui::Margin::same(22))
            .corner_radius(rad(18))
            .fill(pal().panel)
            .stroke(egui::Stroke::new(1.0, pal().border))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(title_text(tr("New in your stores"), 18.0));
                ui.add_space(8.0);
                let row = |ui: &mut egui::Ui, chip: &str, chip_bg: egui::Color32, text: &str, button: &str| -> bool {
                    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                    ui.painter().rect_filled(line, 0, pal().card2);
                    let mut clicked = false;
                    ui.horizontal(|ui| {
                        ui.set_min_height(52.0);
                        tag(ui, chip, chip_bg, pal().bg);
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(disp(text)).size(15.0).color(pal().text));
                        if !button.is_empty() {
                            ui.with_layout(far_end(), |ui| {
                                clicked = ui.add(egui::Button::new(tr(button)).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(0.0, 40.0))).clicked();
                            });
                        }
                    });
                    clicked
                };
                for (label, f) in &installs {
                    row(ui, &tr("Installing"), pal().accent, &format!("{label} — {}%", (f * 100.0) as u32), "");
                }
                for (store, n) in &updates {
                    let text = if *n == 1 { lfmt("1 update ready on {}", &[&store.name()]) } else { lfmt("{} updates ready on {}", &[n, &store.name()]) };
                    let chip_bg = if *store == Store::Gog { egui::Color32::from_rgb(0xB4, 0x8C, 0xFF) } else { egui::Color32::from_rgb(0xF7, 0xEE, 0xF4) };
                    if row(ui, if *store == Store::Gog { "GOG" } else { "Epic" }, chip_bg, &text, "View") {
                        self.stores.tab = *store;
                        self.stores.show = stores_ui::Show::Updates;
                        self.page = Page::Stores;
                    }
                }
                for f in free.iter().take(4) {
                    let when = if f.upcoming { lfmt("{} — free from {}", &[&f.title, &sessions::day_label(f.day)]) } else { lfmt("{} — free until {}", &[&f.title, &sessions::day_label(f.day)]) };
                    let (chip, bg) = if f.upcoming { (tr("Soon"), pal().subtext) } else { (tr("Free"), pal().ok) };
                    if row(ui, &chip, bg, &when, "Open") {
                        acts.open = Some(f.url.clone());
                    }
                }
            });
    }
}
