//! The Artwork page: browse and pick covers, banners and icons from SteamGridDB.

use crate::*;

impl App {
    pub(crate) fn page_artwork(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let Some(gi) = self.art.game.filter(|&i| i < self.cfg.custom_games.len()) else {
            acts.back = true;
            return;
        };
        let name = self.cfg.custom_games[gi].name.clone();
        acts.back |= back_button(ui, ctx, &name);
        ui.add_space(10.0);
        page_header(
            ui,
            "Artwork",
            &lfmt("Icon, cover and banner for {} — from SteamGridDB or your own files", &[&name]),
        );

        // Current artwork, each with local-file and reset options.
        let icon = self.game_icon(ctx, gi);
        let cover = self.game_poster(ctx, gi);
        let hero = self.game_hero(ctx, gi);
        let (has_icon, has_cover, has_hero) = {
            let g = &self.cfg.custom_games[gi];
            (!g.icon.is_empty(), !g.cover.is_empty(), !g.hero.is_empty())
        };
        let initial = name.chars().next().unwrap_or('?');
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let note = if has_icon { "custom image" } else { "from the game’s exe" };
                art_current(ui, ArtKind::Icon, note, has_icon, acts, |ui| {
                    icon_slot(ui, icon.as_ref(), pal().accent, initial);
                });
                ui.add_space(18.0);
                let note = if has_cover { "shown in the library" } else { "not set" };
                art_current(ui, ArtKind::Cover, note, has_cover, acts, |ui| match &cover {
                    Some(c) => {
                        cover_slot(ui, c, egui::vec2(44.0, 66.0));
                    }
                    None => cover_placeholder(ui, egui::vec2(44.0, 66.0)),
                });
                ui.add_space(18.0);
                let note = if has_hero { "game page banner" } else { "uses the blurred cover" };
                art_current(ui, ArtKind::Hero, note, has_hero, acts, |ui| match &hero {
                    Some(h) => {
                        cover_slot(ui, h, egui::vec2(132.0, 44.0));
                    }
                    None => cover_placeholder(ui, egui::vec2(132.0, 44.0)),
                });
            });
        });
        ui.add_space(14.0);

        if self.sgdb_key().is_none() {
            card_frame().show(ui, |ui| {
                ui.label(
                    egui::RichText::new(tr("SteamGridDB API key needed"))
                        .strong()
                        .color(pal().text),
                );
                para(ui, "Browsing artwork needs a free API key. Paste it here (it’s also in Settings).", pal().subtext, true);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.art.key_input)
                        .password(true)
                        .hint_text(tr("API key"))
                        .desired_width(300.0),
                    );
                    if primary_button(ui, "Save key").clicked() {
                        acts.art.push(ArtCmd::SaveKey);
                    }
                    ui.hyperlink_to(tr("Get a key"),
                        "https://www.steamgriddb.com/profile/preferences/api",
                    );
                });
            });
            return;
        }

        // Search + matching SteamGridDB entries.
        ui.horizontal(|ui| {
            let w = (ui.available_width() - button_width(ui, "Search") - 8.0).max(120.0);
            let (resp, _) = text_field(ui, &mut self.art.query, "Search SteamGridDB…", w);
            let enter = resp.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if primary_button(ui, "Search").clicked() || enter {
                acts.art.push(ArtCmd::Search);
            }
        });
        if !self.art.matches.is_empty() {
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(tr("Match")).color(pal().subtext).small());
                for m in &self.art.matches {
                    let label = match m.year {
                        Some(y) => disp(&format!("{} ({y})", m.name)),
                        None => disp(&m.name),
                    };
                    let on = self.art.sgdb_id == Some(m.id);
                    let tip = if m.verified {
                        "Verified SteamGridDB entry"
                    } else {
                        "Unverified SteamGridDB entry"
                    };
                    if choice_chip(ui, on, &label).on_hover_text(tr(tip)).clicked() {
                        acts.art.push(ArtCmd::SelectMatch(m.id));
                    }
                }
            });
        }
        ui.add_space(10.0);

        // Icons / Covers tabs + apply.
        let kind = self.art.kind;
        ui.horizontal(|ui| {
            for k in [ArtKind::Icon, ArtKind::Cover, ArtKind::Hero] {
                if choice_chip(ui, kind == k, k.label()).clicked() {
                    acts.art.push(ArtCmd::Kind(k));
                }
            }
            if self.art.loading {
                ui.spinner();
            }
            ui.with_layout(
                far_end(),
                |ui| {
                    let can = self.art.picked.is_some() && !self.art.applying;
                    ui.add_enabled_ui(can, |ui| {
                        let label = lfmt("Apply as {}", &[&lg(kind.noun())]);
                        if primary_button(ui, &label).clicked() {
                            acts.art.push(ArtCmd::Apply);
                        }
                    });
                    if self.art.applying {
                        ui.spinner();
                    }
                },
            );
        });
        if let Some(e) = &self.art.error {
            ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
        }
        ui.add_space(10.0);

        // Thumbnail grid.
        let tile = match kind {
            ArtKind::Icon => egui::vec2(104.0, 104.0),
            ArtKind::Cover => egui::vec2(112.0, 168.0),
            ArtKind::Hero => egui::vec2(240.0, 78.0),
        };
        // Thumbnails fade (and settle) in as they arrive instead of popping.
        let (now, fade_secs) = (self.now, self.secs(0.3));
        let fades: Vec<f32> = self
            .art
            .assets
            .iter()
            .map(|a| self.motion.thumb_loaded.get(&a.thumb).map_or(1.0, |&t| ease_since(now, t, fade_secs)))
            .collect();
        self.motion.busy |= fades.iter().any(|&f| f < 1.0);
        let pick_secs = self.secs(0.15);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            for (n, a) in self.art.assets.iter().enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(tile, egui::Sense::click());
                let radius = rad(10);
                ui.painter().rect_filled(rect, radius, pal().card2);
                match self.art.thumbs.get(&a.thumb) {
                    Some(t) => {
                        let inner = match kind {
                            ArtKind::Icon => rect.shrink(12.0),
                            ArtKind::Cover | ArtKind::Hero => rect,
                        };
                        let sz = t.size_vec2();
                        let f = fades[n];
                        let s = (inner.width() / sz.x).min(inner.height() / sz.y) * (0.96 + 0.04 * f);
                        egui::Image::new(egui::load::SizedTexture::from_handle(t))
                            .corner_radius(radius)
                            .tint(egui::Color32::WHITE.gamma_multiply(f))
                            .paint_at(
                                ui,
                                egui::Rect::from_center_size(inner.center(), sz * s),
                            );
                    }
                    None if self.art.thumb_failed.contains(&a.thumb) => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            tr("no preview"),
                            egui::FontId::proportional(11.0),
                            pal().faint,
                        );
                    }
                    None => {
                        ui.put(
                            egui::Rect::from_center_size(
                                rect.center(),
                                egui::vec2(18.0, 18.0),
                            ),
                            egui::Spinner::new(),
                        );
                    }
                }
                let picked = ctx.animate_bool_with_time(
                    egui::Id::unique(("thumb-pick", n)),
                    self.art.picked == Some(n),
                    pick_secs,
                );
                let rest = if resp.hovered() { pal().subtext } else { pal().border };
                let stroke = egui::Stroke::new(1.0 + 1.5 * picked, rest.lerp_to_gamma(pal().accent, picked));
                ui.painter().rect_stroke(
                    rect,
                    radius,
                    stroke,
                    egui::StrokeKind::Inside,
                );
                let dims = if a.width == 0 {
                    "ICO".to_string()
                } else {
                    format!("{}×{}", a.width, a.height)
                };
                let by = if a.author.is_empty() {
                    String::new()
                } else {
                    lfmt(" · by {}", &[&a.author])
                };
                let resp = resp
                    .on_hover_text(disp(&format!("{dims} · {}{by}", a.style)))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.clicked() {
                    acts.art.push(ArtCmd::Pick(n));
                }
                if resp.double_clicked() {
                    acts.art.push(ArtCmd::Pick(n));
                    acts.art.push(ArtCmd::Apply);
                }
                resp.context_menu(|ui| menu_ui(ui, |ui| {
                    if ui.button(trf("Apply as {}", &[&lg(kind.noun())])).clicked() {
                        acts.art.push(ArtCmd::Pick(n));
                        acts.art.push(ArtCmd::Apply);
                        ui.close();
                    }
                    if ui.button(tr("Open full image")).clicked() {
                        acts.open = Some(a.url.clone());
                        ui.close();
                    }
                    if ui.button(tr("Copy image link")).clicked() {
                        acts.copy = Some(a.url.clone());
                        ui.close();
                    }
                }));
            }
        });
        if self.art.more && !self.art.loading {
            ui.add_space(12.0);
            if ui.button(tr("Load more")).clicked() {
                acts.art.push(ArtCmd::More);
            }
        }
        if !self.art.assets.is_empty() {
            ui.add_space(10.0);
            para(ui, "Click to select, then Apply — or double-click a tile to apply it straight away.", pal().subtext, true);
        }
    }
}
