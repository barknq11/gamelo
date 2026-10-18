//! The library as a compact list: one row per game with its status, playtime,
//! last session and ProtonDB rating. The column headers sort.

use crate::i18n::{disp, rtl, tr};
use crate::theme::{pal, rad};
use crate::*;
use eframe::egui;

const ROW_H: f32 = 58.0;

impl App {
    pub fn library_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, order: &[usize], acts: &mut Acts) {
        let width = ui.available_width();
        // Column starts (from the reading side) and the header's sort for each.
        let cols: [(&str, f32, Option<SortMode>); 5] = [
            ("Name", 56.0, Some(SortMode::Name)),
            ("Status", width * 0.46, None),
            ("Playtime", width * 0.60, Some(SortMode::Playtime)),
            ("Last played", width * 0.72, Some(SortMode::Recent)),
            ("ProtonDB", width * 0.86, None),
        ];
        let x_at = |rect: egui::Rect, off: f32| if rtl() { rect.right() - off } else { rect.left() + off };
        let align = if rtl() { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };

        let (head, _) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::hover());
        for (label, off, sort) in cols {
            let on = sort == Some(self.sort);
            let color = if on { pal().text } else { pal().faint };
            let pos = egui::pos2(x_at(head, off), head.center().y);
            let r = ui.painter().text(pos, align, tr(label), egui::FontId::proportional(12.5), color);
            if let Some(m) = sort {
                let resp = ui.interact(r.expand(4.0), ui.scope_id().with(("list-head", label)), egui::Sense::click());
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.sort = m;
                    self.cfg.settings.library_sort = m.key().into();
                    save_config(&self.cfg);
                }
            }
        }
        ui.add_space(4.0);

        for &i in order {
            let g = self.cfg.custom_games[i].clone();
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let focused = self.lib_keys && self.lib_cursor.as_deref() == Some(g.id.as_str());
            if focused && std::mem::take(&mut self.lib_scroll) {
                ui.scroll_to_rect(rect.expand(8.0), None);
            }
            let h = ctx.animate_bool_with_time(egui::Id::unique(("list-row", &g.id)), resp.hovered() || focused, self.secs(0.12));
            let p = ui.painter();
            if h > 0.0 {
                p.rect_filled(rect, rad(10), pal().card2.gamma_multiply(h));
            }
            if focused {
                p.rect_stroke(rect, rad(10), egui::Stroke::new(1.5, pal().accent), egui::StrokeKind::Inside);
            }
            // A small poster.
            let thumb = egui::Rect::from_center_size(egui::pos2(x_at(rect, 26.0), rect.center().y), egui::vec2(32.0, 48.0));
            if let Some(t) = self.game_poster(ctx, i).or_else(|| self.game_generated_poster(ctx, i)) {
                ui.painter().add(cover_fill_shape(&t, thumb, 4, 1.0));
            }
            let p = ui.painter();
            let running = self.running.contains_key(&g.id);
            let name_color = pal().text.lerp_to_gamma(pal().accent, 0.5 * h);
            let mut name = egui::text::LayoutJob::simple_singleline(disp(&g.name), egui::FontId::proportional(15.0), name_color);
            name.wrap = egui::text::TextWrapping { max_width: width * 0.40 - 56.0, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
            let galley = p.layout_job(name);
            let nx = if rtl() { x_at(rect, cols[0].1) - galley.size().x } else { x_at(rect, cols[0].1) };
            p.galley(egui::pos2(nx, rect.center().y - galley.size().y / 2.0), galley, name_color);
            if g.favorite {
                let sx = x_at(rect, cols[1].1 - 18.0);
                star(p, egui::pos2(sx, rect.center().y), 6.5, true, GOLD);
            }
            let mid = |off: f32| egui::pos2(x_at(rect, off), rect.center().y);
            if g.status != GameStatus::None || running {
                let (label, color) = if running { ("Running", pal().ok) } else { (g.status.label(), g.status.color()) };
                p.text(mid(cols[1].1), align, tr(label), egui::FontId::proportional(13.5), color);
            }
            let stat = self.stats.get(&g.id).cloned().unwrap_or_default();
            let small = egui::FontId::proportional(13.5);
            if stat.playtime_secs > 0 {
                p.text(mid(cols[2].1), align, disp(&format_duration(stat.playtime_secs)), small.clone(), pal().subtext);
            }
            let last = if stat.last_played == 0 { tr("Never played") } else { disp(&format_ago(stat.last_played, unix_now())) };
            p.text(mid(cols[3].1), align, last, small.clone(), if stat.last_played == 0 { pal().faint } else { pal().subtext });
            if let Some((label, color)) = self.store.cache.get(&g.id).and_then(|e| e.info.as_ref()).and_then(|info| crate::store::tier_style(&info.protondb)) {
                p.text(mid(cols[4].1), align, tr(label), small, color);
            }
            // Play / Stop at the far end.
            let c = egui::pos2(if rtl() { rect.left() + 26.0 } else { rect.right() - 26.0 }, rect.center().y);
            let play = egui::Rect::from_center_size(c, egui::vec2(36.0, 36.0));
            let pr = ui.interact(play, egui::Id::unique(("list-play", &g.id)), egui::Sense::click());
            let fill = if running { pal().danger } else { pal().accent };
            let fill = if pr.hovered() { fill } else { fill.gamma_multiply(0.25 + 0.6 * h) };
            let p = ui.painter();
            if pal().square {
                p.rect_filled(play, 0, fill);
            } else {
                p.circle_filled(c, 18.0, fill);
            }
            if running {
                p.rect_filled(egui::Rect::from_center_size(c, egui::vec2(12.0, 12.0)), rad(2), pal().on_accent);
            } else {
                play_triangle(p, c, 9.0, pal().on_accent);
            }
            if pr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                acts.game.push((g.id.clone(), if running { GameAct::Stop } else { GameAct::Play }));
            } else if resp.clicked() {
                acts.game.push((g.id.clone(), GameAct::Open));
            }
            let collections = self.cfg.settings.collections.clone();
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                .context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, running, false, &collections, acts)));
        }
    }
}
