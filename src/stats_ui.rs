//! The Stats page: playtime by week, month and year, a daily heatmap, top
//! games, streaks, and recaps of finished months and years.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr};
use crate::sessions::{self, Session};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    #[default]
    Week,
    Month,
    Year,
}

/// One bar of the chart: its label, its tooltip, and the seconds played.
struct Bar {
    label: String,
    tip: String,
    secs: u64,
    now: bool,
}

/// Seconds played Monday to today.
pub fn this_week(days: &HashMap<i64, u64>, today: i64) -> u64 {
    let monday = today - sessions::weekday(today) as i64;
    (monday..=today).map(|d| days.get(&d).copied().unwrap_or(0)).sum()
}

/// Seconds played in the whole week before this one.
pub fn last_week(days: &HashMap<i64, u64>, today: i64) -> u64 {
    let monday = today - sessions::weekday(today) as i64;
    (monday - 7..monday).map(|d| days.get(&d).copied().unwrap_or(0)).sum()
}

/// A bar chart; the newest bar sits at the far end (right, or left in RTL).
fn bar_chart(ui: &mut egui::Ui, bars: &[Bar], height: f32, labels: bool) {
    let w = ui.available_width();
    let label_h = if labels { 22.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, height + label_h), egui::Sense::hover());
    let p = ui.painter();
    let max = bars.iter().map(|b| b.secs).max().unwrap_or(0).max(3600);
    let plot = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - label_h));
    p.line_segment([plot.left_bottom(), plot.right_bottom()], egui::Stroke::new(1.0, pal().border));
    let n = bars.len().max(1) as f32;
    let slot = plot.width() / n;
    let hovered = resp.hover_pos().map(|pos| {
        let k = ((pos.x - plot.left()) / slot).floor().clamp(0.0, n - 1.0) as usize;
        if rtl() { bars.len() - 1 - k } else { k }
    });
    // Hours over each bar: "12", "2.5", "0.4".
    let value = |secs: u64| {
        let h = secs as f64 / 3600.0;
        if h >= 10.0 { format!("{h:.0}") } else { format!("{h:.1}").trim_end_matches(".0").to_string() }
    };
    for (k, b) in bars.iter().enumerate() {
        let col = if rtl() { bars.len() - 1 - k } else { k } as f32;
        let cx = plot.left() + slot * (col + 0.5);
        let bw = (slot * 0.62).clamp(3.0, 44.0);
        let h = (b.secs as f32 / max as f32) * (plot.height() - 24.0);
        let on = hovered == Some(k);
        if b.secs > 0 {
            let bar = egui::Rect::from_min_max(egui::pos2(cx - bw / 2.0, plot.bottom() - h.max(2.0)), egui::pos2(cx + bw / 2.0, plot.bottom()));
            let color = if b.now || on { pal().accent } else { pal().card2.lerp_to_gamma(pal().accent, 0.3) };
            p.rect_filled(bar, egui::CornerRadius { nw: rad(6).nw, ne: rad(6).ne, sw: 0, se: 0 }, color);
            p.text(egui::pos2(cx, bar.top() - 6.0), egui::Align2::CENTER_BOTTOM, value(b.secs), egui::FontId::proportional(12.0), pal().subtext);
        }
        if labels && !b.label.is_empty() {
            let color = if b.now { pal().text } else { pal().faint };
            p.text(egui::pos2(cx, plot.bottom() + 6.0), egui::Align2::CENTER_TOP, disp(&b.label), egui::FontId::proportional(11.5), color);
        }
    }
    if let Some(k) = hovered {
        let b = &bars[k];
        resp.on_hover_text(disp(&format!("{} · {}", b.tip, if b.secs > 0 { format_duration(b.secs) } else { lg("Not played").to_string() })));
    }
}

