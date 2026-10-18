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

/// Seconds as a short chart label: "3h", "45m".
fn short_hours(secs: u64) -> String {
    match secs {
        0 => "0".into(),
        s if s < 3600 => lfmt("{}m", &[&(s / 60).max(1)]),
        s => lfmt("{}h", &[&((s as f64 / 3600.0).round() as u64)]),
    }
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
    let label_h = if labels { 20.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, height + label_h), egui::Sense::hover());
    let p = ui.painter();
    let max = bars.iter().map(|b| b.secs).max().unwrap_or(0).max(3600);
    let plot = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - label_h));
    // A faint line at the top value and one at the base.
    for (y, value) in [(plot.top(), Some(max)), (plot.bottom(), None)] {
        p.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], egui::Stroke::new(1.0, pal().border.gamma_multiply(0.6)));
        if let Some(v) = value {
            let align = if rtl() { egui::Align2::LEFT_TOP } else { egui::Align2::RIGHT_TOP };
            let x = if rtl() { plot.left() } else { plot.right() };
            p.text(egui::pos2(x, y + 3.0), align, disp(&short_hours(v)), egui::FontId::proportional(11.0), pal().faint);
        }
    }
    let n = bars.len().max(1) as f32;
    let slot = plot.width() / n;
    let hovered = resp.hover_pos().map(|pos| {
        let k = ((pos.x - plot.left()) / slot).floor().clamp(0.0, n - 1.0) as usize;
        if rtl() { bars.len() - 1 - k } else { k }
    });
    for (k, b) in bars.iter().enumerate() {
        let col = if rtl() { bars.len() - 1 - k } else { k } as f32;
        let cx = plot.left() + slot * (col + 0.5);
        let bw = (slot * 0.62).clamp(3.0, 46.0);
        let h = (b.secs as f32 / max as f32) * (plot.height() - 18.0);
        let on = hovered == Some(k);
        if b.secs > 0 {
            let bar = egui::Rect::from_min_max(egui::pos2(cx - bw / 2.0, plot.bottom() - h.max(2.0)), egui::pos2(cx + bw / 2.0, plot.bottom()));
            let color = if on || b.now { pal().accent } else { pal().accent.gamma_multiply(0.55) };
            p.rect_filled(bar, egui::CornerRadius { nw: rad(4).nw, ne: rad(4).ne, sw: 0, se: 0 }, color);
        }
        if labels && !b.label.is_empty() {
            let color = if b.now { pal().text } else { pal().faint };
            p.text(egui::pos2(cx, plot.bottom() + 4.0), egui::Align2::CENTER_TOP, disp(&b.label), egui::FontId::proportional(11.0), color);
        }
    }
    if let Some(k) = hovered {
        let b = &bars[k];
        resp.on_hover_text(disp(&format!("{} · {}", b.tip, if b.secs > 0 { format_duration(b.secs) } else { lg("Not played").to_string() })));
    }
}

/// The last seven days as bars, today at the end (for Home's glance).
pub fn week_bars(ui: &mut egui::Ui, days: &HashMap<i64, u64>, today: i64, height: f32) {
    let bars: Vec<Bar> = (today - 6..=today)
        .map(|d| Bar { label: lg(sessions::DAYS_SHORT[sessions::weekday(d)]).to_string(), tip: sessions::day_label(d), secs: days.get(&d).copied().unwrap_or(0), now: d == today })
        .collect();
    bar_chart(ui, &bars, height, true);
}

