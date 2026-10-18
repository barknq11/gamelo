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
    pub image: String,
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
        let images = e["keyImages"].as_array().cloned().unwrap_or_default();
        let image = ["OfferImageWide", "Thumbnail", "OfferImageTall"]
            .iter()
            .find_map(|k| images.iter().find(|i| i["type"] == *k).and_then(|i| i["url"].as_str()))
            .unwrap_or_default()
            .to_string();
        let slug = e["catalogNs"]["mappings"].as_array().and_then(|m| m.first()).and_then(|m| m["pageSlug"].as_str()).or_else(|| e["productSlug"].as_str()).unwrap_or_default();
        out.push(FreeGame {
            title: e["title"].as_str().unwrap_or_default().to_string(),
            image,
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
        let greeting = match sessions::hour_of(now) {
            5..=11 => "Good morning",
            12..=17 => "Good afternoon",
            18..=22 => "Good evening",
            _ => "Up late?",
        };
        ui.label(egui::RichText::new(tr(greeting)).size(26.0).strong().color(pal().text));
        let n = self.cfg.custom_games.len();
        let games = if n == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&n]) };
        let week = stats_ui::this_week(&days, today);
        let sub = if week > 0 { format!("{games} · {}", lfmt("{} played this week", &[&format_duration(week)])) } else { games };
        ui.label(egui::RichText::new(disp(&sub)).color(pal().subtext).small());
        ui.add_space(16.0);

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

        self.jump_back_in(ui, ctx, acts);

        // This week and a suggestion, side by side (stacked when narrow).
        let w = ui.available_width();
        if w > 760.0 {
            let half = (w - 14.0) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let column = egui::Layout::top_down(if rtl() { egui::Align::Max } else { egui::Align::Min });
                ui.allocate_ui_with_layout(egui::vec2(half, 0.0), column, |ui| self.week_card(ui, &days, today));
                ui.allocate_ui_with_layout(egui::vec2(half, 0.0), column, |ui| self.pick_card(ui, ctx, acts));
            });
        } else {
            self.week_card(ui, &days, today);
            ui.add_space(12.0);
            self.pick_card(ui, ctx, acts);
        }
        ui.add_space(22.0);

        let hero = self.recently_played().first().copied();
        let recent: Vec<usize> = self.recently_played().into_iter().filter(|&i| Some(i) != hero).collect();
        if !recent.is_empty() {
            self.poster_row(ui, ctx, "Recently played", &recent, acts);
        }
        let mut added: Vec<usize> = (0..self.cfg.custom_games.len()).collect();
        // Newest first; games from before "added" was recorded keep their list order.
        added.sort_by_key(|&i| std::cmp::Reverse((self.cfg.custom_games[i].added, i)));
        self.poster_row(ui, ctx, "Recently added", &added, acts);

        self.store_activity(ui, ctx, acts);
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
        let resp = card_frame()
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tr("This week")).strong().color(pal().text));
                    ui.with_layout(far_end(), |ui| {
                        if let Some(s) = stats_ui::streak_text(days, today) {
                            pill(ui, &s, pal().accent);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new(disp(&format_duration(stats_ui::this_week(days, today)))).size(28.0).strong().color(pal().text));
                let (delta, color) = stats_ui::week_delta(days, today);
                ui.label(egui::RichText::new(disp(&delta)).small().color(color));
                ui.add_space(10.0);
                stats_ui::week_bars(ui, days, today, 70.0);
            })
            .response;
        let resp = ui.interact(resp.rect, ui.scope_id().with("week-card"), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            self.page = Page::Stats;
        }
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
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Not sure what to play?")).strong().color(pal().text));
            ui.add_space(8.0);
            let Some(i) = pick else {
                para(ui, "You've started everything in your library. Mark games as Backlog to get suggestions here.", pal().subtext, true);
                return;
            };
            let g = self.cfg.custom_games[i].clone();
            ui.horizontal_top(|ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(84.0, 126.0), egui::Sense::click());
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
                    ui.add(egui::Label::new(egui::RichText::new(disp(&g.name)).size(18.0).strong().color(pal().text)).truncate());
                    let why = if g.status == GameStatus::Backlog { "In your backlog" } else { "Never played" };
                    ui.label(egui::RichText::new(tr(why)).small().color(pal().subtext));
                    if let Some(t) = self.hltb.cache.get(&g.id).and_then(|e| e.times.clone()).filter(|t| t.main > 0) {
                        ui.label(egui::RichText::new(disp(&lfmt("About {} to beat", &[&hltb::hours(t.main)]))).small().color(pal().subtext));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Play").clicked() {
                            acts.game.push((g.id.clone(), GameAct::Play));
                        }
                        if ui.button(tr("Another one")).clicked() {
                            self.reroll();
                        }
                    });
                });
            });
        });
    }

    /// A titled row of small posters that fit the width.
    fn poster_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, title: &str, games: &[usize], acts: &mut Acts) {
        let (pw, ph, gap) = (124.0, 186.0, 16.0);
        let fit = (((ui.available_width() + gap) / (pw + gap)).floor() as usize).max(1);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr(title)).size(17.0).strong().color(pal().text));
            ui.with_layout(far_end(), |ui| {
                if games.len() > fit && ui.link(tr("See all")).clicked() {
                    self.page = Page::Library;
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &i in games.iter().take(fit) {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(pw, ph + 26.0), egui::Sense::click());
                let g = self.cfg.custom_games[i].clone();
                let h = ctx.animate_bool_with_time(egui::Id::unique(("home-poster", title, &g.id)), resp.hovered(), self.secs(0.16));
                let poster = egui::Rect::from_min_size(rect.min - egui::vec2(0.0, 4.0 * h), egui::vec2(pw, ph));
                let painter = ui.painter().clone();
                match self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
                    Some(t) => {
                        painter.add(cover_fill_shape(&t, poster, 8, 1.0));
                    }
                    None => {
                        painter.rect_filled(poster, rad(8), pal().card2);
                    }
                }
                painter.rect_stroke(poster, rad(8), egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h)), egui::StrokeKind::Inside);
                if self.running.contains_key(&g.id) {
                    let over = theme::OverImage::new();
                    poster_badge(&painter, poster.left_top() + egui::vec2(6.0, 6.0), "Running", pal().ok, 1.0);
                    drop(over);
                }
                let mut job = egui::text::LayoutJob::simple_singleline(disp(&g.name), egui::FontId::proportional(13.0), pal().text.lerp_to_gamma(pal().accent, 0.5 * h));
                job.wrap = egui::text::TextWrapping { max_width: pw, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
                let galley = painter.layout_job(job);
                let x = if rtl() { rect.right() - galley.size().x } else { rect.left() };
                painter.galley(egui::pos2(x, rect.top() + ph + 6.0), galley, pal().text);
                let running = self.running.contains_key(&g.id);
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    acts.game.push((g.id.clone(), GameAct::Open));
                }
                resp.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, running, false, &self.cfg.settings.collections, acts)));
            }
        });
        ui.add_space(22.0);
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
        ui.label(egui::RichText::new(tr("From your stores")).size(17.0).strong().color(pal().text));
        ui.add_space(8.0);
        for (label, f) in &installs {
            card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(disp(label)).color(pal().text));
                ui.add(egui::ProgressBar::new(*f).desired_height(6.0).fill(pal().accent));
            });
            ui.add_space(8.0);
        }
        for (store, n) in &updates {
            card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let text = if *n == 1 { lfmt("1 update ready on {}", &[&store.name()]) } else { lfmt("{} updates ready on {}", &[n, &store.name()]) };
                    ui.label(egui::RichText::new(disp(&text)).color(pal().text));
                    ui.with_layout(far_end(), |ui| {
                        if ui.button(tr("View")).clicked() {
                            self.stores.tab = *store;
                            self.stores.show = stores_ui::Show::Updates;
                            self.page = Page::Stores;
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
        if free.is_empty() {
            return;
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tr("Free on Epic Games")).strong().color(pal().subtext));
        ui.add_space(8.0);
        let (cw, ch, gap) = (240.0, 135.0, 14.0);
        let fit = (((ui.available_width() + gap) / (cw + gap)).floor() as usize).max(1);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, 14.0);
            for f in free.iter().take(fit.max(2) * 2) {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(cw, ch + 44.0), egui::Sense::click());
                let h = ctx.animate_bool_with_time(egui::Id::unique(("free", &f.title)), resp.hovered(), self.secs(0.16));
                let img = egui::Rect::from_min_size(rect.min, egui::vec2(cw, ch));
                match (!f.image.is_empty()).then(|| self.store_image(ctx, &f.image)).flatten() {
                    Some(t) => {
                        ui.painter().add(cover_fill_shape(&t, img, 10, if f.upcoming { 0.6 } else { 1.0 }));
                    }
                    None => {
                        ui.painter().rect_filled(img, rad(10), pal().card2);
                    }
                }
                ui.painter().rect_stroke(img, rad(10), egui::Stroke::new(1.0 + h, pal().border.lerp_to_gamma(pal().accent, h)), egui::StrokeKind::Inside);
                let over = theme::OverImage::new();
                let (badge, color) = if f.upcoming { ("Coming soon", pal().subtext) } else { ("Free now", pal().ok) };
                poster_badge(ui.painter(), img.left_top() + egui::vec2(8.0, 8.0), badge, color, 1.0);
                drop(over);
                let x = if rtl() { rect.right() } else { rect.left() };
                let align = if rtl() { egui::Align2::RIGHT_TOP } else { egui::Align2::LEFT_TOP };
                let p = ui.painter().with_clip_rect(rect);
                p.text(egui::pos2(x, img.bottom() + 6.0), align, disp(&f.title), egui::FontId::proportional(13.5), pal().text);
                let when = if f.upcoming { lfmt("Free from {}", &[&sessions::day_label(f.day)]) } else { lfmt("Free until {}", &[&sessions::day_label(f.day)]) };
                p.text(egui::pos2(x, img.bottom() + 24.0), align, disp(&when), egui::FontId::proportional(12.0), pal().subtext);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tr("Open in your browser to claim it")).clicked() {
                    acts.open = Some(f.url.clone());
                }
            }
        });
    }
}