/// This week, Monday to Sunday, as the design's small bars: today in the
/// accent, later days as stubs.
pub fn mini_week(ui: &mut egui::Ui, days: &HashMap<i64, u64>, today: i64) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 64.0), egui::Sense::hover());
    let p = ui.painter();
    let monday = today - sessions::weekday(today) as i64;
    let secs: Vec<u64> = (0..7).map(|k| days.get(&(monday + k)).copied().unwrap_or(0)).collect();
    let max = secs.iter().copied().max().unwrap_or(0).max(1);
    let gap = 8.0;
    let bw = (w - gap * 6.0) / 7.0;
    let plot_h = 44.0;
    let mut tip = None;
    for k in 0..7 {
        let col = if rtl() { 6 - k } else { k } as f32;
        let x = rect.left() + col * (bw + gap);
        let h = (secs[k] as f32 / max as f32 * plot_h).max(4.0);
        let bar = egui::Rect::from_min_max(egui::pos2(x, rect.top() + plot_h - h), egui::pos2(x + bw, rect.top() + plot_h));
        let d = monday + k as i64;
        let color = if d == today { pal().accent } else { pal().border };
        p.rect_filled(bar, rad(4), color);
        // One letter a day; Arabic day names all start with "ال", so it has its own.
        const AR: [&str; 7] = ["ن", "ث", "ر", "خ", "ج", "س", "ح"];
        let label = if rtl() { AR[k].to_string() } else { sessions::DAYS_SHORT[k].chars().next().map(String::from).unwrap_or_default() };
        p.text(egui::pos2(bar.center().x, rect.top() + plot_h + 6.0), egui::Align2::CENTER_TOP, disp(&label), egui::FontId::proportional(11.0), pal().faint);
        if resp.hover_pos().is_some_and(|pos| pos.x >= x - gap / 2.0 && pos.x < x + bw + gap / 2.0) {
            tip = Some((d, secs[k]));
        }
    }
    if let Some((d, s)) = tip {
        resp.on_hover_text(disp(&format!("{} · {}", sessions::day_label(d), if s > 0 { format_duration(s) } else { lg("Not played").to_string() })));
    }
}

impl App {
    pub fn page_stats(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let now = unix_now();
        let today = sessions::day_of(now);
        let log = self.sessions.clone();
        let days = sessions::by_day(&log);
        let since = log.iter().map(|s| s.start).min();
        let (ty, tm, _) = sessions::civil(today);
        let this_month = sessions::days_from_civil(ty, tm, 1);

        // Title; the period switch and the latest recap on the far side.
        ui.horizontal(|ui| {
            ui.label(title_text(tr("Stats"), 32.0));
            ui.with_layout(far_end(), |ui| {
                let last_month = this_month - 1;
                let (_, lm, _) = sessions::civil(last_month);
                let recap = lfmt("{} recap", &[&lg(sessions::MONTHS[lm as usize - 1])]);
                if ui.add(egui::Button::new(egui::RichText::new(disp(&recap)).family(semibold_family())).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(0.0, 44.0))).clicked() {
                    self.home.show_recaps = true;
                }
                ui.add_space(6.0);
                // Added from the far end: reads Week · Month · Year.
                let picked = segmented(ui, &[(Range::Week, "Week"), (Range::Month, "Month"), (Range::Year, "Year")], self.home.range);
                if let Some(r) = picked {
                    self.home.range = r;
                }
            });
        });
        let sub = match since {
            Some(t) => lfmt("Tracking every session since {}", &[&sessions::day_label(sessions::day_of(t))]),
            None => lg("Charts fill in from your next session — totals below include everything played before.").to_string(),
        };
        ui.label(egui::RichText::new(disp(&sub)).size(13.0).color(pal().faint));
        ui.add_space(18.0);