impl App {
    pub fn page_stats(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let now = unix_now();
        let today = sessions::day_of(now);
        let log = self.sessions.clone();
        let days = sessions::by_day(&log);
        let since = log.iter().map(|s| s.start).min();
        let sub = match since {
            Some(t) => lfmt("Tracking every session since {}", &[&sessions::day_label(sessions::day_of(t))]),
            None => lg("Charts fill in from your next session — totals below include everything played before.").to_string(),
        };
        ui.label(egui::RichText::new(tr("Stats")).size(24.0).strong().color(pal().text));
        ui.label(egui::RichText::new(disp(&sub)).color(pal().subtext).small());
        ui.add_space(14.0);

        let total: u64 = self.cfg.custom_games.iter().filter_map(|g| self.stats.get(&g.id)).map(|s| s.playtime_secs).sum();
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 34.0;
                let days_text = |n: u32| if n == 1 { lg("1 day").to_string() } else { lfmt("{} days", &[&n]) };
                hltb::stat_block(ui, &tr("Total played"), &disp(&format_duration(total)));
                hltb::stat_block(ui, &tr("This week"), &disp(&format_duration(this_week(&days, today))));
                hltb::stat_block(ui, &tr("Sessions"), &log.len().to_string());
                hltb::stat_block(ui, &tr("Current streak"), &disp(&days_text(sessions::current_streak(&days, today))));
                hltb::stat_block(ui, &tr("Longest streak"), &disp(&days_text(sessions::longest_streak(&days))));
            });
        });
        ui.add_space(12.0);

        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tr("Playtime")).strong().color(pal().text));
                ui.with_layout(far_end(), |ui| {
                    // Added from the far end: reads Week · Month · Year.
                    for (r, label) in [(Range::Year, "Year"), (Range::Month, "Month"), (Range::Week, "Week")] {
                        if choice_chip(ui, self.home.range == r, label).clicked() {
                            self.home.range = r;
                        }
                    }
                });
            });
            ui.add_space(10.0);
            let bars: Vec<Bar> = match self.home.range {
                Range::Week => (today - 6..=today)
                    .map(|d| Bar { label: lg(sessions::DAYS_SHORT[sessions::weekday(d)]).to_string(), tip: sessions::day_label(d), secs: days.get(&d).copied().unwrap_or(0), now: d == today })
                    .collect(),
                Range::Month => (today - 29..=today)
                    .map(|d| {
                        let (_, _, dd) = sessions::civil(d);
                        let label = if (today - d) % 5 == 0 { dd.to_string() } else { String::new() };
                        Bar { label, tip: sessions::day_label(d), secs: days.get(&d).copied().unwrap_or(0), now: d == today }
                    })
                    .collect(),
                Range::Year => {
                    let (y, m, _) = sessions::civil(today);
                    (0..12)
                        .rev()
                        .map(|back| {
                            let (yy, mm) = ((y * 12 + i64::from(m) - 1 - back).div_euclid(12), ((y * 12 + i64::from(m) - 1 - back).rem_euclid(12)) as u32 + 1);
                            let from = sessions::days_from_civil(yy, mm, 1);
                            let to = sessions::next_month(from);
                            let secs = (from..to).map(|d| days.get(&d).copied().unwrap_or(0)).sum();
                            let short = lg(sessions::MONTHS_SHORT[mm as usize - 1]).to_string();
                            Bar { tip: lfmt("{} {}", &[&lg(sessions::MONTHS[mm as usize - 1]), &yy]), label: short, secs, now: back == 0 }
                        })
                        .collect()
                }
            };
            bar_chart(ui, &bars, 170.0, true);
        });
        ui.add_space(12.0);

        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(tr("Every day")).strong().color(pal().text));
            ui.add_space(10.0);
            heatmap(ui, &days, today);
        });
        ui.add_space(12.0);

        self.top_games(ui, ctx, acts);
        ui.add_space(12.0);
        self.recaps(ui, ctx, &log, today, since);
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

/// Every day of the past year as a square, darker for more play.
fn heatmap(ui: &mut egui::Ui, days: &HashMap<i64, u64>, today: i64) {
    let (cell, gap) = (12.0, 3.0);
    let label_w = 30.0;
    let weeks = (((ui.available_width() - label_w) / (cell + gap)).floor() as i64).clamp(8, 53);
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
        s if s < 30 * 60 => Some(0.3),
        s if s < 3600 => Some(0.5),
        s if s < 2 * 3600 => Some(0.7),
        s if s < 4 * 3600 => Some(0.85),
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
            let color = level(secs).map_or(pal().card2, |a| pal().accent.gamma_multiply(a));
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
    let steps = [None, Some(0.3), Some(0.5), Some(0.7), Some(1.0)];
    let span = steps.len() as f32 * (cell + gap);
    let more_w = 34.0;
    let start = if rtl() { end + more_w + span } else { end - more_w - span };
    for (k, a) in steps.iter().enumerate() {
        let x = if rtl() { start - (k as f32 + 1.0) * (cell + gap) } else { start + k as f32 * (cell + gap) };
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, ly), egui::vec2(cell, cell)), rad(3), a.map_or(pal().card2, |a| pal().accent.gamma_multiply(a)));
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
