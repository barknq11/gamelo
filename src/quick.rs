//! Ctrl+K quick search: type part of a game's name, Enter plays it,
//! Shift+Enter opens its page.

use crate::i18n::{disp, fmt_logical as lfmt, logical as lg, rtl, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;

#[derive(Default)]
pub struct Quick {
    pub query: String,
    pub cursor: usize,
    /// Focus the field on the first frame.
    pub fresh: bool,
}

/// How well `name` matches `query` (higher is better); None when it doesn't.
/// Prefixes beat substrings, which beat word initials ("fbk" → First
/// Berserker Khazan), which beat letters merely in order.
pub fn score(name: &str, query: &str) -> Option<i32> {
    let (n, q) = (name.to_lowercase(), query.trim().to_lowercase());
    if q.is_empty() {
        return Some(0);
    }
    if n.starts_with(&q) {
        return Some(1000 - n.len() as i32);
    }
    if let Some(pos) = n.find(&q) {
        return Some(800 - pos as i32);
    }
    let initials: String = n.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next()).collect();
    if initials.contains(&q) {
        return Some(600);
    }
    // Every letter in order; tighter matches score higher.
    let mut gaps = 0;
    let mut it = n.chars();
    for qc in q.chars().filter(|c| !c.is_whitespace()) {
        let mut found = false;
        for c in it.by_ref() {
            if c == qc {
                found = true;
                break;
            }
            gaps += 1;
        }
        if !found {
            return None;
        }
    }
    Some(300 - gaps)
}

impl App {
    /// Matching games, best first (recently played first for an empty query).
    fn quick_results(&self, query: &str) -> Vec<usize> {
        let games = &self.cfg.custom_games;
        let last = |i: usize| self.stats.get(&games[i].id).map_or(0, |s| s.last_played);
        let mut hits: Vec<(usize, i32)> = (0..games.len()).filter_map(|i| score(&games[i].name, query).map(|s| (i, s))).collect();
        hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| last(b.0).cmp(&last(a.0))));
        hits.into_iter().take(8).map(|(i, _)| i).collect()
    }

    pub fn quick_modal(&mut self, ctx: &egui::Context, acts: &mut Acts) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) && self.couch.is_none() {
            self.quick = if self.quick.is_some() { None } else { Some(Quick { fresh: true, ..Default::default() }) };
        }
        let Some(mut q) = self.quick.take() else { return };
        let results = self.quick_results(&q.query);
        q.cursor = q.cursor.min(results.len().saturating_sub(1));
        // Keys the text field would otherwise take.
        let (down, up, play, open, close) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if down && q.cursor + 1 < results.len() {
            q.cursor += 1;
        }
        if up {
            q.cursor = q.cursor.saturating_sub(1);
        }
        let mut pick: Option<(usize, GameAct)> = None;
        let act_for = |app: &App, i: usize| if app.running.contains_key(&app.cfg.custom_games[i].id) { GameAct::Stop } else { GameAct::Play };
        if let Some(&i) = results.get(q.cursor) {
            if play {
                pick = Some((i, act_for(self, i)));
            } else if open {
                pick = Some((i, GameAct::Open));
            }
        }
        let resp = egui::Modal::new(egui::Id::unique("quick-search")).frame(egui::Frame::new().fill(pal().panel).stroke(egui::Stroke::new(1.0, pal().border)).inner_margin(egui::Margin::same(14)).corner_radius(rad(14))).show(ctx, |ui| {
            dir_ui(ui, |ui| {
                ui.set_width(560.0);
                let (r, _) = text_field(ui, &mut q.query, "Find a game…", 560.0);
                if std::mem::take(&mut q.fresh) {
                    r.request_focus();
                }
                if r.changed() {
                    q.cursor = 0;
                }
                ui.add_space(8.0);
                if results.is_empty() {
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new(tr("No games match.")).color(pal().subtext)));
                    ui.add_space(10.0);
                }
                for (k, &i) in results.iter().enumerate() {
                    let w = ui.available_width();
                    let (rect, row) = ui.allocate_exact_size(egui::vec2(w, 54.0), egui::Sense::click());
                    if row.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        q.cursor = k;
                    }
                    let on = k == q.cursor;
                    if on {
                        ui.painter().rect_filled(rect, rad(8), pal().accent.gamma_multiply(0.16));
                    }
                    let x = |off: f32| if rtl() { rect.right() - off } else { rect.left() + off };
                    let thumb = egui::Rect::from_center_size(egui::pos2(x(24.0), rect.center().y), egui::vec2(30.0, 45.0));
                    match self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
                        Some(t) => {
                            ui.painter().add(cover_fill_shape(&t, thumb, 4, 1.0));
                        }
                        None => {
                            ui.painter().rect_filled(thumb, rad(4), pal().card2);
                        }
                    }
                    let g = &self.cfg.custom_games[i];
                    let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
                    let running = self.running.contains_key(&g.id);
                    let sub = if running {
                        lg("Running").to_string()
                    } else {
                        match self.stats.get(&g.id).filter(|s| s.last_played > 0) {
                            Some(s) => lfmt("Last played {}", &[&format_ago(s.last_played, unix_now())]),
                            None => lg("Never played").to_string(),
                        }
                    };
                    let text_end = if rtl() { rect.left() + 150.0 } else { rect.right() - 150.0 };
                    let clip = egui::Rect::from_x_y_ranges(x(50.0).min(text_end)..=x(50.0).max(text_end), rect.y_range());
                    let p = ui.painter().with_clip_rect(clip);
                    p.text(egui::pos2(x(50.0), rect.center().y - 9.0), align, disp(&g.name), egui::FontId::proportional(15.0), pal().text);
                    p.text(egui::pos2(x(50.0), rect.center().y + 11.0), align, disp(&sub), egui::FontId::proportional(12.0), if running { pal().ok } else { pal().subtext });
                    if on {
                        let hint = if running { lg("Enter to stop · Shift+Enter for its page") } else { lg("Enter to play · Shift+Enter for its page") };
                        let end = if rtl() { egui::Align2::LEFT_CENTER } else { egui::Align2::RIGHT_CENTER };
                        let ex = if rtl() { rect.left() + 10.0 } else { rect.right() - 10.0 };
                        ui.painter().text(egui::pos2(ex, rect.center().y), end, disp(hint), egui::FontId::proportional(11.5), pal().faint);
                    }
                    if row.clicked() {
                        pick = Some((i, act_for(self, i)));
                    }
                }
            })
        });
        if resp.should_close() || close {
            return;
        }
        if let Some((i, act)) = pick {
            acts.game.push((self.cfg.custom_games[i].id.clone(), act));
            return;
        }
        self.quick = Some(q);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_search_ranks_sensibly() {
        assert!(score("Hades", "had") > score("Shadow Hades", "had"));
        assert!(score("Shadow Hades", "had") > score("The First Berserker Khazan", "fbk"));
        assert!(score("The First Berserker Khazan", "fbk") > score("Hollow Knight", "hk2").or(Some(0)));
        assert_eq!(score("Celeste", "xyz"), None);
        assert!(score("Celeste", "clst").is_some());
        assert_eq!(score("Anything", "  "), Some(0));
    }
}