        // Four headline numbers.
        let year_start = sessions::days_from_civil(ty, 1, 1);
        let year: Vec<&Session> = log.iter().filter(|s| sessions::day_of(s.start) >= year_start).collect();
        let year_secs: u64 = year.iter().map(|s| s.secs).sum();
        let year_games = year.iter().map(|s| s.id.as_str()).collect::<std::collections::HashSet<_>>().len();
        let days_text = |n: u32| if n == 1 { lg("1 day").to_string() } else { lfmt("{} days", &[&n]) };
        let (delta, _) = week_delta(&days, today);
        let finished = self.cfg.custom_games.iter().filter(|g| g.status == GameStatus::Finished).count();
        let backlog = self.cfg.custom_games.iter().filter(|g| g.status == GameStatus::Backlog).count();
        let total: u64 = self.cfg.custom_games.iter().filter_map(|g| self.stats.get(&g.id)).map(|s| s.playtime_secs).sum();
        let across = |n: usize| if n == 1 { lg("Across 1 game").to_string() } else { lfmt("Across {} games", &[&n]) };
        let kpis = [
            (lg("This week").to_string(), format_duration(this_week(&days, today)), delta),
            (lg("This year").to_string(), format_duration(year_secs), across(year_games)),
            (lg("Current streak").to_string(), days_text(sessions::current_streak(&days, today)), lfmt("Best: {}", &[&days_text(sessions::longest_streak(&days))])),
            (lg("Played in all").to_string(), format_duration(total), lfmt("{} finished · {} in your backlog", &[&finished, &backlog])),
        ];
        let w = ui.available_width();
        let tile_w = (w - 14.0 * 3.0) / 4.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (label, value, sub) in kpis {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 18)).corner_radius(rad(16)).fill(pal().card).stroke(egui::Stroke::new(1.0, pal().border)).show(ui, |ui| {
                    ui.set_width(tile_w - 42.0);
                    vcol(ui, |ui| {
                        ui.label(egui::RichText::new(disp(&label)).size(13.0).color(pal().faint));
                        ui.add(egui::Label::new(title_text(disp(&value), 30.0)).truncate());
                        ui.add(egui::Label::new(egui::RichText::new(disp(&sub)).size(13.0).color(pal().subtext)).truncate());
                    });
                });
            }
        });
        ui.add_space(20.0);

        // Playtime by week, month or year.
        let (title, note, bars): (&str, String, Vec<Bar>) = match self.home.range {
            Range::Week => {
                let monday = today - sessions::weekday(today) as i64;
                let bars = (0..12)
                    .rev()
                    .map(|back| {
                        let from = monday - 7 * back;
                        let secs = (from..from + 7).map(|d| days.get(&d).copied().unwrap_or(0)).sum();
                        Bar { label: String::new(), tip: lfmt("Week of {}", &[&sessions::day_label(from)]), secs, now: back == 0 }
                    })
                    .collect();
                ("Playtime by week", lg("Last 12 weeks · hours").to_string(), bars)
            }
            Range::Month => {
                let bars = (0..12)
                    .rev()
                    .map(|back| {
                        let (yy, mm) = ((ty * 12 + i64::from(tm) - 1 - back).div_euclid(12), ((ty * 12 + i64::from(tm) - 1 - back).rem_euclid(12)) as u32 + 1);
                        let from = sessions::days_from_civil(yy, mm, 1);
                        let secs = (from..sessions::next_month(from)).map(|d| days.get(&d).copied().unwrap_or(0)).sum();
                        Bar { label: lg(sessions::MONTHS_SHORT[mm as usize - 1]).to_string(), tip: lfmt("{} {}", &[&lg(sessions::MONTHS[mm as usize - 1]), &yy]), secs, now: back == 0 }
                    })
                    .collect();
                ("Playtime by month", lg("Last 12 months · hours").to_string(), bars)
            }
            Range::Year => {
                let first = since.map_or(ty, |s| sessions::civil(sessions::day_of(s)).0).min(ty);
                let bars = (first.max(ty - 9)..=ty)
                    .map(|y| {
                        let from = sessions::days_from_civil(y, 1, 1);
                        let secs = (from..sessions::days_from_civil(y + 1, 1, 1)).map(|d| days.get(&d).copied().unwrap_or(0)).sum();
                        Bar { label: y.to_string(), tip: y.to_string(), secs, now: y == ty }
                    })
                    .collect();
                ("Playtime by year", lg("Hours each year").to_string(), bars)
            }
        };
        panel_card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr(title)).family(semibold_family()).size(16.0).color(pal().text));
                ui.with_layout(far_end(), |ui| {
                    ui.label(egui::RichText::new(disp(&note)).size(13.0).color(pal().faint));
                });
            });
            ui.add_space(14.0);
            bar_chart(ui, &bars, 190.0, self.home.range != Range::Week);
        });
        ui.add_space(20.0);

        // Every day, and this month's most played, side by side.
        let w = ui.available_width();
        let column = egui::Layout::top_down(if rtl() { egui::Align::Max } else { egui::Align::Min });
        let month_games = self.month_top(&log, this_month);
        if w > 820.0 {
            let side = (w - 20.0) / 3.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                ui.allocate_ui_with_layout(egui::vec2(w - side - 20.0, 0.0), column, |ui| heatmap_card(ui, &days, today));
                ui.allocate_ui_with_layout(egui::vec2(side, 0.0), column, |ui| self.month_card(ui, &month_games, acts));
            });
        } else {
            heatmap_card(ui, &days, today);
            ui.add_space(14.0);
            self.month_card(ui, &month_games, acts);
        }
        ui.add_space(20.0);

        let r = ui.scope(|ui| self.recaps(ui, ctx, &log, today, since)).response;
        if std::mem::take(&mut self.home.show_recaps) {
            ui.scroll_to_rect(r.rect, Some(egui::Align::TOP));
        }
        ui.add_space(20.0);
        self.top_games(ui, ctx, acts);
    }

    /// This month's games by time played: (index, seconds), most first.
    fn month_top(&self, log: &[Session], month_start: i64) -> Vec<(usize, u64)> {
        let mut secs: HashMap<&str, u64> = HashMap::new();
        for s in log.iter().filter(|s| sessions::day_of(s.start) >= month_start) {
            *secs.entry(s.id.as_str()).or_insert(0) += s.secs;
        }
        let mut v: Vec<(usize, u64)> = secs.into_iter().filter_map(|(id, s)| self.cfg.custom_games.iter().position(|g| g.id == id).map(|i| (i, s))).collect();
        v.sort_by_key(|&(_, s)| std::cmp::Reverse(s));
        v.truncate(5);
        v
    }

    /// "Most played this month": a bar per game.
    fn month_card(&mut self, ui: &mut egui::Ui, games: &[(usize, u64)], acts: &mut Acts) {
        big_card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Most played this month")).family(semibold_family()).size(16.0).color(pal().text));
            ui.add_space(10.0);
            if games.is_empty() {
                para(ui, "Nothing played this month yet.", pal().subtext, true);
                return;
            }
            let max = games[0].1.max(1);
            for (k, &(i, secs)) in games.iter().enumerate() {
                let g = &self.cfg.custom_games[i];
                let resp = ui
                    .horizontal(|ui| {
                        let time = disp(&format_duration(secs));
                        let time_w = ui.painter().layout_no_wrap(time.clone(), egui::FontId::proportional(14.0), pal().subtext).size().x;
                        ui.allocate_ui(egui::vec2(ui.available_width() - time_w - 12.0, 20.0), |ui| {
                            ui.add(egui::Label::new(egui::RichText::new(disp(&g.name)).size(14.0).color(pal().text)).truncate());
                        });
                        ui.with_layout(far_end(), |ui| {
                            ui.label(egui::RichText::new(time).size(14.0).color(pal().subtext));
                        });
                    })
                    .response;
                let (track, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::hover());
                ui.painter().rect_filled(track, rad(4), pal().card2);
                let fw = track.width() * secs as f32 / max as f32;
                let fill = if rtl() { egui::Rect::from_min_max(egui::pos2(track.right() - fw, track.top()), track.right_bottom()) } else { egui::Rect::from_min_size(track.min, egui::vec2(fw, 8.0)) };
                ui.painter().rect_filled(fill, rad(4), if k == 0 { pal().accent } else { pal().card2.lerp_to_gamma(pal().accent, 0.5) });
                ui.add_space(8.0);
                if ui.interact(resp.rect.union(track), ui.scope_id().with(("month-top", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.game.push((g.id.clone(), GameAct::Open));
                }
            }
        });
    }

    /// Most played, all time (older totals included).
    fn top_games(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let mut top: Vec<(usize, u64)> = self
            .cfg
            .custom_games
            .iter()
            .enumerate()
            .filter_map(|(i, g)| self.stats.get(&g.id).map(|s| (i, s.playtime_secs)))
            .filter(|&(_, s)| s > 0)
            .collect();
        top.sort_by_key(|a| std::cmp::Reverse(a.1));
        top.truncate(8);
        section_sub(ui, "Most played", "All time, including playtime from before tracking started", |ui| {
            if top.is_empty() {
                para(ui, "Nothing played yet.", pal().subtext, true);
                return;
            }
            let max = top[0].1.max(1);
            for (rank, &(i, secs)) in top.iter().enumerate() {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 54.0), egui::Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect, rad(8), pal().hover.gamma_multiply(0.6));
                }
                let x = |off: f32| if rtl() { rect.right() - off } else { rect.left() + off };
                let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
                let p = ui.painter().clone();
                p.text(egui::pos2(x(14.0), rect.center().y), egui::Align2::CENTER_CENTER, (rank + 1).to_string(), egui::FontId::proportional(14.0), pal().faint);
                let thumb = egui::Rect::from_center_size(egui::pos2(x(46.0), rect.center().y), egui::vec2(30.0, 45.0));
                match self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
                    Some(t) => {
                        p.add(cover_fill_shape(&t, thumb, 4, 1.0));
                    }
                    None => {
                        p.rect_filled(thumb, rad(4), pal().card2);
                    }
                }
                let name = &self.cfg.custom_games[i].name;
                p.text(egui::pos2(x(74.0), rect.center().y - 9.0), align, disp(name), egui::FontId::proportional(14.5), pal().text);
                let bar_w = (w * 0.45) * secs as f32 / max as f32;
                let bar_x = if rtl() { x(74.0) - bar_w } else { x(74.0) };
                let bar = egui::Rect::from_min_size(egui::pos2(bar_x, rect.center().y + 6.0), egui::vec2(bar_w.max(3.0), 5.0));
                p.rect_filled(bar, rad(3), pal().accent.gamma_multiply(if rank == 0 { 1.0 } else { 0.6 }));
                let end = if rtl() { egui::Align2::LEFT_CENTER } else { egui::Align2::RIGHT_CENTER };
                let ex = if rtl() { rect.left() + 10.0 } else { rect.right() - 10.0 };
                p.text(egui::pos2(ex, rect.center().y), end, disp(&format_duration(secs)), egui::FontId::proportional(14.0), pal().subtext);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.game.push((self.cfg.custom_games[i].id.clone(), GameAct::Open));
                }
            }
        });
    }

    /// A card per finished month (newest first) and one per finished year.
    fn recaps(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, log: &[Session], today: i64, since: Option<u64>) {
        let (ty, tm, _) = sessions::civil(today);
        let this_month = sessions::days_from_civil(ty, tm, 1);
        let mut cards: Vec<(String, sessions::Recap)> = Vec::new();
        if let Some(first) = since.map(sessions::day_of) {
            let (fy, _, _) = sessions::civil(first);
            for y in (fy..ty).rev() {
                if let Some(r) = sessions::recap(log, sessions::days_from_civil(y, 1, 1), sessions::days_from_civil(y + 1, 1, 1)) {
                    cards.push((lfmt("Your {} in games", &[&y]), r));
                }
            }
            let (fy, fm, _) = sessions::civil(first);
            let mut start = sessions::days_from_civil(fy, fm, 1);
            let mut months = Vec::new();
            while start < this_month {
                let end = sessions::next_month(start);
                if let Some(r) = sessions::recap(log, start, end) {
                    let (y, m, _) = sessions::civil(start);
                    months.push((lfmt("{} {}", &[&lg(sessions::MONTHS[m as usize - 1]), &y]), r));
                }
                start = end;
            }
            months.reverse();
            cards.extend(months);
        }
        section_sub(ui, "Recaps", "A look back at each finished month and year", |ui| {
            if cards.is_empty() {
                let first = since.map_or(today, sessions::day_of);
                let (y, m, _) = sessions::civil(sessions::next_month(first));
                para(ui, &lfmt("Your first monthly recap arrives on 1 {} {}.", &[&lg(sessions::MONTHS[m as usize - 1]), &y]), pal().subtext, true);
                return;
            }
            let w = ui.available_width();
            let cols = ((w + 12.0) / 330.0).floor().max(1.0);
            let card_w = (w - 12.0 * (cols - 1.0)) / cols;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                for (title, r) in &cards {
                    self.recap_card(ui, ctx, title, r, card_w);
                }
            });
        });
    }

    fn recap_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, title: &str, r: &sessions::Recap, w: f32) {
        let top = self.cfg.custom_games.iter().position(|g| g.id == r.games[0].0);
        let hero = top.and_then(|i| self.game_hero(ctx, i));
        egui::Frame::new()
            .inner_margin(egui::Margin::same(16))
            .corner_radius(rad(14))
            .fill(pal().card2)
            .stroke(egui::Stroke::new(1.0, pal().border))
            .show(ui, |ui| {
                ui.set_width(w - 34.0);
                ui.with_layout(egui::Layout::top_down(if rtl() { egui::Align::Max } else { egui::Align::Min }), |ui| self.recap_body(ui, title, r, hero.as_ref()));
            });
    }

    fn recap_body(&self, ui: &mut egui::Ui, title: &str, r: &sessions::Recap, hero: Option<&egui::TextureHandle>) {
        if let Some(t) = hero {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 90.0), egui::Sense::hover());
            ui.painter().add(cover_fill_shape(t, rect, 10, 1.0));
            ui.add_space(8.0);
        }
        ui.label(egui::RichText::new(disp(title)).small().strong().color(pal().accent));
        ui.label(egui::RichText::new(disp(&format_duration(r.secs))).size(28.0).strong().color(pal().text));
        let games = if r.games.len() == 1 { lg("1 game").to_string() } else { lfmt("{} games", &[&r.games.len()]) };
        let line = lfmt("{} sessions · {}", &[&r.sessions, &games]);
        ui.label(egui::RichText::new(disp(&line)).small().color(pal().subtext));
        ui.add_space(8.0);
        let name = |id: &str| self.cfg.custom_games.iter().find(|g| g.id == id).map_or_else(|| lg("A removed game").to_string(), |g| g.name.clone());
        let rows = [
            (lg("Most played").to_string(), lfmt("{} · {}", &[&name(&r.games[0].0), &format_duration(r.games[0].1)])),
            (lg("Best day").to_string(), lfmt("{} · {}", &[&sessions::day_label(r.best_day.0), &format_duration(r.best_day.1)])),
            (lg("Longest session").to_string(), lfmt("{} · {}", &[&format_duration(r.longest.secs), &name(&r.longest.id)])),
            (lg("New games").to_string(), r.new_games.to_string()),
        ];
        for (k, v) in rows {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(disp(&k)).small().color(pal().faint));
                ui.with_layout(far_end(), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(disp(&v)).small().color(pal().text)).truncate());
                });
            });
        }
    }
}

