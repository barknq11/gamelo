//! A game's page, and the add-game and remove-game dialogs.

use crate::*;

impl App {
    pub(crate) fn page_game(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, acts: &mut Acts) {
        let Some(i) = self.selected_index() else {
            acts.back = true;
            return;
        };
        self.sync_edit(i);
        let g = self.cfg.custom_games[i].clone();
        let id = g.id.clone();
        let run = self.running.get(&id).cloned();
        let stat = self.stats.get(&id).cloned().unwrap_or_default();

        acts.back |= back_button(ui, ctx, "Library");
        ui.add_space(12.0);

        // Hero banner, with the poster, title and actions laid over it.
        let w = ui.available_width();
        let hero_h = (w * 0.32).clamp(250.0, 340.0);
        let (hero, hero_resp) = ui.allocate_exact_size(egui::vec2(w, hero_h), egui::Sense::click());
        let painter = ui.painter().clone();
        let radius = rad(14);
        // Parallax: the banner image drifts slower than the page as it scrolls.
        let drift = (hero.top() - ui.clip_rect().top()) / hero.height();
        let pan = if self.cfg.settings.reduce_motion { 0.5 } else { (0.5 + 0.6 * drift).clamp(0.0, 1.0) };
        // The banner settles from slightly zoomed in when the page opens.
        let settle = self.progress(self.motion.page_since, 0.8);
        let zoom = 1.12 + 0.08 * (1.0 - settle);
        match self.game_hero(ctx, i) {
            Some(t) => {
                painter.add(cover_fill_shape_ex(&t, hero, 14, 1.0, zoom, pan));
            }
            None => {
                painter.rect_filled(hero, radius, pal().card);
            }
        }
        painter.rect_stroke(hero, radius, egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);

        let pad = 24.0;
        let ph = (hero_h - 2.0 * pad).min(220.0);
        let pw = ph * 2.0 / 3.0;
        let px = if rtl() { hero.right() - pad - pw } else { hero.left() + pad };
        let poster = egui::Rect::from_min_size(egui::pos2(px, hero.bottom() - pad - ph), egui::vec2(pw, ph));
        self.motion.hero_poster = Some(poster);
        // Arriving from the grid, the poster is still flying in: draw it once it lands.
        let flying = self.fly(ctx, &id, false, poster);
        if !flying
            && let Some(t) = self
                .game_poster(ctx, i)
                .or_else(|| self.game_generated_poster(ctx, i))
        {
            painter.add(cover_fill_shape(&t, poster, 10, 1.0));
        }
        painter.rect_stroke(
            poster,
            rad(10),
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(45)),
            egui::StrokeKind::Inside,
        );
        let poster_resp = ui
            .interact(poster, ui.scope_id().with("hero-poster"), egui::Sense::click())
            .on_hover_text(tr("Change artwork"))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if poster_resp.clicked() {
            acts.game.push((id.clone(), GameAct::Artwork));
        }
        for r in [&hero_resp, &poster_resp] {
            r.context_menu(|ui| menu_ui(ui, |ui| game_menu(ui, &g, run.is_some(), true, &self.cfg.settings.collections, acts)));
        }

