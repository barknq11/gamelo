//! The Settings and Dependencies pages.

use crate::*;

impl App {
    pub(crate) fn page_settings(&mut self, ui: &mut egui::Ui, acts: &mut Acts) {
        page_header(ui, "Settings", "Changes are saved as you make them");
        ui.set_max_width(820.0);
        let mut save = false;
        let indent = 128.0;

        // ---- Appearance
        let zoom = self.cfg.settings.zoom();
        let dark_now = pal().dark;
        section_sub(ui, "Appearance", "Language, colours, shape, size and motion", |ui| {
            modal_row(ui, "Language", |ui| {
                for (key, label) in [("", tr("System")), ("en", "English".to_string()), ("ar", disp("العربية"))] {
                    if choice_chip(ui, self.cfg.settings.language == key, &label).clicked() {
                        self.cfg.settings.language = key.into();
                        save = true;
                    }
                }
            });
            modal_row(ui, "Mode", |ui| {
                for (key, label) in [("dark", tr("Dark")), ("light", tr("Light")), ("system", tr("Match system"))] {
                    let on = match self.cfg.settings.theme_mode.as_str() {
                        "" => key == "dark",
                        m => m == key,
                    };
                    if choice_chip(ui, on, &label).clicked() {
                        self.cfg.settings.theme_mode = key.into();
                        save = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(120.0, 22.0),
                    egui::Label::new(egui::RichText::new(tr("Theme")).color(pal().subtext)),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                    let current = theme::theme_by_key(
                        if dark_now { &self.cfg.settings.dark_theme } else { &self.cfg.settings.light_theme },
                        dark_now,
                    )
                    .key;
                    for t in theme::THEMES.iter().filter(|t| t.dark == dark_now) {
                        if theme_tile(ui, t, t.key == current).clicked() {
                            let slot = if dark_now {
                                &mut self.cfg.settings.dark_theme
                            } else {
                                &mut self.cfg.settings.light_theme
                            };
                            *slot = t.key.into();
                            save = true;
                        }
                    }
                });
            });
            note(
                ui,
                indent,
                if dark_now {
                    "Dark themes. Switch the mode to Light to pick a light one; each mode remembers its own."
                } else {
                    "Light themes. Switch the mode to Dark to pick a dark one; each mode remembers its own."
                },
            );
            ui.add_space(4.0);
            modal_row(ui, "Accent", |ui| {
                let theme_accent = theme::build(
                    if dark_now { &self.cfg.settings.dark_theme } else { &self.cfg.settings.light_theme },
                    dark_now,
                    "",
                    false,
                )
                .accent;
                if accent_swatch(ui, theme_accent, self.cfg.settings.accent.is_empty())
                    .on_hover_text(tr("The theme's own"))
                    .clicked()
                {
                    self.cfg.settings.accent.clear();
                    save = true;
                }
                ui.add_space(6.0);
                for (key, name, _, _) in theme::ACCENTS {
                    let c = theme::accent_color(key, dark_now).unwrap_or(theme_accent);
                    if accent_swatch(ui, c, self.cfg.settings.accent == *key)
                        .on_hover_text(tr(name))
                        .clicked()
                    {
                        self.cfg.settings.accent = (*key).into();
                        save = true;
                    }
                }
            });
            modal_row(ui, "Corners", |ui| {
                for (square, label) in [(false, tr("Rounded")), (true, tr("Square"))] {
                    if choice_chip(ui, self.cfg.settings.square_corners == square, &label).clicked() {
                        self.cfg.settings.square_corners = square;
                        save = true;
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "Library tiles", |ui| {
                for (key, label, _) in TILE_SIZES {
                    if choice_chip(ui, self.cfg.settings.tile_size_key() == key, label).clicked() {
                        self.cfg.settings.tile_size = key.into();
                        save = true;
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "UI scale", |ui| {
                // Steps, not a slider: dragging a slider that rescales the UI under
                // the pointer makes the value jump around.
                let smaller = ZOOM_STEPS.iter().rev().find(|&&z| z < zoom - 0.005).copied();
                let larger = ZOOM_STEPS.iter().find(|&&z| z > zoom + 0.005).copied();
                let step = |ui: &mut egui::Ui, label: &str, to: Option<f32>, tip: &str| {
                    ui.add_enabled(to.is_some(), egui::Button::new(label).min_size(egui::vec2(32.0, 28.0)))
                        .on_hover_text(tip)
                        .clicked()
                        .then_some(to)
                        .flatten()
                };
                let mut pick = step(ui, "−", smaller, &tr("Smaller (Ctrl −)"));
                ui.add_sized(
                    [64.0, 28.0],
                    egui::Label::new(
                        egui::RichText::new(format!("{:.0}%", zoom * 100.0))
                            .size(16.0)
                            .strong()
                            .color(pal().text),
                    ),
                );
                pick = pick.or(step(ui, "+", larger, &tr("Larger (Ctrl +)")));
                if (zoom - 1.0).abs() > 0.005 {
                    ui.add_space(6.0);
                    if ui.button(tr("Reset")).on_hover_text(tr("Back to 100% (Ctrl 0)")).clicked() {
                        pick = Some(1.0);
                    }
                }
                if let Some(z) = pick {
                    self.cfg.settings.zoom = z;
                    save = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.spacing_mut().item_spacing.x = 6.0;
                for z in ZOOM_STEPS {
                    let label = format!("{:.0}%", z * 100.0);
                    if choice_chip(ui, (z - zoom).abs() < 0.005, &label).clicked() {
                        self.cfg.settings.zoom = z;
                        save = true;
                    }
                }
            });
            note(ui, indent, "Applies on click. Ctrl + / Ctrl − / Ctrl 0 and Ctrl + scroll work anywhere and are remembered too.");
            ui.add_space(8.0);
            modal_row(ui, "VR", |ui| {
                if check(ui, &mut self.cfg.settings.allow_openxr, tr("Let games use VR headsets (OpenXR)")).changed() {
                    save = true;
                }
            });
            note(ui, indent, "Leave off unless you play VR games: on some systems Proton's VR support crashes games as they start.");
            ui.add_space(8.0);
            modal_row(ui, "Controller", |ui| {
                let mut on = !self.cfg.settings.no_gamepad;
                if check(ui, &mut on, tr("Navigate with a game controller")).changed() {
                    self.cfg.settings.no_gamepad = !on;
                    save = true;
                }
            });
            let names = self.pad.as_ref().map(|p| p.connected.lock().unwrap_or_else(|e| e.into_inner()).clone()).unwrap_or_default();
            let line = match names.first() {
                _ if self.cfg.settings.no_gamepad => lg("Off.").to_string(),
                Some(n) => lfmt("Connected: {}. D-pad or left stick moves, A opens or plays, B goes back, LB/RB switch tabs, right stick scrolls.", &[n]),
                None => lg("No controller connected. Plug one in any time.").to_string(),
            };
            note(ui, indent, &line);
            ui.add_space(8.0);
            modal_row(ui, "Motion", |ui| {
                save |= check(ui, &mut self.cfg.settings.reduce_motion, tr("Reduce motion"))
                    .changed();
            });
            note(ui, indent, "Turns off transitions, glides and pulses.");
        });
        ui.add_space(12.0);

        // ---- Artwork
        let key_set = self.sgdb_key().is_some();
        let missing_art = self.needs_art().len();
        section_sub(ui, "Artwork", "Covers, banners and icons from SteamGridDB", |ui| {
            let (mut toggle_show, mut key_edited) = (false, false);
            let show_key = self.show_key;
            let can_test = key_set && !self.key_checking;
            row_with_buttons(
                ui,
                "API key",
                |ui| {
                    if ui.add_enabled(can_test, egui::Button::new(tr("Test"))).clicked() {
                        acts.test_key = true;
                    }
                    toggle_show = ui.button(tr(if show_key { "Hide" } else { "Show" })).clicked();
                },
                |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.cfg.settings.steamgriddb_key)
                            .password(!show_key)
                            .hint_text(tr("paste your key"))
                            .desired_width(ui.available_width()),
                    );
                    key_edited = r.changed();
                    save |= r.lost_focus();
                },
            );
            if toggle_show {
                self.show_key = !self.show_key;
            }
            if key_edited {
                self.key_check = None;
            }
            ui.horizontal(|ui| {
                ui.add_space(indent);
                match (&self.key_check, self.key_checking) {
                    (_, true) => {
                        ui.spinner();
                        ui.label(egui::RichText::new(tr("Checking the key…")).color(pal().subtext).small());
                    }
                    (Some(Ok(())), _) => {
                        status_dot(ui, pal().ok, true);
                        ui.label(egui::RichText::new(tr("The key works")).color(pal().ok).small());
                    }
                    (Some(Err(e)), _) => {
                        status_dot(ui, pal().warn, true);
                        ui.label(egui::RichText::new(tr(e)).color(pal().warn).small());
                    }
                    (None, _) => {
                        ui.label(egui::RichText::new(tr("Free with a SteamGridDB account:")).color(pal().subtext).small());
                        ui.hyperlink_to(
                            egui::RichText::new(tr("get a key")).small(),
                            "https://www.steamgriddb.com/profile/preferences/api",
                        );
                    }
                }
            });
            ui.add_space(8.0);
            modal_row(ui, "New games", |ui| {
                let mut on = !self.cfg.settings.skip_auto_fetch;
                if ui.add_enabled_ui(key_set, |ui| check(ui, &mut on, tr("Fetch a cover and banner when a game is added"))).inner
                    .changed()
                {
                    self.cfg.settings.skip_auto_fetch = !on;
                    save = true;
                }
            });
            modal_row(ui, "Game info", |ui| {
                let mut on = !self.cfg.settings.no_hltb;
                if check(ui, &mut on, tr("Show HowLongToBeat times on game pages")).changed() {
                    self.cfg.settings.no_hltb = !on;
                    save = true;
                }
            });
            modal_row(ui, "", |ui| {
                let mut on = !self.cfg.settings.no_store;
                if check(ui, &mut on, tr("Show Steam store info: description, screenshots, ProtonDB rating")).changed() {
                    self.cfg.settings.no_store = !on;
                    save = true;
                }
            });
            if key_set && missing_art > 0 {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let label = if missing_art == 1 {
                        tr("Fill missing artwork (1 game)")
                    } else {
                        trf("Fill missing artwork ({} games)", &[&missing_art])
                    };
                    if ui.add_enabled(self.auto_pending.is_empty(), egui::Button::new(label)).clicked() {
                        acts.fill_art = true;
                    }
                });
            }
        });
        ui.add_space(12.0);

        // ---- Saves
        let db = self.saves.index.as_ref().map(|ix| ix.len());
        let db_age = saves::index_age();
        let db_busy = self.tasks.iter().any(|t| t.key == "save-db");
        let mut update_db = false;
        section_sub(ui, "Saves", "Backups after every session, and a folder to keep saves in sync", |ui| {
            let (mut browse, mut open) = (false, false);
            let has_sync = !self.cfg.settings.save_sync_dir.is_empty();
            row_with_buttons(
                ui,
                "Sync folder",
                |ui| {
                    open = ui.add_enabled(has_sync, egui::Button::new(tr("Open"))).clicked();
                    browse = ui.button(tr("Browse…")).clicked();
                },
                |ui| {
                    let w = ui.available_width();
                    let (r, by_menu) = text_field(ui, &mut self.cfg.settings.save_sync_dir, &tr("off"), w);
                    save |= r.lost_focus() || by_menu;
                },
            );
            if browse && let Some(p) = pick_folder(lg("Folder to keep game saves in sync")) {
                self.cfg.settings.save_sync_dir = p.to_string_lossy().into();
                save = true;
            }
            if open {
                acts.open = Some(self.cfg.settings.save_sync_dir.clone());
            }
            note(
                ui,
                indent,
                "Use a Syncthing, Nextcloud or Dropbox folder, a USB drive, or a partition Windows can see. Saves are pulled in before a game starts and copied out after it closes.",
            );
            ui.add_space(6.0);
            modal_row(ui, "Backups", |ui| {
                let mut on = self.cfg.settings.save_auto_backup();
                if check(ui, &mut on, tr("Back up saves after every session")).changed() {
                    self.cfg.settings.save_no_auto_backup = !on;
                    save = true;
                }
            });
            modal_row(ui, "Keep", |ui| {
                let keep = self.cfg.settings.save_keep();
                for n in [5usize, 10, 20, 50] {
                    if choice_chip(ui, keep == n, &lfmt("{} per game", &[&n])).clicked() {
                        self.cfg.settings.save_keep = n as u32;
                        save = true;
                    }
                }
            });
            row_with_buttons(
                ui,
                "Save database",
                |ui| {
                    update_db = ui.add_enabled(!db_busy, egui::Button::new(tr("Update"))).clicked();
                },
                |ui| {
                    let text = match (db, db_age) {
                        _ if db_busy => tr("Downloading…"),
                        (Some(n), Some(t)) => trf("{} games · updated {}", &[&n, &format_ago(t, unix_now())]),
                        (None, Some(t)) => trf("downloaded {}", &[&format_ago(t, unix_now())]),
                        _ => tr("not downloaded yet — it's fetched the first time you open a Save Vault"),
                    };
                    ui.label(egui::RichText::new(text).color(pal().subtext).small());
                },
            );
            note(ui, indent, "From the Ludusavi project: where thousands of games keep their saves.");
        });
        if update_db {
            self.update_save_db();
        }
        ui.add_space(12.0);

        // ---- Tools
        section_sub(ui, "Tools", "Leave a field empty to find the tool on your PATH", |ui| {
            let s = &mut self.cfg.settings;
            for (label, field, hint, dep) in [
                ("umu-run", &mut s.umu_bin, "umu-run", "umu-run"),
                ("MangoHud", &mut s.mangohud_bin, "mangohud", "MangoHud"),
                ("GameMode", &mut s.gamemoderun_bin, "gamemoderun", "GameMode"),
            ] {
                let mut browse = false;
                row_with_buttons(
                    ui,
                    label,
                    |ui| browse = ui.button(tr("Browse…")).clicked(),
                    |ui| {
                        let (r, by_menu) = text_field(ui, field, hint, ui.available_width());
                        save |= r.lost_focus() || by_menu;
                    },
                );
                if browse && let Some(p) = pick_any_file(&lfmt("Select {}", &[&label])) {
                    *field = p.to_string_lossy().into();
                    save = true;
                }
                // What will actually be used, so a typo shows up straight away.
                let row = self.deps.iter().find(|r| r.name == dep);
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    match row {
                        Some(r) if r.found => {
                            status_dot(ui, pal().ok, true);
                            ui.label(egui::RichText::new(&r.detail).color(pal().subtext).small().monospace());
                        }
                        Some(r) => {
                            status_dot(ui, pal().warn, true);
                            ui.label(egui::RichText::new(tr(&r.detail)).color(pal().warn).small());
                        }
                        None => {}
                    }
                });
                ui.add_space(4.0);
            }
        });
        ui.add_space(12.0);

        // ---- New games
        let tool_labels: Vec<String> = self.proton_tools.iter().map(|t| t.label.clone()).collect();
        let current_proton = self
            .proton_tools
            .iter()
            .position(|t| t.dir.to_string_lossy() == self.cfg.settings.default_proton);
        section_sub(ui, "New games", "What a game starts with when you add it", |ui| {
            modal_row(ui, "Proton", |ui| {
                let shown = current_proton.map_or(tr("Same as the last game added"), |n| tool_labels[n].clone());
                egui::ComboBox::from_id_salt("default-proton")
                    .selected_text(shown)
                    .width(280.0)
                    .show_ui(ui, |ui| menu_ui(ui, |ui| {
                        if ui
                            .selectable_label(current_proton.is_none(), tr("Same as the last game added"))
                            .clicked()
                        {
                            self.cfg.settings.default_proton.clear();
                            save = true;
                        }
                        for (n, t) in tool_labels.iter().enumerate() {
                            if ui.selectable_label(current_proton == Some(n), t).clicked() {
                                self.cfg.settings.default_proton = self.proton_tools[n].dir.to_string_lossy().into();
                                save = true;
                            }
                        }
                    }));
            });
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(120.0, 22.0),
                    egui::Label::new(egui::RichText::new(tr("Performance")).color(pal().subtext)),
                );
                vcol(ui, |ui| {
                    let mut f = self.cfg.settings.new_game_flags();
                    if flags_ui(ui, &mut f) {
                        self.cfg.settings.new_flags = Some(f);
                        save = true;
                    }
                });
            });
        });
        ui.add_space(12.0);

        // ---- Keyboard shortcuts
        section_sub(ui, "Keyboard shortcuts", "Work anywhere in Gamelo; Meta+G works from anywhere on your desktop", |ui| {
            for (keys, what) in [
                ("Ctrl+K", "Find a game and play it"),
                ("Ctrl+F", "Search the library"),
                ("F11", "Couch mode (a controller's Guide button too)"),
                ("Meta+G", "Show Gamelo — change it in your desktop's shortcut settings"),
            ] {
                modal_row(ui, keys, |ui| {
                    ui.label(egui::RichText::new(tr(what)).color(pal().subtext));
                });
            }
        });
        ui.add_space(12.0);

        // ---- Storage
        if self.cache_bytes.is_none() {
            self.cache_bytes = Some(dir_size(&thumb_cache_dir()) + dir_size(&icons_dir()));
        }
        let cache = self.cache_bytes.unwrap_or(0);
        section_sub(ui, "Storage", "Where Gamelo keeps things", |ui| {
            let auto = data_dir().join("prefixes").to_string_lossy().to_string();
            let (mut browse, mut open) = (false, false);
            row_with_buttons(
                ui,
                "Prefixes",
                |ui| {
                    open = ui.button(tr("Open")).clicked();
                    browse = ui.button(tr("Browse…")).clicked();
                },
                |ui| {
                    let w = ui.available_width();
                    let (r, by_menu) = text_field(ui, &mut self.cfg.settings.prefix_base, &auto, w);
                    save |= r.lost_focus() || by_menu;
                },
            );
            if browse && let Some(p) = pick_folder(lg("Folder for new game prefixes")) {
                self.cfg.settings.prefix_base = p.to_string_lossy().into();
                save = true;
            }
            if open {
                acts.open = Some(self.cfg.settings.prefix_base().to_string_lossy().into());
            }
            note(ui, indent, "Only affects games without their own prefix; existing prefixes aren’t moved.");
            ui.add_space(6.0);
            for (label, dir) in [
                ("Settings", config_path().parent().map(Path::to_path_buf).unwrap_or_default()),
                ("Artwork", art_dir()),
                ("Logs", data_dir().join("logs")),
            ] {
                row_with_buttons(
                    ui,
                    label,
                    |ui| {
                        if ui.button(tr("Open")).clicked() {
                            acts.open = Some(dir.to_string_lossy().into());
                        }
                    },
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(dir.to_string_lossy()).color(pal().subtext).small().monospace(),
                            )
                            .truncate(),
                        );
                    },
                );
            }
            row_with_buttons(
                ui,
                "Image cache",
                |ui| {
                    if ui.add_enabled(cache > 0, egui::Button::new(tr("Clear"))).clicked() {
                        acts.clear_cache = true;
                    }
                },
                |ui| {
                    ui.label(egui::RichText::new(format_bytes(cache)).color(pal().text));
                    ui.label(
                        egui::RichText::new(tr("thumbnails and resized artwork, rebuilt when needed"))
                            .color(pal().subtext)
                            .small(),
                    );
                },
            );
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
            brand_mark(ui, rect);
            ui.label(
                egui::RichText::new(format!("Gamelo {}", env!("CARGO_PKG_VERSION")))
                    .color(pal().faint)
                    .small(),
            );
            ui.with_layout(far_end(), |ui| {
                if ui.button(tr("Run first-time setup again")).clicked() {
                    self.setup = Some(setup::Setup::default());
                }
            });
        });
        if save {
            acts.save_settings = true;
        }
    }

    pub(crate) fn page_dependencies(&mut self, ui: &mut egui::Ui, acts: &mut Acts) {
        page_header(ui, "Dependencies", "Install missing tools and detect what’s available");
        let busy = self.download.is_some();
        let rows = &self.deps;
        let distro = distro_name();
        ui.label(
            egui::RichText::new(trf("Detected: {}", &[&distro]))
                .color(pal().subtext)
                .small(),
        );
        ui.add_space(10.0);
        for row in rows {
            card_frame().show(
                ui,
                |ui| {
                    ui.horizontal(|ui| {
                        let col =
                            if row.found { pal().ok } else { pal().warn };
                        status_dot(ui, col, row.found);
                        ui.label(
                            egui::RichText::new(tr(row.name))
                                .strong()
                                .color(pal().text),
                        );
                        ui.with_layout(
                            far_end(),
                            |ui| {
                                if let Some((action, label)) = row.action {
                                    ui.add_enabled_ui(!busy, |ui| {
                                        if primary_button(ui, label)
                                            .clicked()
                                        {
                                            acts.dep = Some(action);
                                        }
                                    });
                                }
                            },
                        );
                    });
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(tr(&row.detail))
                                .color(pal().subtext)
                                .small()
                                .monospace(),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .context_menu(|ui| menu_ui(ui, |ui| {
                        if ui.button(tr("Copy")).clicked() {
                            acts.copy = Some(row.detail.clone());
                            ui.close();
                        }
                    }));
                    if let Some(cmd) = &row.install_cmd {
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(cmd)
                                    .color(pal().info())
                                    .small()
                                    .monospace(),
                            );
                            if ui.small_button(tr("Copy")).clicked() {
                                acts.copy = Some(cmd.clone());
                            }
                        });
                    }
                },
            );
            ui.add_space(8.0);
        }
        ui.add_space(2.0);
        para(ui, "Downloads install into your home directory and need no root.", pal().subtext, true);
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                let label = self
                    .download
                    .as_ref()
                    .map(|d| d.label.clone())
                    .unwrap_or_default();
                ui.label(egui::RichText::new(tr(&label)).color(pal().subtext).small());
            }
            if ui.button(tr("Re-scan")).clicked() {
                acts.rescan = true;
            }
        });
    }
}