/// "Daily activity": the heatmap in its card.
fn heatmap_card(ui: &mut egui::Ui, days: &HashMap<i64, u64>, today: i64) {
    panel_card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(tr("Daily activity")).family(semibold_family()).size(16.0).color(pal().text));
            ui.with_layout(far_end(), |ui| {
                ui.label(egui::RichText::new(tr("Last 26 weeks")).size(13.0).color(pal().faint));
            });
        });
        ui.add_space(12.0);
        heatmap(ui, days, today);
    });
}

/// Every day of the past half year as a square, brighter for more play.
fn heatmap(ui: &mut egui::Ui, days: &HashMap<i64, u64>, today: i64) {
    let (cell, gap) = (14.0, 4.0);
    let label_w = 30.0;
    let weeks = (((ui.available_width() - label_w) / (cell + gap)).floor() as i64).clamp(8, 26);
    let height = 18.0 + 7.0 * (cell + gap) + 26.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    let p = ui.painter();
    let monday = today - sessions::weekday(today) as i64;
    let first = monday - 7 * (weeks - 1);
    let grid_w = weeks as f32 * (cell + gap);
    // Oldest week at the reading start.
    let origin_x = if rtl() { rect.right() - label_w } else { rect.left() + label_w };
    let col_x = |c: i64| if rtl() { origin_x - (c as f32 + 1.0) * (cell + gap) + gap } else { origin_x + c as f32 * (cell + gap) };
    let top = rect.top() + 18.0;
    let level = |secs: u64| match secs {
        0 => None,
        s if s < 3600 => Some(0.3),
        s if s < 3 * 3600 => Some(0.55),
        _ => Some(1.0),
    };
    for (row, name) in [(0usize, "Mon"), (2, "Wed"), (4, "Fri")] {
        let y = top + row as f32 * (cell + gap) + cell / 2.0;
        let (x, align) = if rtl() { (rect.right(), egui::Align2::RIGHT_CENTER) } else { (rect.left(), egui::Align2::LEFT_CENTER) };
        p.text(egui::pos2(x, y), align, tr(name), egui::FontId::proportional(10.5), pal().faint);
    }
    let mut hovered = None;
    let mut last_month = 0;
    for c in 0..weeks {
        let week = first + 7 * c;
        let (_, m, _) = sessions::civil(week);
        if m != last_month {
            if c > 0 || weeks < 9 {
                p.text(egui::pos2(col_x(c) + if rtl() { cell } else { 0.0 }, rect.top()), if rtl() { egui::Align2::RIGHT_TOP } else { egui::Align2::LEFT_TOP }, tr(sessions::MONTHS_SHORT[m as usize - 1]), egui::FontId::proportional(10.5), pal().faint);
            }
            last_month = m;
        }
        for r in 0..7 {
            let d = week + r;
            if d > today {
                continue;
            }
            let sq = egui::Rect::from_min_size(egui::pos2(col_x(c), top + r as f32 * (cell + gap)), egui::vec2(cell, cell));
            let secs = days.get(&d).copied().unwrap_or(0);
            let color = level(secs).map_or(pal().card2, |a| pal().card2.lerp_to_gamma(pal().accent, a));
            p.rect_filled(sq, rad(3), color);
            if d == today {
                p.rect_stroke(sq, rad(3), egui::Stroke::new(1.0, pal().text.gamma_multiply(0.6)), egui::StrokeKind::Outside);
            }
            if resp.hover_pos().is_some_and(|pos| sq.expand(gap / 2.0).contains(pos)) {
                hovered = Some((d, secs));
            }
        }
    }
    // Legend: Less ▢▢▢▢▢ More, under the grid at the far end.
    let ly = top + 7.0 * (cell + gap) + 8.0;
    let end = if rtl() { origin_x - grid_w } else { origin_x + grid_w };
    let steps = [None, Some(0.3), Some(0.55), Some(1.0)];
    let span = steps.len() as f32 * (cell + gap);
    let more_w = 34.0;
    let start = if rtl() { end + more_w + span } else { end - more_w - span };
    for (k, a) in steps.iter().enumerate() {
        let x = if rtl() { start - (k as f32 + 1.0) * (cell + gap) } else { start + k as f32 * (cell + gap) };
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, ly), egui::vec2(cell, cell)), rad(3), a.map_or(pal().card2, |a| pal().card2.lerp_to_gamma(pal().accent, a)));
    }
    let (less_x, less_a, more_x, more_a) = if rtl() {
        (start + 6.0, egui::Align2::LEFT_CENTER, start - span - 4.0, egui::Align2::RIGHT_CENTER)
    } else {
        (start - 6.0, egui::Align2::RIGHT_CENTER, start + span + 4.0, egui::Align2::LEFT_CENTER)
    };
    p.text(egui::pos2(less_x, ly + cell / 2.0), less_a, tr("Less"), egui::FontId::proportional(10.5), pal().faint);
    p.text(egui::pos2(more_x, ly + cell / 2.0), more_a, tr("More"), egui::FontId::proportional(10.5), pal().faint);
    if let Some((d, secs)) = hovered {
        let tip = if secs > 0 { format_duration(secs) } else { lg("Not played").to_string() };
        resp.on_hover_text(disp(&format!("{} · {tip}", sessions::day_label(d))));
    }
}

/// "3 h this week" and how it compares with last week (logical text).
pub fn week_delta(days: &HashMap<i64, u64>, today: i64) -> (String, egui::Color32) {
    let (now, before) = (this_week(days, today), last_week(days, today));
    match now.cmp(&before) {
        _ if before == 0 => (lg("Nothing played last week").to_string(), pal().faint),
        std::cmp::Ordering::Greater => (lfmt("{} more than last week", &[&format_duration(now - before)]), pal().ok),
        std::cmp::Ordering::Less => (lfmt("{} less than last week", &[&format_duration(before - now)]), pal().subtext),
        std::cmp::Ordering::Equal => (lg("Same as last week").to_string(), pal().subtext),
    }
}

/// "3 days" streak text, or None for no streak.
pub fn streak_text(days: &HashMap<i64, u64>, today: i64) -> Option<String> {
    match sessions::current_streak(days, today) {
        0 => None,
        1 => Some(lg("1-day streak").to_string()),
        n => Some(lfmt("{}-day streak", &[&n])),
    }
}