        let (play_state, phase) = self.play_state(&id);
        // Enter (a controller's A) plays or stops, unless something else has the keys.
        if ctx.memory(|m| m.focused().is_none())
            && !overlay_open(ctx)
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
            && matches!(play_state, PlayState::Play | PlayState::Stop)
        {
            let act = if run.is_some() { GameAct::Stop } else { GameAct::Play };
            acts.game.push((id.clone(), act));
        }
        let dx = self.shake_dx(&id);
        let (playtime, added) = self.shown_playtime(&id, stat.playtime_secs);
        let text_area = if rtl() {
            egui::Rect::from_min_max(egui::pos2(hero.left() + pad, hero.top() + pad), egui::pos2(poster.left() - 24.0, hero.bottom() - pad))
        } else {
            egui::Rect::from_min_max(egui::pos2(poster.right() + 24.0, hero.top() + pad), egui::pos2(hero.right() - pad, hero.bottom() - pad))
        };
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(text_area)
                .layout(egui::Layout::bottom_up(if rtl() { egui::Align::Max } else { egui::Align::Min })),
            |ui| {
                let _over = theme::OverImage::new();
                // Bottom-up: the action row goes in first, the title last.
                ui.horizontal(|ui| {
                    match &run {
                        Some(m) => {
                            if play_button(ui, play_state, phase, dx).clicked() {
                                acts.game.push((id.clone(), GameAct::Stop));
                            }
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(trf(
                                    "Running · {}",
                                    &[&format_duration(unix_now().saturating_sub(m.started))],
                                ))
                                .color(pal().ok),
                            );
                        }
                        None => {
                            if play_button(ui, play_state, phase, dx).clicked() {
                                acts.game.push((id.clone(), GameAct::Play));
                            }
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button(tr("Change artwork")).clicked() {
                        acts.game.push((id.clone(), GameAct::Artwork));
                    }
                    ui.menu_button(tr("More"), |ui| menu_ui(ui, |ui| {
                        for (label, act) in [
                            ("Add to app menu", GameAct::Shortcut),
                            ("Copy launch command", GameAct::CopyCommand),
                            ("Open log", GameAct::OpenLog),
                        ] {
                            if ui.button(tr(label)).clicked() {
                                acts.game.push((id.clone(), act));
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui
                            .button(egui::RichText::new(tr("Remove from library…")).color(pal().danger))
                            .clicked()
                        {
                            acts.game.push((id.clone(), GameAct::AskRemove));
                            ui.close();
                        }
                    }));
                    let tip = if g.favorite { "Remove from favorites" } else { "Add to favorites" };
                    if star_button(ui, g.favorite).on_hover_text(tr(tip)).clicked() {
                        acts.game.push((id.clone(), GameAct::Favorite));
                    }
                    let shown = if g.status == GameStatus::None { tr("Set status") } else { tr(g.status.label()) };
                    egui::ComboBox::from_id_salt("game-status")
                        .selected_text(egui::RichText::new(shown).color(g.status.color().lerp_to_gamma(pal().text, 0.3)))
                        .show_ui(ui, |ui| menu_ui(ui, |ui| {
                            for st in std::iter::once(GameStatus::None).chain(GameStatus::ALL) {
                                if ui.selectable_label(g.status == st, tr(st.label())).clicked() {
                                    acts.game.push((id.clone(), GameAct::SetStatus(st)));
                                }
                            }
                        }));
                });
                ui.add_space(12.0);
                let played = if stat.last_played == 0 {
                    tr("Never played")
                } else {
                    trf("{} played · last played {}", &[&format_duration(playtime), &format_ago(stat.last_played, unix_now())])
                };
                ui.horizontal(|ui| {
                    let glow = added.as_ref().map_or(0.0, |(_, a)| *a);
                    ui.label(egui::RichText::new(played).color(pal().subtext.lerp_to_gamma(pal().ok, glow)));
                    if let Some((delta, alpha)) = &added {
                        ui.label(egui::RichText::new(disp(delta)).color(pal().ok.gamma_multiply(*alpha)));
                    }
                });
                let mut kind = match g.kind {
                    GameKind::Windows => trf("Windows · {}", &[&self.proton_label(&g.proton_dir)]),
                    GameKind::Linux => tr("Linux native"),
                };
                if let Some(link) = &g.store {
                    kind = format!("{kind} · {}", link.store.name());
                }
                ui.label(egui::RichText::new(kind).color(pal().subtext).small());
                ui.add(
                    egui::Label::new(egui::RichText::new(disp(&g.name)).size(30.0).strong().color(pal().text))
                        .truncate(),
                );
            },
        );
        ui.add_space(18.0);

        if game_missing_files(&g) {
            let what = if !Path::new(&g.exe).is_file() {
                "The game’s executable can’t be found — fix the path under Advanced."
            } else {
                "The selected Proton build is missing — pick another one below."
            };
            card_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    status_dot(ui, pal().warn, true);
                    ui.label(egui::RichText::new(tr(what)).color(pal().warn));
                });
            });
            ui.add_space(12.0);
        }

        let tab = self.game_tab_bar(ui, g.kind == GameKind::Windows);
        ui.add_space(14.0);
        match tab {
            GameTab::Overview => {
                self.about_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.glance_card(ui, ctx, i, &stat, acts);
                ui.add_space(12.0);
                self.hltb_card(ui, ctx, i, stat.playtime_secs);
                ui.add_space(12.0);
                self.saves_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.last_session_card(ui, i, &stat, acts);
                return;
            }
            GameTab::Mods => {
                self.opti_card(ui, ctx, i, acts);
                ui.add_space(12.0);
                self.components_card(ui, i);
                return;
            }
            GameTab::Settings => {}
        }
        section(ui, "Performance", |ui| {
            let mut f = g.flags;
            if flags_ui(ui, &mut f) {
                acts.game.push((id.clone(), GameAct::SetFlags(f)));
            }
            if g.flags.gamescope {
                ui.add_space(10.0);
                let mut o = g.gamescope_opts.clone();
                if gamescope_ui(ui, &mut o) {
                    acts.game.push((id.clone(), GameAct::SetGamescope(o)));
                }
            }
            ui.add_space(10.0);
            let mut t = g.tuning.clone();
            if tuning_ui(ui, &mut t, g.flags.mangohud, g.kind == GameKind::Windows) {
                acts.game.push((id.clone(), GameAct::SetTuning(t)));
            }
        });
        ui.add_space(12.0);
        section(ui, "Launch", |ui| {
            if edit_row(ui, "Name", &mut self.edit.name, "") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Name)));
            }
            if edit_row(ui, "Arguments", &mut self.edit.args, "e.g. -windowed") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Args)));
            }
            if edit_row(ui, "Working dir", &mut self.edit.workdir, "the exe’s folder") {
                acts.game.push((id.clone(), GameAct::Commit(Field::Workdir)));
            }
            modal_row(ui, "Environment", |ui| {
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.edit.env)
                        .hint_text("DXVK_HUD=fps\nPROTON_LOG=1")
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                if r.lost_focus() {
                    acts.game.push((id.clone(), GameAct::Commit(Field::Env)));
                }
            });
            note(ui, 128.0, "One KEY=value per line. They're added to the launch; WINEDLLOVERRIDES entries are combined with the app's own.");
        });
        ui.add_space(12.0);
        if g.kind == GameKind::Windows {
            let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
            let current = self
                .proton_tools
                .iter()
                .position(|t| t.dir.to_string_lossy() == g.proton_dir);
            let auto_prefix = self
                .cfg
                .settings
                .prefix_base()
                .join(slugify(&g.name))
                .to_string_lossy()
                .to_string();
            let default_id = format!("umu-{}", slugify(&g.name));
            if self.proton.umu_db.is_none() {
                self.load_fix_data(ctx, false);
            }
            let umu_hint = self.umu_match(&g.name);
            let fix = match &self.proton.fixes {
                Some(Ok(f)) => Some(proton::fix_for(f, &game_umu_id(&g))),
                _ => None,
            };
            section(ui, "Proton & prefix", |ui| {
                modal_row(ui, "Proton", |ui| {
                    if tool_labels.is_empty() {
                        pill(ui, "no Proton builds found — see Dependencies", pal().warn);
                    } else {
                        let shown = current.map_or(tr("Choose a Proton build…"), |n| tool_labels[n].clone());
                        egui::ComboBox::from_id_salt("game-proton")
                            .selected_text(shown)
                            .width(260.0)
                            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                for (n, t) in tool_labels.iter().enumerate() {
                                    if ui.selectable_label(current == Some(n), t).clicked() {
                                        acts.game.push((id.clone(), GameAct::SetProton(n)));
                                    }
                                }
                            }));
                    }
                });
                if edit_row(ui, "umu ID", &mut self.edit.umu_id, &default_id) {
                    acts.game.push((id.clone(), GameAct::Commit(Field::UmuId)));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(128.0);
                    let gameid = game_umu_id(&g);
                    match (&umu_hint, &fix) {
                        (Some(e), _) if e.umu_id != gameid => {
                            status_dot(ui, pal().accent, true);
                            ui.label(
                                egui::RichText::new(trf("The umu database lists this game as {}", &[&e.umu_id]))
                                    .color(pal().accent)
                                    .small(),
                            );
                            if ui.small_button(tr("Use it")).clicked() {
                                acts.game.push((id.clone(), GameAct::SetUmuId(e.umu_id.clone())));
                            }
                        }
                        (_, Some(Some(path))) => {
                            status_dot(ui, pal().ok, true);
                            ui.label(egui::RichText::new(tr("A protonfix exists for this ID")).color(pal().ok).small());
                            if ui.small_button(tr("View")).clicked() {
                                acts.open = Some(format!("{}{path}", proton::FIXES_REPO_URL));
                            }
                        }
                        _ => {
                            ui.label(
                                egui::RichText::new(tr(
                                    "A umu-database ID (e.g. umu-1091500) turns on protonfixes for this game.",
                                ))
                                .color(pal().subtext)
                                .small(),
                            );
                        }
                    }
                });
                ui.add_space(6.0);
                if edit_row(ui, "Prefix", &mut self.edit.prefix, &auto_prefix) {
                    acts.game.push((id.clone(), GameAct::Commit(Field::Prefix)));
                }
                ui.horizontal(|ui| {
                    ui.add_space(128.0);
                    for (label, act) in [
                        ("Open prefix folder", GameAct::OpenPrefix),
                        ("Initialize prefix", GameAct::InitPrefix),
                        ("Winecfg", GameAct::Winecfg),
                    ] {
                        if ui.button(tr(label)).clicked() {
                            acts.game.push((id.clone(), act));
                        }
                    }
                });
            });
            ui.add_space(12.0);
        }
        let settings = &self.cfg.settings;
        card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let id_adv = ui.make_persistent_id("game-advanced");
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id_adv, false);
            // egui's collapsing header always puts its arrow on the left: build
            // the row by hand so it starts on the reading side.
            let dir = if rtl() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) };
            let clicked = ui
                .with_layout(dir, |ui| {
                    let arrow = state.show_toggle_button(ui, disclosure_icon);
                    let label = ui.add(
                        egui::Label::new(egui::RichText::new(tr("Advanced")).strong().color(pal().text)).sense(egui::Sense::click()),
                    );
                    !arrow.clicked() && label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                })
                .inner;
            if clicked {
                state.toggle(ui);
            }
            state.show_body_unindented(ui, |ui| {
                    ui.add_space(6.0);
                    modal_row(ui, "Type", |ui| {
                        for (k, label) in [(GameKind::Windows, "Windows (Proton)"), (GameKind::Linux, "Linux native")] {
                            if choice_chip(ui, g.kind == k, label).clicked() && g.kind != k {
                                acts.game.push((id.clone(), GameAct::SetKind(k)));
                            }
                        }
                    });
                    modal_row(ui, "Executable", |ui| {
                        let w = (ui.available_width() - button_width(ui, "Browse…")).max(120.0);
                        let (r, by_menu) = text_field(ui, &mut self.edit.exe, "", w);
                        if r.lost_focus() || by_menu {
                            acts.game.push((id.clone(), GameAct::Commit(Field::Exe)));
                        }
                        if ui.button(tr("Browse…")).clicked() {
                            acts.game.push((id.clone(), GameAct::BrowseExe));
                        }
                    });
                    ui.add_space(4.0);
                    match &launch_command_string(&g, settings) {
                        Ok(cmd) => {
                            command_preview(ui, cmd);
                            ui.add_space(6.0);
                            if ui.button(tr("Copy launch command")).clicked() {
                                acts.copy = Some(cmd.clone());
                            }
                        }
                        Err(e) => {
                            ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
                        }
                    }
                });
        });
    }

    pub(crate) fn proton_label(&self, dir: &str) -> String {
        self.proton_tools
            .iter()
            .find(|t| t.dir.to_string_lossy() == dir)
            .map(|t| t.label.clone())
            .or_else(|| Path::new(dir).file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| lg("no Proton build").into())
    }

    /// The "add game" dialog. Returns the details once the user confirms.
    pub(crate) fn add_modal_ui(&mut self, ctx: &egui::Context) -> Option<AddModal> {
        let mut m = self.add_modal.take()?;
        let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
        let has_key = self.sgdb_key().is_some() && !self.cfg.settings.skip_auto_fetch;
        let (mut submit, mut cancel) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("add-game")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(470.0);
            ui.label(egui::RichText::new(tr("Add game")).size(18.0).strong().color(pal().text));
            ui.add_space(12.0);
            modal_row(ui, "Name", |ui| {
                text_field(ui, &mut m.name, "", f32::INFINITY);
            });
            modal_row(ui, "Executable", |ui| {
                let shown = Path::new(&m.exe)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| m.exe.clone());
                let w = (ui.available_width() - button_width(ui, "Change…")).max(60.0);
                ui.scope(|ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(egui::RichText::new(disp(&shown)).color(pal().subtext)).truncate())
                        .on_hover_text(&m.exe);
                });
                if ui.button(tr("Change…")).clicked()
                    && let Some(p) = pick_executable()
                {
                    m = self.new_add_modal(p);
                }
            });
            modal_row(ui, "Type", |ui| {
                for (k, label) in [(GameKind::Windows, "Windows (Proton)"), (GameKind::Linux, "Linux native")] {
                    if choice_chip(ui, m.kind == k, label).clicked() {
                        m.kind = k;
                    }
                }
            });
            if m.kind == GameKind::Windows {
                modal_row(ui, "Proton", |ui| {
                    if tool_labels.is_empty() {
                        pill(ui, "no Proton builds found — see Dependencies", pal().warn);
                    } else {
                        let idx = m.proton_idx.min(tool_labels.len() - 1);
                        egui::ComboBox::from_id_salt("add-proton")
                            .selected_text(tool_labels[idx].as_str())
                            .width(260.0)
                            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                                for (n, t) in tool_labels.iter().enumerate() {
                                    ui.selectable_value(&mut m.proton_idx, n, t);
                                }
                            }));
                    }
                });
            }
            para(
                ui,
                if has_key {
                    "Its cover and banner are fetched from SteamGridDB automatically."
                } else {
                    "Tip: with a SteamGridDB key (Settings) its artwork is fetched automatically."
                },
                pal().subtext,
                true,
            );
            ui.add_space(14.0);
            let can_add = !m.name.trim().is_empty()
                && (m.kind == GameKind::Linux || !tool_labels.is_empty());
            ui.horizontal(|ui| {
                ui.add_enabled_ui(can_add, |ui| {
                    if primary_button(ui, "Add game").clicked() {
                        submit = true;
                    }
                });
                if ui.button(tr("Cancel")).clicked() {
                    cancel = true;
                }
            });
        }));
        if resp.should_close() {
            cancel = true;
        }
        if submit {
            return Some(m);
        }
        if !cancel {
            self.add_modal = Some(m);
        }
        None
    }

    pub(crate) fn remove_modal_ui(&mut self, ctx: &egui::Context, acts: &mut Acts) {
        let Some(id) = self.confirm_remove.clone() else {
            return;
        };
        let Some(i) = self.index_of(&id) else {
            self.confirm_remove = None;
            return;
        };
        let g = &self.cfg.custom_games[i];
        let name = g.name.clone();
        let keeps = match g.kind {
            GameKind::Windows => lfmt(
                "It leaves your library. Its prefix and saves stay on disk:\n{}",
                &[&game_prefix(g, &self.cfg.settings).display()],
            ),
            GameKind::Linux => lg("It leaves your library. The game’s files stay on disk.").to_string(),
        };
        let (mut yes, mut no) = (false, false);
        let resp = egui::Modal::new(egui::Id::unique("confirm-remove")).show(ctx, |ui| dir_ui(ui, |ui| {
            ui.set_width(440.0);
            ui.label(
                egui::RichText::new(trf("Remove “{}”?", &[&name]))
                    .size(18.0)
                    .strong()
                    .color(pal().text),
            );
            ui.add_space(8.0);
            para(ui, &keeps, pal().subtext, false);
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(tr("Remove")).color(pal().on_accent).strong())
                            .fill(pal().danger)
                            .stroke(egui::Stroke::new(1.0, pal().danger))
                            .corner_radius(rad(9)),
                    )
                    .clicked()
                {
                    yes = true;
                }
                if ui.button(tr("Cancel")).clicked() {
                    no = true;
                }
            });
        }));
        if yes {
            acts.game.push((id, GameAct::Remove));
            self.confirm_remove = None;
        } else if no || resp.should_close() {
            self.confirm_remove = None;
        }
    }
}
