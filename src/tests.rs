//! Unit tests and the offscreen screenshot suites.

use super::*;

#[test]
fn shell_quote_basic() {
    assert_eq!(
        shell_quote("PROTON_FSR4_UPGRADE=1"),
        "PROTON_FSR4_UPGRADE=1"
    );
    assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
    assert_eq!(shell_quote("it's"), "'it'\\''s'");
}

#[test]
fn tokenize_respects_quotes() {
    assert_eq!(
        tokenize(r#"-windowed "-name foo bar" -x"#),
        vec!["-windowed", "-name foo bar", "-x"]
    );
}

#[test]
fn flag_envs_follow_flags() {
    let f = GameFlags {
        fsr4: Fsr4Mode::Rrna3,
        watermark: true,
        ntsync: true,
        ..Default::default()
    };
    let envs = flag_envs(&f);
    assert!(envs.contains(&("PROTON_FSR4_RDNA3_UPGRADE", "1")));
    assert!(envs.contains(&("PROTON_FSR4_INDICATOR", "1")));
    assert!(envs.contains(&("PROTON_USE_NTSYNC", "1")));
    assert!(!envs.iter().any(|(k, _)| *k == "PROTON_ENABLE_WAYLAND"));
}

#[test]
fn url_encode_path_segment() {
    assert_eq!(url_encode("Half-Life 2"), "Half-Life%202");
    assert_eq!(url_encode("a/b?c&d"), "a%2Fb%3Fc%26d");
    assert_eq!(url_encode("Café"), "Caf%C3%A9");
}

#[test]
fn old_config_still_loads() {
    // A config written before umu_id / icon / steamgriddb_key existed.
    let json = r#"{"custom_games":[{"name":"G","exe":"/g.exe"}],"settings":{"zoom":1.0}}"#;
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.custom_games[0].umu_id, "");
    assert_eq!(cfg.custom_games[0].icon, "");
    assert_eq!(cfg.custom_games[0].cover, "");
    assert_eq!(cfg.custom_games[0].hero, "");
    assert!(!cfg.custom_games[0].art_checked);
    assert_eq!(cfg.settings.steamgriddb_key, "");
}

#[test]
fn year_from_timestamp() {
    assert_eq!(year_of(0), 1970);
    assert_eq!(year_of(1_444_918_080), 2015); // Oct 2015
    assert_eq!(year_of(1_704_067_199), 2023); // 2023-12-31 23:59:59
    assert_eq!(year_of(1_704_067_200), 2024); // 2024-01-01 00:00:00
}

#[test]
fn image_fitting() {
    let wide = image::RgbaImage::new(1000, 500);
    assert_eq!(cover_fit(&wide, 600, 900).dimensions(), (600, 900));
    assert_eq!(fit_within(&wide, 240, 360).dimensions(), (240, 120));
    let grey = image::RgbaImage::from_pixel(600, 900, image::Rgba([200; 4]));
    let hero = hero_from(&grey, true);
    assert_eq!(hero.dimensions(), (640, 210));
    // Dimmed, darkest bottom-left (behind the title), brightest top-right.
    let (bl, tr) = (hero.get_pixel(0, 209).0, hero.get_pixel(639, 0).0);
    assert!(bl[0] < tr[0] && bl[0] < 200, "banner must be darker bottom-left");
    assert_eq!(bl[3], 255);
    assert_eq!(hero_from(&grey, false).dimensions(), (1280, 420));
    let poster = generated_poster(Some(&image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255]))), "x");
    assert_eq!(poster.dimensions(), (300, 450));
    assert_eq!(generated_poster(None, "x").dimensions(), (300, 450));
    let small = image::RgbaImage::new(64, 64);
    assert_eq!(fit_within(&small, 240, 360).dimensions(), (64, 64));
}

/// Hits the real SteamGridDB API with the key from the user's config.
/// Run with: cargo test -- --ignored sgdb_live
#[test]
#[ignore]
fn sgdb_live() {
    let key = load_config().settings.steamgriddb_key;
    assert!(!key.trim().is_empty(), "no SteamGridDB key in config");
    let games = sgdb_search("Broforce", key.trim()).unwrap();
    let id = games.first().expect("no match").id;
    for kind in [ArtKind::Icon, ArtKind::Cover] {
        let (assets, _) = sgdb_assets(kind, id, 0, key.trim()).unwrap();
        let first = assets.first().expect("no assets");
        assert!(load_thumb(&first.thumb).is_some(), "thumbnail failed: {}", first.thumb);
    }
}

#[test]
fn rename_keeps_auto_prefix() {
    let base = std::env::temp_dir().join(format!("plaunch-prefixes-{}", std::process::id()));
    fs::create_dir_all(base.join("townfall")).unwrap();
    let settings = Settings {
        prefix_base: base.to_string_lossy().into(),
        ..Default::default()
    };
    let old = CustomGame {
        name: "Townfall".into(),
        ..Default::default()
    };
    // Renamed with an auto prefix: pinned to the existing one.
    let mut renamed = CustomGame {
        name: "silent hill Townfall".into(),
        ..Default::default()
    };
    assert!(keep_prefix_on_rename(&old, &mut renamed, &settings));
    assert_eq!(game_prefix(&renamed, &settings), base.join("townfall"));
    // Same slug (only case changed): nothing to do.
    let mut recased = CustomGame {
        name: "TOWNFALL".into(),
        ..Default::default()
    };
    assert!(!keep_prefix_on_rename(&old, &mut recased, &settings));
    assert!(recased.prefix.is_empty());
    // Old game had no prefix on disk yet: nothing to keep.
    let fresh = CustomGame {
        name: "Never Launched".into(),
        ..Default::default()
    };
    let mut fresh_renamed = CustomGame {
        name: "Other".into(),
        ..Default::default()
    };
    assert!(!keep_prefix_on_rename(&fresh, &mut fresh_renamed, &settings));
    // A custom prefix the user cleared on purpose is not brought back.
    let custom = CustomGame {
        name: "Townfall".into(),
        prefix: "/somewhere/else".into(),
        ..Default::default()
    };
    let mut cleared = CustomGame {
        name: "Renamed".into(),
        ..Default::default()
    };
    assert!(!keep_prefix_on_rename(&custom, &mut cleared, &settings));
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn guesses_names_from_paths() {
    let g = |p: &str| guess_game_name(Path::new(p));
    assert_eq!(g("/games/BroForce-v2354/BroForce.v2354/BroForce.v2354/Broforce.exe"), "BroForce");
    assert_eq!(g("/d/Games/Resonance - A Plague Tale Legacy/Resonance.exe"), "Resonance - A Plague Tale Legacy");
    assert_eq!(g("/d/Games/Silent Hill Townfall/Townfall/Binaries/Win64/Townfall-Win64-Shipping.exe"), "Townfall");
    assert_eq!(g("/d/Games/Half Life 2/hl2.exe"), "Half Life 2");
    assert_eq!(g("/home/u/Games/game.exe"), "game");
    assert_eq!(g("/g/Thronefall-v1-61/Thronefall.v1.61/Thronefall.v1.61/Thronefall.exe"), "Thronefall");
    assert_eq!(g("/g/Enter-the-Gungeon-v2-1-9/Enter.the.Gungeon.v2.1.9/EtG.exe"), "Enter the Gungeon");
    assert_eq!(g("/g/Streets.of.Rogue.v98.2/StreetsOfRogue.exe"), "Streets of Rogue");
}

#[test]
fn desktop_exec_args_are_escaped() {
    assert_eq!(desktop_exec_arg("/usr/bin/gamelo"), "/usr/bin/gamelo");
    assert_eq!(desktop_exec_arg("/a b/app"), "\"/a b/app\"");
    assert_eq!(desktop_exec_arg("/100%/app"), "/100%%/app");
    assert_eq!(desktop_exec_arg("/a $x/app"), "\"/a \\\\$x/app\"");
}

#[test]
fn best_match_prefers_exact_then_verified() {
    let game = |id, name: &str, verified| SgdbGame { id, name: name.into(), year: None, verified };
    let list = [game(1, "Broforce Forever", false), game(2, "Broforce", false), game(3, "Bro", true)];
    assert_eq!(best_match("broforce", &list).unwrap().id, 2);
    assert_eq!(best_match("nothing alike", &list).unwrap().id, 3);
    assert!(best_match("x", &[]).is_none());
}

#[test]
fn sessions_accumulate_in_stats() {
    let dir = std::env::temp_dir().join(format!("plaunch-stats-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    record_session_in(&dir, "abc", 1_000, 600, Some(0));
    record_session_in(&dir, "abc", 5_000, 120, Some(1));
    record_session_in(&dir, "other", 7_000, 30, None);
    let stats = load_stats_in(&dir);
    let s = &stats["abc"];
    assert_eq!((s.playtime_secs, s.last_played, s.last_session_secs, s.last_exit), (720, 5_000, 120, Some(1)));
    assert_eq!(stats["other"].last_exit, None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn durations_and_ages_read_naturally() {
    assert_eq!(format_duration(45), "45s");
    assert_eq!(format_duration(25 * 60), "25m");
    assert_eq!(format_duration(12 * 3600 + 40 * 60), "12h 40m");
    let day = 86_400;
    assert_eq!(format_ago(10 * day, 10 * day + 100), "today");
    assert_eq!(format_ago(10 * day, 11 * day + 5), "yesterday");
    assert_eq!(format_ago(0, 3 * day), "3 days ago");
    assert_eq!(format_ago(0, 21 * day), "3 weeks ago");
}

#[test]
fn process_tree_includes_children() {
    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    let tree = process_tree(std::process::id());
    assert_eq!(tree[0], std::process::id());
    assert!(tree.contains(&child.id()), "spawned child missing from tree");
    let _ = child.kill();
    let _ = child.wait();
}

/// A tiny CPU rasterizer for egui's output, so the UI can be checked
/// without a window.
struct Raster {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
    textures: HashMap<egui::TextureId, egui::ColorImage>,
}

impl Raster {
    fn new(w: usize, h: usize) -> Self {
        Raster { w, h, px: vec![[0.0; 4]; w * h], textures: HashMap::new() }
    }

    fn apply(&mut self, delta: &egui::epaint::textures::TexturesDelta) {
        for (id, deltas) in &delta.set {
            for d in deltas {
                let egui::epaint::ImageData::Color(img) = &d.image;
                match d.pos {
                    None => {
                        self.textures.insert(*id, (**img).clone());
                    }
                    Some([x0, y0]) => {
                        let Some(t) = self.textures.get_mut(id) else { continue };
                        for y in 0..img.size[1] {
                            for x in 0..img.size[0] {
                                t.pixels[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    fn sample(t: &egui::ColorImage, u: f32, v: f32) -> [f32; 4] {
        let x = ((u * t.size[0] as f32) as usize).min(t.size[0] - 1);
        let y = ((v * t.size[1] as f32) as usize).min(t.size[1] - 1);
        let c = t.pixels[y * t.size[0] + x];
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
    }

    fn draw(&mut self, prims: &[egui::ClippedPrimitive]) {
        for p in prims {
            let egui::epaint::Primitive::Mesh(m) = &p.primitive else { continue };
            let tex = self.textures.get(&m.texture_id).cloned();
            let clip = p.clip_rect;
            for tri in m.indices.as_chunks::<3>().0 {
                let v = tri.map(|i| m.vertices[i as usize]);
                let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
                let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = a.x.min(b.x).min(c.x).max(clip.min.x).max(0.0) as usize;
                let y0 = a.y.min(b.y).min(c.y).max(clip.min.y).max(0.0) as usize;
                let x1 = (a.x.max(b.x).max(c.x).min(clip.max.x).ceil() as usize).min(self.w);
                let y1 = (a.y.max(b.y).max(c.y).min(clip.max.y).ceil() as usize).min(self.h);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                        let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let mut col = [0.0f32; 4];
                        for (k, vert) in v.iter().enumerate() {
                            let wk = [w0, w1, w2][k];
                            let c = vert.color;
                            col[0] += wk * c.r() as f32 / 255.0;
                            col[1] += wk * c.g() as f32 / 255.0;
                            col[2] += wk * c.b() as f32 / 255.0;
                            col[3] += wk * c.a() as f32 / 255.0;
                        }
                        if let Some(t) = &tex {
                            let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                            let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                            let s = Self::sample(t, u, vv);
                            for i in 0..4 {
                                col[i] *= s[i];
                            }
                        }
                        // Premultiplied "over".
                        let d = &mut self.px[y * self.w + x];
                        for i in 0..4 {
                            d[i] = col[i] + d[i] * (1.0 - col[3]);
                        }
                    }
                }
            }
        }
    }

    fn save(&self, path: &Path) {
        let img = image::RgbaImage::from_fn(self.w as u32, self.h as u32, |x, y| {
            let p = self.px[y as usize * self.w + x as usize];
            image::Rgba([(p[0] * 255.0) as u8, (p[1] * 255.0) as u8, (p[2] * 255.0) as u8, 255])
        });
        img.save(path).unwrap();
    }
}

/// Render the real UI offscreen (with a copy of your library) and save PNGs.
/// Run alone with: SHOTS=/some/dir cargo test -- --ignored ui_screenshots
#[test]
#[ignore]
fn ui_screenshots() {
    let out = PathBuf::from(std::env::var("SHOTS").expect("set SHOTS to an output dir"));
    fs::create_dir_all(&out).unwrap();
    // Work on a copy of the config so nothing here touches the real one.
    let fake_home = std::env::temp_dir().join(format!("plaunch-shots-{}", std::process::id()));
    let _ = fs::remove_dir_all(&fake_home);
    fs::create_dir_all(fake_home.join(".config/gamelo")).unwrap();
    let _ = fs::copy(config_path(), fake_home.join(".config/gamelo/config.json"));
    // SAFETY: this ignored test is run on its own, so no other thread reads the env.
    unsafe {
        std::env::set_var("HOME", &fake_home);
    }
    let ctx = egui::Context::default();
    configure_style(&ctx);
    let mut app = App::new(find_steam_root());
    app.page = Page::Library;
    app.cfg.settings.zoom = 1.0; // in memory only
    app.cfg.settings.language = "en".into();
    app.cfg.settings.tile_size = String::new();
    app.cfg.settings.no_store = true;
    app.sort = SortMode::Recent;
    app.cfg.custom_games.iter_mut().for_each(|g| g.favorite = false);
    // Online cards change height as data arrives; keep the layout fixed.
    app.cfg.settings.no_hltb = true;
    let (w, h) = (1240usize, 860usize);
    let mut raster = Raster::new(w, h);
    let mut t = 0.0;
    let dt = std::cell::Cell::new(0.1);
    let mut frame = |app: &mut App, raster: &mut Raster, events: Vec<egui::Event>, name: Option<&str>| {
        t += dt.get();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        raster.apply(&output.textures_delta);
        output.textures_delta.clear();
        let prims = ctx.tessellate(output.shapes, output.pixels_per_point);
        if let Some(name) = name {
            raster.px.iter_mut().for_each(|p| *p = [0.0; 4]);
            raster.draw(&prims);
            raster.save(&out.join(name));
        }
    };
    let click = |pos: egui::Pos2| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        ]
    };
    let release = |pos: egui::Pos2| {
        vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]
    };
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("1-library.png"));
    // Hover the first poster (226 sidebar + 26 margin, below the toolbar).
    // Posters start below the toolbar and the filter row.
    let first = egui::pos2(226.0 + 26.0 + 75.0, 280.0);
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(first)], None);
    frame(&mut app, &mut raster, vec![], Some("2-library-hover.png"));
    // Click the poster away from its play button to open the game page.
    let open = egui::pos2(226.0 + 26.0 + 20.0, 250.0);
    frame(&mut app, &mut raster, click(open), None);
    frame(&mut app, &mut raster, release(open), None);
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))], None);
    }
    frame(&mut app, &mut raster, vec![], Some("3-game.png"));
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 600.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -600.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("4-game-scrolled.png"));
    // The add dialog and the remove confirmation.
    app.add_modal = Some(app.new_add_modal(PathBuf::from("/home/u/Games/Hollow Knight/hollow_knight.exe")));
    for _ in 0..2 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("5-add.png"));
    app.add_modal = None;
    app.confirm_remove = app.selected.clone();
    for _ in 0..2 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("6-remove.png"));
    // Artwork page layout (no search, so no network).
    app.confirm_remove = None;
    app.art.game = app.selected_index();
    app.page = Page::Artwork;
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("7-artwork.png"));

    // Typing in a field and then leaving through the sidebar must still save it.
    app.page = Page::Game;
    app.game_tab = GameTab::Settings;
    // A fixed Performance card above Launch: no overlay rows, no frame cap.
    if let Some(i) = app.selected_index() {
        app.cfg.custom_games[i].flags.mangohud = false;
        app.cfg.custom_games[i].flags.gamescope = false;
        app.cfg.custom_games[i].tuning = Tuning::default();
    }
    for _ in 0..2 {
        frame(&mut app, &mut raster, vec![], None);
    }
    // This page remembers its scroll from earlier; go back to the top.
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 3000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    // Bring the Launch card's fields into view.
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -600.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
    for _ in 0..6 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("8b-launch-fields.png"));
    let args_field = egui::pos2(800.0, 512.0);
    frame(&mut app, &mut raster, click(args_field), None);
    frame(&mut app, &mut raster, release(args_field), None);
    frame(&mut app, &mut raster, vec![egui::Event::Text("-dx11".into())], None);
    let sidebar_library = egui::pos2(110.0, 188.0);
    frame(&mut app, &mut raster, click(sidebar_library), None);
    frame(&mut app, &mut raster, release(sidebar_library), None);
    let g = &app.cfg.custom_games[app.selected_index().unwrap()];
    assert!(app.page == Page::Library);
    assert!(g.args.ends_with("-dx11"), "edit lost on sidebar navigation: {:?}", g.args);
    let saved = fs::read_to_string(config_path()).unwrap();
    assert!(saved.contains("-dx11"), "edit not written to the config");

    // Animation states, captured mid-transition.
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    app.search = "town".into();
    dt.set(0.07);
    frame(&mut app, &mut raster, vec![], None);
    frame(&mut app, &mut raster, vec![], Some("8-glide-mid.png"));
    dt.set(0.1);
    app.search.clear();
    for _ in 0..6 {
        frame(&mut app, &mut raster, vec![], None);
    }
    let ids: Vec<String> = app.cfg.custom_games.iter().map(|g| g.id.clone()).collect();
    app.motion.launching.insert(ids[0].clone(), (0.0, std::process::id()));
    app.auto_pending.insert(ids[1].clone());
    dt.set(0.37);
    frame(&mut app, &mut raster, vec![], None);
    frame(&mut app, &mut raster, vec![], Some("9-starting-and-fetching.png"));
    app.selected = Some(ids[0].clone());
    app.page = Page::Game;
    dt.set(0.1);
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("10-game-starting.png"));
    // Back to the grid: the poster flies home to its slot.
    app.motion.launching.clear();
    app.auto_pending.clear();
    app.page = Page::Library;
    dt.set(0.02);
    frame(&mut app, &mut raster, vec![], None);
    dt.set(0.17);
    frame(&mut app, &mut raster, vec![], Some("11-flight-home.png"));
    dt.set(0.1);
    for _ in 0..5 {
        frame(&mut app, &mut raster, vec![], None);
    }
    // A session just ended: playtime counts up with a fading "+1h 25m".
    app.motion.played.insert(ids[3].clone(), (3600, 3600 + 85 * 60, None));
    app.stats.insert(ids[3].clone(), GameStats { playtime_secs: 3600 + 85 * 60, last_played: unix_now() - 60, ..Default::default() });
    dt.set(0.2);
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("12-playtime-count.png"));
    // Right-click menus: a poster, empty library space, a text field.
    app.motion.played.clear();
    dt.set(0.1);
    let right = |pos: egui::Pos2, pressed: bool| {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() },
        ]
    };
    let away = egui::pos2(1150.0, 780.0);
    for (pos, name) in [
        (egui::pos2(335.0, 350.0), "13-menu-poster.png"),
        (egui::pos2(900.0, 640.0), "14-menu-library.png"),
        (egui::pos2(878.0, 52.0), "15-menu-text.png"),
    ] {
        for _ in 0..3 {
            frame(&mut app, &mut raster, vec![], None);
        }
        frame(&mut app, &mut raster, right(pos, true), None);
        frame(&mut app, &mut raster, right(pos, false), None);
        frame(&mut app, &mut raster, vec![], None);
        frame(&mut app, &mut raster, vec![], Some(name));
        frame(&mut app, &mut raster, click(away), None);
        frame(&mut app, &mut raster, release(away), None);
    }
    // The text menu works: "Clear" empties the search box.
    app.search = "ved".into();
    let search = egui::pos2(878.0, 52.0);
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, right(search, true), None);
    frame(&mut app, &mut raster, right(search, false), None);
    frame(&mut app, &mut raster, vec![], Some("16-menu-text-filled.png"));
    let clear_item = egui::pos2(906.0, 268.0);
    frame(&mut app, &mut raster, click(clear_item), None);
    frame(&mut app, &mut raster, release(clear_item), None);
    assert!(app.search.is_empty(), "Clear didn't clear: {:?}", app.search);
    // Settings, top and bottom; then a scale preset click applies and saves.
    app.page = Page::Settings;
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("17-settings.png"));
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -2000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    frame(&mut app, &mut raster, vec![], Some("18-settings-bottom.png"));
    // Clicking a scale preset applies it and saves it straight away.
    frame(&mut app, &mut raster, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, 3000.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Default::default() }], None);
    for _ in 0..4 {
        frame(&mut app, &mut raster, vec![], None);
    }
    let preset_125 = egui::pos2(726.0, 752.0);
    frame(&mut app, &mut raster, click(preset_125), None);
    frame(&mut app, &mut raster, release(preset_125), None);
    for _ in 0..3 {
        frame(&mut app, &mut raster, vec![], None);
    }
    assert_eq!(app.cfg.settings.zoom, 1.25);
    assert!((ctx.zoom_factor() - 1.25).abs() < 1e-4, "zoom not applied: {}", ctx.zoom_factor());
    let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path()).unwrap()).unwrap();
    assert_eq!(saved.settings.zoom, 1.25, "preset not saved");
    // Ctrl − changes it too, and is saved once it settles.
    let minus = egui::Event::Key {
        key: egui::Key::Minus,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    };
    frame(&mut app, &mut raster, vec![minus], None);
    for _ in 0..12 {
        frame(&mut app, &mut raster, vec![], None);
    }
    let z = app.cfg.settings.zoom;
    assert!(z < 1.25, "Ctrl − ignored: {z}");
    let saved: AppConfig = serde_json::from_str(&fs::read_to_string(config_path()).unwrap()).unwrap();
    assert!((saved.settings.zoom - z).abs() < 1e-4, "Ctrl − zoom not saved: {} vs {z}", saved.settings.zoom);
    let _ = fs::remove_dir_all(&fake_home);
}

/// Offscreen driver for the real UI: feed frames, save PNGs.
struct Harness {
    ctx: egui::Context,
    app: App,
    raster: Raster,
    t: f64,
    out: PathBuf,
    home: PathBuf,
    /// How soon the last frame asked to be drawn again.
    repaint: Duration,
}

impl Harness {
    /// The UI on a copy of your config (in a fake HOME), at 1240×860.
    fn new(tag: &str) -> Self {
        let out = PathBuf::from(std::env::var("SHOTS").expect("set SHOTS to an output dir"));
        fs::create_dir_all(&out).unwrap();
        let real = config_path();
        let home = std::env::temp_dir().join(format!("plaunch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".config/gamelo")).unwrap();
        let _ = fs::copy(&real, home.join(".config/gamelo/config.json"));
        // Stats, artwork cache and save data are read from the real data dir copy.
        let real_data = data_dir();
        let data = home.join(".local/share/gamelo");
        fs::create_dir_all(&data).unwrap();
        let _ = fs::copy(real_data.join("stats.json"), data.join("stats.json"));
        fs::create_dir_all(data.join("cache")).unwrap();
        if let Ok(rd) = fs::read_dir(real_data.join("cache")) {
            for e in rd.flatten() {
                let _ = fs::copy(e.path(), data.join("cache").join(e.file_name()));
            }
        }
        // Read-only view of Steam, so "find saves elsewhere" has something to find.
        let real_steam = real.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|h| h.join(".local/share/Steam"));
        if let Some(steam) = real_steam.filter(|s| s.is_dir()) {
            let _ = std::os::unix::fs::symlink(steam, data.parent().unwrap().join("Steam"));
        }
        // SAFETY: these ignored tests run alone (--test-threads=1).
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let ctx = egui::Context::default();
        configure_style(&ctx);
        let mut app = App::new(find_steam_root());
        // Start from the defaults whatever the real config has picked.
        app.cfg.settings.zoom = 1.0;
        app.cfg.settings.language = "en".into();
        app.cfg.settings.tile_size = String::new();
        app.cfg.settings.no_store = true; // no network in screenshots
        app.sort = SortMode::Recent;
        app.cfg.custom_games.iter_mut().for_each(|g| g.favorite = false);
        Harness { ctx, app, raster: Raster::new(1240, 860), t: 0.0, out, home, repaint: Duration::MAX }
    }

    fn frame_dt(&mut self, dt: f64, events: Vec<egui::Event>, name: Option<&str>) {
        self.t += dt;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| app.draw(ui));
        self.repaint = output.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        self.raster.apply(&output.textures_delta);
        output.textures_delta.clear();
        let prims = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        if let Some(name) = name {
            self.raster.px.iter_mut().for_each(|p| *p = [0.0; 4]);
            self.raster.draw(&prims);
            self.raster.save(&self.out.join(name));
        }
    }

    fn idle(&mut self, n: usize) {
        for _ in 0..n {
            self.frame_dt(0.1, vec![], None);
        }
    }

    fn shot(&mut self, name: &str) {
        self.idle(4);
        self.frame_dt(0.1, vec![], Some(name));
    }

    fn scroll(&mut self, dy: f32) {
        let ev = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Default::default(),
        };
        self.frame_dt(0.1, vec![egui::Event::PointerMoved(egui::pos2(700.0, 500.0)), ev], None);
        self.idle(3);
    }

    fn click(&mut self, pos: egui::Pos2, button: egui::PointerButton) {
        let ev = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: Default::default() };
        self.frame_dt(0.1, vec![egui::Event::PointerMoved(pos), ev(true)], None);
        self.frame_dt(0.1, vec![ev(false)], None);
    }

    fn hover(&mut self, pos: egui::Pos2) {
        self.frame_dt(0.1, vec![egui::Event::PointerMoved(pos)], None);
        self.idle(8);
    }

    fn key(&mut self, key: egui::Key) {
        let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() };
        self.frame_dt(0.1, vec![ev(true), ev(false)], None);
    }

    /// Show `page` (for game pages, of the first game).
    fn page(&mut self, page: Page) {
        if self.app.selected.is_none() {
            self.app.selected = self.app.cfg.custom_games.first().map(|g| g.id.clone());
        }
        self.app.page = page;
        self.idle(2);
        self.scroll(5000.0);
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

/// Light mode, square corners, other themes: SHOTS=dir cargo test -- --ignored theme_shots --test-threads=1
#[test]
#[ignore]
fn theme_shots() {
    let mut h = Harness::new("themes");
    h.app.cfg.settings.theme_mode = "light".into();
    h.page(Page::Library);
    h.shot("t1-light-library.png");
    h.page(Page::Game);
    h.shot("t2-light-game.png");
    h.page(Page::Settings);
    h.shot("t3-light-settings.png");
    h.app.cfg.settings.square_corners = true;
    h.app.cfg.settings.light_theme = "rose".into();
    h.page(Page::Library);
    h.shot("t4-rose-square-library.png");
    h.app.cfg.settings.theme_mode = "dark".into();
    h.app.cfg.settings.dark_theme = "ocean".into();
    h.page(Page::Game);
    h.shot("t5-ocean-square-game.png");
    h.app.cfg.settings.square_corners = false;
    h.app.cfg.settings.dark_theme = "galaxy".into();
    h.page(Page::Settings);
    h.shot("t6-galaxy-settings.png");
    h.app.cfg.settings.theme_mode = "light".into();
    h.app.cfg.settings.light_theme = "arctic".into();
    h.page(Page::Proton);
    h.shot("t7-arctic-proton.png");
    h.page(Page::Saves);
    h.shot("t8-arctic-saves.png");
    h.app.cfg.settings.light_theme = "botanical".into();
    h.app.game_tab = GameTab::Mods;
    h.page(Page::Game);
    h.scroll(-400.0);
    h.shot("t9-botanical-mods.png");
    h.app.cfg.settings.theme_mode = "dark".into();
    h.idle(1);
    assert!(pal().dark);
}

/// The Proton page with live release data: SHOTS=dir cargo test -- --ignored proton_shots --test-threads=1
#[test]
#[ignore]
fn proton_shots() {
    let mut h = Harness::new("proton");
    h.page(Page::Proton);
    for _ in 0..150 {
        if h.app.proton.releases.contains_key("ge") && h.app.proton.fixes.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        h.idle(1);
    }
    assert!(matches!(h.app.proton.releases.get("ge"), Some(Ok(l)) if !l.is_empty()), "no GE releases");
    h.shot("p1-proton.png");
    h.scroll(-700.0);
    h.shot("p2-proton-mid.png");
    h.scroll(-3000.0);
    h.shot("p3-proton-fixes.png");
}

/// The Save Vault: SHOTS=dir cargo test -- --ignored saves_shots --test-threads=1
#[test]
#[ignore]
fn saves_shots() {
    let mut h = Harness::new("saves");
    let town = h.app.cfg.custom_games.iter().find(|g| g.name.to_lowercase().contains("townfall")).map(|g| g.id.clone());
    h.app.selected = town.or_else(|| h.app.cfg.custom_games.first().map(|g| g.id.clone()));
    h.page(Page::Game);
    for _ in 0..50 {
        if h.app.saves.index.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        h.idle(1);
    }
    assert!(h.app.saves.index.is_some(), "save index not loaded");
    h.scroll(-900.0);
    h.shot("s1-game-saves-card.png");
    h.page(Page::Saves);
    h.shot("s2-vault.png");
    // Scan for other copies (button in the third card).
    let i = h.app.selected_index().unwrap();
    let gs = h.app.game_saves(i);
    let found = saves::find_elsewhere(&gs, &h.app.cfg.settings);
    h.app.saves.found.insert(gs.game.id.clone(), found);
    h.scroll(-700.0);
    h.shot("s3-vault-elsewhere.png");
    h.scroll(-3000.0);
    h.shot("s4-vault-sync.png");
    h.app.saves.search = Some("silent hill".into());
    h.shot("s5-db-search.png");
}

/// Game page cards that fetch online info: SHOTS=dir cargo test -- --ignored game_shots --test-threads=1
#[test]
#[ignore]
fn game_shots() {
    let mut h = Harness::new("game");
    let valheim = h.app.cfg.custom_games.iter().find(|g| g.name == "Valheim").map(|g| g.id.clone());
    h.app.selected = valheim.or_else(|| h.app.cfg.custom_games.first().map(|g| g.id.clone()));
    let id = h.app.selected.clone().unwrap();
    h.app.stats.insert(id.clone(), GameStats { playtime_secs: 31 * 3600 + 20 * 60, last_played: unix_now() - 7200, ..Default::default() });
    h.page(Page::Game);
    for _ in 0..80 {
        if h.app.hltb.cache.contains_key(&id) || h.app.hltb.errors.contains_key(&id) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        h.idle(1);
    }
    assert!(h.app.hltb.cache.get(&id).is_some_and(|e| e.times.is_some()), "no HLTB data: {:?}", h.app.hltb.errors.get(&id));
    h.app.motion.page_since = h.app.now;
    h.scroll(-380.0);
    h.idle(10);
    h.shot("g1-hltb.png");
}

/// The OptiScaler card (live release list): SHOTS=dir cargo test -- --ignored opti_shots --test-threads=1
#[test]
#[ignore]
fn opti_shots() {
    let mut h = Harness::new("opti");
    let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone());
    h.app.selected = id;
    h.page(Page::Game);
    for _ in 0..60 {
        if h.app.opti.releases.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        h.idle(1);
    }
    h.scroll(-1500.0);
    h.shot("o1-optiscaler.png");
    h.app.cfg.settings.theme_mode = "light".into();
    h.shot("o2-optiscaler-light.png");
}

/// The interface in Arabic (RTL): SHOTS=dir cargo test -- --ignored arabic_shots --test-threads=1
#[test]
#[ignore]
fn arabic_shots() {
    let mut h = Harness::new("arabic");
    h.app.cfg.settings.language = "ar".into();
    h.page(Page::Library);
    h.shot("a1-library.png");
    let id = h.app.cfg.custom_games.iter().find(|g| g.name == "Valheim").map(|g| g.id.clone());
    h.app.selected = id;
    h.page(Page::Game);
    h.shot("a2-game.png");
    h.scroll(-600.0);
    h.shot("a3-game-mid.png");
    h.page(Page::Settings);
    h.shot("a4-settings.png");
    h.scroll(-900.0);
    h.shot("a5-settings-mid.png");
    h.page(Page::Proton);
    h.shot("a6-proton.png");
    h.page(Page::Saves);
    h.shot("a7-saves.png");
    h.app.confirm_remove = h.app.selected.clone();
    h.shot("a8-remove.png");
    h.app.confirm_remove = None;
    h.app.cfg.settings.theme_mode = "light".into();
    h.page(Page::Library);
    h.shot("a9-library-light.png");
    i18n::set_lang(i18n::Lang::En);
}

/// Store info on the game page (needs the network: real Steam data):
/// SHOTS=dir cargo test -- --ignored store_shots --test-threads=1
#[test]
#[ignore]
fn store_shots() {
    let mut h = Harness::new("store");
    h.app.cfg.settings.no_hltb = true;
    h.app.cfg.settings.no_store = false;
    let id = h.app.cfg.custom_games.iter().find(|g| g.name.to_lowercase().contains("broforce")).map(|g| g.id.clone()).expect("Broforce in the library");
    h.app.selected = Some(id.clone());
    for (lang, steam) in [("en", "english"), ("ar", "arabic")] {
        h.app.cfg.settings.language = lang.into();
        let info = store::details(274190, steam).unwrap();
        h.app.store.cache.insert(id.clone(), store::Entry { info, fetched: unix_now(), pinned: true });
        h.page(Page::Game);
        h.app.game_tab = GameTab::Overview;
        h.scroll(-380.0);
        // Let the thumbnails arrive.
        for _ in 0..80 {
            h.idle(1);
            if h.app.store.images.len() >= 4 && h.app.store.loading.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        h.shot(&format!("s1-about-{lang}.png"));
        h.app.store.viewer = Some((id.clone(), 1));
        for _ in 0..60 {
            h.idle(1);
            if h.app.store.loading.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        h.shot(&format!("s2-viewer-{lang}.png"));
        h.key(egui::Key::ArrowRight);
        assert_eq!(h.app.store.viewer.as_ref().map(|v| v.1), Some(2), "arrow flips to the next screenshot");
        h.key(egui::Key::Escape);
        h.idle(2);
        assert!(h.app.store.viewer.is_none() && h.app.page == Page::Game, "Escape closes the viewer only");
    }
}

/// Per-game tuning rows and the Disk space page:
/// SHOTS=dir cargo test -- --ignored tune_shots --test-threads=1
#[test]
#[ignore]
fn tune_shots() {
    let mut h = Harness::new("tune");
    let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone()).unwrap();
    let i = h.app.index_of(&id).unwrap();
    h.app.cfg.custom_games[i].flags.mangohud = true;
    h.app.cfg.custom_games[i].flags.gamescope = false;
    h.app.cfg.custom_games[i].tuning = Tuning { fps_limit: 60, overlay: "standard".into(), position: "top-right".into(), custom: String::new() };
    h.app.selected = Some(id.clone());
    h.page(Page::Game);
    h.app.game_tab = GameTab::Settings;
    h.idle(2);
    h.scroll(-380.0);
    h.shot("t1-tuning.png");
    let l = build_launch(&h.app.cfg.custom_games[i], &h.app.cfg.settings);
    if let Ok(l) = l {
        assert!(l.envs.iter().any(|(k, v)| k == "MANGOHUD_CONFIG" && v.contains("fps_limit=60") && v.contains("position=top-right")));
    }
    // A prefix no game uses, a shader cache and some logs to clean up.
    let orphan = h.app.cfg.settings.prefix_base().join("old_game");
    fs::create_dir_all(orphan.join("drive_c/users")).unwrap();
    fs::write(orphan.join("system.reg"), vec![0u8; 3_000_000]).unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    fs::create_dir_all(home.join(".cache/mesa_shader_cache")).unwrap();
    fs::write(home.join(".cache/mesa_shader_cache/index"), vec![0u8; 5_000_000]).unwrap();
    fs::create_dir_all(data_dir().join("logs")).unwrap();
    fs::write(data_dir().join("logs/a.log"), "x".repeat(40_000)).unwrap();
    h.page(Page::Cleanup);
    for _ in 0..30 {
        h.idle(1);
        if h.app.cleanup.sizes.lock().unwrap().values().all(|v| v.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.shot("t2-disk-space.png");
    assert!(h.app.orphan_prefixes().contains(&orphan));
    // Moving it to the Trash takes it off the list.
    let trash = home.join(".local/share/Trash");
    assert!(cleanup::move_to_trash(&orphan).is_ok());
    assert!(!orphan.exists() && (trash.join("files/old_game").is_dir() || find_in_path("gio").is_some()));
    h.app.cfg.settings.language = "ar".into();
    h.page(Page::Cleanup);
    h.shot("t3-disk-space-ar.png");
    h.app.cfg.settings.language = "en".into();
}

/// The Stores page in each state (no network: a stand-in tool and account):
/// SHOTS=dir cargo test -- --ignored stores_shots --test-threads=1
#[test]
#[ignore]
fn stores_shots() {
    const STORE_TILE: (f32, f32) = (600.0, 400.0);
    const ACCOUNT_CHIP: (f32, f32) = (960.0, 59.0);
    const ACCOUNT_CHIP_AR: (f32, f32) = (260.0, 59.0);
    use stores::{Installed, Owned, SizeInfo, Store};
    let mut h = Harness::new("stores");
    h.app.page = Page::Stores;
    h.page(Page::Stores);
    h.shot("st1-no-tool.png");
    // A stand-in tool: signed out shows the sign-in steps.
    let tools = data_dir().join("tools");
    fs::create_dir_all(&tools).unwrap();
    fs::write(tools.join("legendary"), "#!/bin/sh\nexit 1\n").unwrap();
    h.app.stores.code = "not a code".into();
    h.idle(3);
    h.shot("st2-sign-in.png");
    h.app.stores.code = "0123456789abcdef0123456789abcdef".into();
    h.app.stores.opened.insert(Store::Epic);
    h.app.stores.errors.insert(Store::Epic, "the code has expired".into());
    h.idle(3);
    h.shot("st2b-code-ok.png");
    h.app.stores.errors.clear();
    h.app.stores.code.clear();
    h.app.cfg.settings.language = "ar".into();
    h.idle(3);
    h.shot("st2c-sign-in-ar.png");
    h.app.cfg.settings.language = "en".into();
    // Signed in, with a library (the refresh is marked done so nothing runs).
    let cfg = std::env::var_os("HOME").map(PathBuf::from).unwrap().join(".config/legendary");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("user.json"), r#"{"displayName":"Tester","account_id":"x"}"#).unwrap();
    h.app.stores.loaded.insert(Store::Epic);
    // Covers from the copied library stand in for store art (already "downloaded").
    let covers: Vec<String> = h.app.cfg.custom_games.iter().map(|g| g.cover.clone()).filter(|c| Path::new(c).is_file()).collect();
    let owned = |app: &str, title: &str, version: &str| {
        let k = app.len() % covers.len().max(1);
        let cover = covers.get(k).map(|c| {
            let url = format!("https://test/{app}.jpg");
            let path = store::image_path(&url);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::copy(c, &path).unwrap();
            url
        });
        Owned { app: app.into(), title: title.into(), version: version.into(), cover: cover.unwrap_or_default(), ..Default::default() }
    };
    h.app.stores.owned.insert(Store::Epic, vec![owned("Fortnite", "Fortnite", "2"), owned("Hades", "Hades", "1"), owned("Control", "Control", "5"), owned("Celeste", "Celeste", "1"), owned("Alan", "Alan Wake 2", "1")]);
    h.app.stores.installed.insert(Store::Epic, vec![
        Installed { app: "Hades".into(), title: "Hades".into(), path: "/games/Hades".into(), version: "1".into() },
        Installed { app: "Control".into(), title: "Control".into(), path: "/games/Control".into(), version: "4".into() },
    ]);
    h.idle(3);
    h.shot("st3-library.png");
    h.hover(egui::pos2(STORE_TILE.0, STORE_TILE.1));
    h.shot("st3b-hover.png");
    h.click(egui::pos2(STORE_TILE.0, STORE_TILE.1 + 40.0), egui::PointerButton::Secondary);
    h.idle(3);
    h.shot("st3c-menu.png");
    h.key(egui::Key::Escape);
    h.app.cfg.settings.library_view = "list".into();
    h.idle(3);
    h.shot("st3d-list.png");
    h.app.cfg.settings.library_view = "grid".into();
    h.click(egui::pos2(ACCOUNT_CHIP.0, ACCOUNT_CHIP.1), egui::PointerButton::Primary);
    h.idle(4);
    h.shot("st3e-account.png");
    h.key(egui::Key::Escape);
    h.app.cfg.settings.language = "ar".into();
    h.idle(4);
    h.click(egui::pos2(ACCOUNT_CHIP_AR.0, ACCOUNT_CHIP_AR.1), egui::PointerButton::Primary);
    h.idle(4);
    h.shot("st3g-account-ar.png");
    h.app.cfg.settings.language = "en".into();
    h.idle(2);
    h.key(egui::Key::Escape);
    h.idle(2);
    h.app.stores.show = stores_ui::Show::Updates;
    h.idle(2);
    h.shot("st4-updates.png");
    h.app.stores.show = stores_ui::Show::All;
    h.app.stores.confirm = Some((Store::Epic, owned("Celeste", "Celeste", "1"), Some(Ok(SizeInfo { download: 1_200_000_000, disk: 1_650_000_000, folder: "Celeste".into() }))));
    h.idle(3);
    h.shot("st5-install.png");
    h.app.stores.confirm = None;
    h.app.stores.uninstall = Some((Store::Epic, Installed { app: "Hades".into(), title: "Hades".into(), path: "/games/Hades".into(), version: "1".into() }, true));
    h.idle(3);
    h.shot("st6-uninstall.png");
    h.app.stores.uninstall = None;
    h.app.cfg.settings.language = "ar".into();
    h.idle(3);
    h.shot("st7-library-ar.png");
    h.app.stores.tab = Store::Gog;
    h.idle(3);
    h.shot("st8-gog-ar.png");
    h.app.cfg.settings.language = "en".into();
}

/// Every page goes quiet when nothing moves (no repaint loop burning CPU):
/// cargo test -- --ignored idle_pages --test-threads=1
#[test]
#[ignore]
fn idle_pages() {
    let mut h = Harness::new("idle");
    h.app.cfg.settings.no_store = true;
    let mut busy = Vec::new();
    for page in [Page::Home, Page::Library, Page::Stats, Page::Stores, Page::Launchers, Page::Emulation, Page::Settings, Page::Proton, Page::Cleanup, Page::Dependencies] {
        h.app.page = page;
        // Let entrance animations and loads settle, with the pointer away.
        h.frame_dt(0.5, vec![egui::Event::PointerGone], None);
        for _ in 0..40 {
            h.frame_dt(0.5, vec![], None);
        }
        // A one-off (a background job finishing) is fine; every frame is a loop.
        let mut looping = true;
        for _ in 0..3 {
            h.frame_dt(0.5, vec![], None);
            looping &= h.repaint < Duration::from_millis(500);
        }
        if looping {
            let causes: Vec<String> = h.ctx.repaint_causes().iter().map(|c| c.to_string()).collect();
            busy.push(format!("{page:?}: {:?} {causes:?} busy={} looping={}", h.repaint, h.app.motion.busy, h.app.motion.looping));
        }
    }
    assert!(busy.is_empty(), "pages still repainting while idle: {busy:?}");
}

/// The Emulation page with a few stand-in ROMs:
/// SHOTS=dir cargo test -- --ignored emulation_shots --test-threads=1
#[test]
#[ignore]
fn emulation_shots() {
    let mut h = Harness::new("emulation");
    let roms = h.home.join("ROMs");
    for f in ["snes/Super Mario World (USA).sfc", "snes/Chrono Trigger (USA).sfc", "gba/Metroid Fusion (Europe).gba", "ps2/Shadow of the Colossus.iso", "Celeste Classic.nes"] {
        let p = roms.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"rom").unwrap();
    }
    h.app.cfg.settings.rom_dirs = vec![roms.to_string_lossy().into()];
    let library = h.app.library_order().len();
    h.app.page = Page::Emulation;
    h.idle(4);
    let n = h.app.cfg.custom_games.iter().filter(|g| g.rom.is_some()).count();
    assert_eq!(n, 5, "every ROM is found");
    assert_eq!(h.app.library_order().len(), library, "ROMs stay out of the Library");
    h.shot("e1-emulation.png");
    h.app.cfg.settings.language = "ar".into();
    h.idle(3);
    h.shot("e2-emulation-ar.png");
    h.app.cfg.settings.language = "en".into();
}

/// The Launchers page with a stand-in Ubisoft Connect and one game:
/// SHOTS=dir cargo test -- --ignored launcher_shots --test-threads=1
#[test]
#[ignore]
fn launcher_shots() {
    let mut h = Harness::new("launchers");
    let d = launchers::def("ubisoft").unwrap();
    let pfx = launchers::prefix(&h.app.cfg.settings, d);
    let c = pfx.join("drive_c");
    let ubi = c.join("Program Files (x86)/Ubisoft/Ubisoft Game Launcher");
    let game = ubi.join("games/Far Cry 5");
    fs::create_dir_all(&game).unwrap();
    fs::create_dir_all(c.join("windows/command")).unwrap();
    for f in [ubi.join("UbisoftConnect.exe"), c.join("windows/command/start.exe"), game.join("FarCry5.exe")] {
        fs::write(f, b"MZ").unwrap();
    }
    fs::write(pfx.join("system.reg"), "WINE REGISTRY Version 2\n\n[Software\\\\Wow6432Node\\\\Ubisoft\\\\Launcher\\\\Installs\\\\4311] 1700000000\n\"InstallDir\"=\"C:/Program Files (x86)/Ubisoft/Ubisoft Game Launcher/games/Far Cry 5/\"\n").unwrap();
    let before = h.app.cfg.custom_games.len();
    h.app.page = Page::Launchers;
    h.idle(4);
    assert_eq!(h.app.cfg.custom_games.len(), before + 2, "the launcher and its game join the library");
    let fc = h.app.cfg.custom_games.iter().find(|g| g.name == "Far Cry 5").unwrap();
    assert_eq!(fc.args, "uplay://launch/4311/0");
    assert_eq!(fc.launcher.as_ref().unwrap().watch, game.to_string_lossy());
    h.shot("l1-launchers.png");
    h.app.cfg.settings.language = "ar".into();
    h.idle(3);
    h.shot("l2-launchers-ar.png");
    h.app.cfg.settings.language = "en".into();
    h.app.launcher_remove = Some("ubisoft");
    h.idle(3);
    h.shot("l3-remove.png");
    h.app.launcher_remove = None;
}

/// Ctrl+K quick search and first-run setup:
/// SHOTS=dir cargo test -- --ignored polish_shots --test-threads=1
#[test]
#[ignore]
fn polish_shots() {
    let mut h = Harness::new("polish");
    h.page(Page::Library);
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    let ev = |pressed| egui::Event::Key { key: egui::Key::K, physical_key: None, pressed, repeat: false, modifiers: ctrl };
    h.frame_dt(0.1, vec![ev(true), ev(false)], None);
    h.idle(3);
    h.shot("p1-quick-empty.png");
    h.frame_dt(0.1, vec![egui::Event::Text("re".into())], None);
    h.idle(2);
    h.key(egui::Key::ArrowDown);
    h.idle(2);
    h.shot("p2-quick-typed.png");
    h.app.cfg.settings.language = "ar".into();
    h.idle(3);
    h.shot("p3-quick-ar.png");
    h.key(egui::Key::Escape);
    h.idle(2);
    assert!(h.app.quick.is_none(), "Escape closes quick search");
    h.app.setup = Some(setup::Setup::default());
    h.idle(3);
    h.shot("p4-setup-ar.png");
    h.app.cfg.settings.language = "en".into();
    h.idle(3);
    h.shot("p5-setup-tools.png");
    h.app.setup = Some(setup::Setup { step: 1 });
    h.idle(3);
    h.shot("p6-setup-games.png");
    h.app.setup = None;
    h.app.crash_at = Some(1);
    h.app.safety_notice = Some(i18n::logical("Gamelo closed unexpectedly last time. The crash log can help fix it.").into());
    h.page(Page::Home);
    h.shot("p7-crash-banner.png");
    h.app.safety_notice = None;
    h.app.crash_at = None;
}

/// Home and Stats with two months of made-up sessions:
/// SHOTS=dir cargo test -- --ignored home_shots --test-threads=1
#[test]
#[ignore]
fn home_shots() {
    let mut h = Harness::new("home");
    let order = h.app.library_order();
    let ids: Vec<String> = order.iter().map(|&i| h.app.cfg.custom_games[i].id.clone()).collect();
    // Sessions on most days of the last ~70 days, heavier at weekends.
    let now = unix_now();
    let mut log = Vec::new();
    for back in 0..70u64 {
        if back % 5 == 3 {
            continue;
        }
        let start = now - back * 86_400 - 3 * 3600;
        let weekend = sessions::weekday(sessions::day_of(start)) >= 5;
        let secs = (25 + (back * 37) % 140) * 60 * if weekend { 2 } else { 1 };
        log.push(sessions::Session { id: ids[(back as usize / 3) % 4].clone(), start, secs });
    }
    for s in &log {
        let st = h.app.stats.entry(s.id.clone()).or_default();
        st.playtime_secs += s.secs;
        st.last_played = st.last_played.max(s.start);
    }
    h.app.sessions = log;
    h.app.cfg.custom_games[order[6]].status = GameStatus::Backlog;
    h.app.home.free = Some(vec![]);
    h.app.page = Page::Home;
    h.idle(4);
    h.shot("h1-home.png");
    h.scroll(-600.0);
    h.shot("h2-home-bottom.png");
    h.app.page = Page::Stats;
    h.idle(2);
    h.scroll(5000.0);
    h.shot("h3-stats.png");
    h.scroll(-700.0);
    h.shot("h4-stats-mid.png");
    h.scroll(-2000.0);
    h.shot("h5-stats-bottom.png");
    h.app.home.range = stats_ui::Range::Year;
    h.scroll(5000.0);
    h.shot("h6-stats-year.png");
    h.app.cfg.settings.language = "ar".into();
    h.app.page = Page::Home;
    h.idle(3);
    h.scroll(5000.0);
    h.shot("h7-home-ar.png");
    h.app.page = Page::Stats;
    h.idle(3);
    h.scroll(5000.0);
    h.shot("h8-stats-ar.png");
    h.scroll(-700.0);
    h.shot("h9-stats-ar-mid.png");
    h.app.cfg.settings.language = "en".into();
}

/// Statuses, collections and the list view:
/// SHOTS=dir cargo test -- --ignored org_shots --test-threads=1
#[test]
#[ignore]
fn org_shots() {
    use egui::PointerButton::{Primary, Secondary};
    let mut h = Harness::new("org");
    h.app.cfg.settings.collections = vec!["Co-op".into(), "Soulslikes".into()];
    let order = h.app.library_order();
    let set = [GameStatus::Playing, GameStatus::Finished, GameStatus::Backlog, GameStatus::Dropped, GameStatus::Backlog];
    for (k, st) in set.into_iter().enumerate() {
        h.app.cfg.custom_games[order[k]].status = st;
    }
    h.app.cfg.custom_games[order[0]].collections = vec!["Co-op".into()];
    h.app.cfg.custom_games[order[3]].collections = vec!["Co-op".into(), "Soulslikes".into()];
    h.page(Page::Library);
    h.shot("o1-grid-statuses.png");
    // Only filters with games show up; picking one narrows the grid.
    let labels: Vec<String> = h.app.filter_options().into_iter().map(|(_, l)| l).collect();
    assert!(labels.contains(&"Backlog".to_string()) && labels.contains(&"Co-op".to_string()));
    h.app.lib_filter = LibFilter::Status(GameStatus::Backlog);
    h.idle(4);
    assert_eq!(h.app.library_order().len(), 2);
    h.shot("o2-filter-backlog.png");
    h.app.lib_filter = LibFilter::Collection("Co-op".into());
    h.idle(4);
    assert_eq!(h.app.library_order().len(), 2);
    h.app.lib_filter = LibFilter::All;
    // List view, sorted by clicking a header.
    h.app.cfg.settings.library_view = "list".into();
    h.page(Page::Library);
    h.shot("o3-list.png");
    // Right-click a row: its game menu, with Status and Collections.
    h.click(egui::pos2(500.0, 520.0), Secondary);
    h.idle(2);
    h.shot("o4-row-menu.png");
    h.key(egui::Key::Escape);
    // A new collection from the dialog.
    h.app.collection_edit = Some(CollectionEdit { game: Some(h.app.cfg.custom_games[order[1]].id.clone()), ..Default::default() });
    h.idle(2);
    h.frame_dt(0.1, vec![egui::Event::Text("Cozy".into())], None);
    h.shot("o5-new-collection.png");
    h.key(egui::Key::Enter);
    h.idle(2);
    assert!(h.app.cfg.settings.collections.contains(&"Cozy".to_string()));
    assert!(h.app.cfg.custom_games[order[1]].collections.contains(&"Cozy".to_string()));
    // Rename it, then delete it: games follow.
    h.app.collection_edit = Some(CollectionEdit { game: None, renaming: Some("Cozy".into()), name: "Comfy".into() });
    h.idle(2);
    h.key(egui::Key::Enter);
    h.idle(2);
    assert!(h.app.cfg.custom_games[order[1]].collections.contains(&"Comfy".to_string()));
    h.app.cfg.settings.library_view = String::new();
    // Arabic list view.
    h.app.cfg.settings.language = "ar".into();
    h.app.cfg.settings.library_view = "list".into();
    h.page(Page::Library);
    h.shot("o6-list-ar.png");
    h.app.cfg.settings.library_view = String::new();
    h.app.cfg.settings.language = "en".into();
    // Couch details: View steps the status.
    h.page(Page::Library);
    h.key(egui::Key::F11);
    h.idle(4);
    h.key(egui::Key::I);
    let focus = h.app.couch.as_ref().unwrap().focus.clone().unwrap();
    let before = h.app.cfg.custom_games[h.app.index_of(&focus).unwrap()].status;
    h.key(egui::Key::F);
    h.idle(2);
    assert_eq!(h.app.cfg.custom_games[h.app.index_of(&focus).unwrap()].status, before.next());
    h.shot("o7-couch-status.png");
    h.key(egui::Key::Escape);
    h.key(egui::Key::Escape);
    let _ = Primary;
}

/// Couch mode, driven with the keys a controller sends:
/// SHOTS=dir cargo test -- --ignored couch_shots --test-threads=1
#[test]
#[ignore]
fn couch_shots() {
    let mut h = Harness::new("couch");
    for lang in ["en", "ar"] {
        h.app.cfg.settings.language = lang.into();
        h.page(Page::Library);
        h.key(egui::Key::F11);
        assert!(h.app.couch.is_some(), "F11 / Guide enters couch mode");
        h.idle(8);
        h.shot(&format!("c1-home-{lang}.png"));
        let first = h.app.couch.as_ref().unwrap().focus.clone();
        // Right moves forward in English; Left does in Arabic.
        h.key(if lang == "ar" { egui::Key::ArrowLeft } else { egui::Key::ArrowRight });
        h.frame_dt(0.15, vec![], Some(&format!("c2-moving-{lang}.png")));
        h.idle(8);
        assert_ne!(h.app.couch.as_ref().unwrap().focus, first, "the focus moved");
        h.shot(&format!("c3-second-{lang}.png"));
        h.key(egui::Key::I);
        h.idle(4);
        assert!(h.app.couch.as_ref().unwrap().details);
        h.shot(&format!("c4-details-{lang}.png"));
        h.key(egui::Key::Escape);
        assert!(h.app.couch.as_ref().is_some_and(|c| !c.details), "B closes details first");
        h.key(egui::Key::CloseBracket);
        h.idle(6);
        assert!(h.app.lib_filter != LibFilter::All);
        h.shot(&format!("c5-filter-{lang}.png"));
        h.app.lib_filter = LibFilter::All;
        h.key(egui::Key::Escape);
        assert!(h.app.couch.is_none(), "B leaves couch mode");
    }
}

/// Library hover and click animations, frame by frame:
/// SHOTS=dir cargo test -- --ignored hover_shots --test-threads=1
#[test]
#[ignore]
fn hover_shots() {
    let mut h = Harness::new("hover");
    h.app.cfg.custom_games.retain(|g| g.cover.is_empty() || Path::new(&g.cover).is_file());
    h.page(Page::Library);
    // Tiles sit below the "Jump back in" banner when something was played.
    let banner = h.app.stats.values().any(|s| s.last_played > 0);
    let top = if banner { 338.0 } else { 172.0 };
    let tile = egui::Rect::from_min_size(egui::pos2(260.0, top), egui::vec2(tile_w(), tile_h()));
    // Pointer near the top-left: the light and the art follow it.
    h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.min + egui::vec2(30.0, 40.0))], None);
    h.frame_dt(0.05, vec![], Some("h1-hover-start.png"));
    h.idle(6);
    h.shot("h2-hover-corner.png");
    // On the play button: it swells and gets a halo.
    h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.center())], None);
    h.idle(6);
    h.shot("h3-over-play.png");
    // Held down: the poster squashes.
    h.frame_dt(0.05, vec![egui::Event::PointerButton { pos: tile.center() + egui::vec2(0.0, 70.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }], None);
    h.frame_dt(0.05, vec![egui::Event::PointerMoved(tile.center() + egui::vec2(0.0, 70.0))], None);
    h.frame_dt(0.05, vec![], Some("h4-pressed.png"));
    h.frame_dt(0.05, vec![egui::Event::PointerMoved(egui::pos2(1230.0, 850.0))], None);
    h.idle(8);
    // Play pressed: rings burst from the button (shown without launching anything).
    let id = h.app.library_order().first().map(|&i| h.app.cfg.custom_games[i].id.clone()).unwrap();
    h.app.motion.ripple.insert(id, h.app.now);
    h.frame_dt(0.2, vec![], Some("h5-ripple.png"));
}

/// Arabic menus, dropdowns, dialogs and the remaining pages:
/// SHOTS=dir cargo test -- --ignored arabic_popups --test-threads=1
#[test]
#[ignore]
fn arabic_popups() {
    use egui::PointerButton::{Primary, Secondary};
    let mut h = Harness::new("arabic-popups");
    h.app.cfg.settings.language = "ar".into();
    h.page(Page::Library);
    h.click(egui::pos2(900.0, 450.0), Secondary);
    h.shot("p1-tile-menu.png");
    h.key(egui::Key::Escape);
    h.click(egui::pos2(212.0, 119.0), Primary);
    h.shot("p2-sort-dropdown.png");
    h.key(egui::Key::Escape);
    h.click(egui::pos2(500.0, 600.0), Secondary);
    h.shot("p3-library-menu.png");
    h.key(egui::Key::Escape);
    h.idle(2);
    h.hover(egui::pos2(100.0, 119.0));
    h.shot("p4-tooltip.png");
    // Typing Arabic into the search box: reads right to left, edits as typed.
    h.click(egui::pos2(347.0, 52.0), Primary);
    h.frame_dt(0.1, vec![egui::Event::Text("لعبة الحرب".into())], None);
    // Straight away, while the caret is lit (it blinks).
    h.frame_dt(0.02, vec![], Some("p4b-search-arabic.png"));
    assert_eq!(h.app.search, "لعبة الحرب");
    // Backspace removes the last letter typed (the leftmost one on screen).
    h.key(egui::Key::Backspace);
    assert_eq!(h.app.search, "لعبة الحر");
    // A click on the far left puts the caret at the end; typing continues there.
    h.click(egui::pos2(228.0, 52.0), Primary);
    h.frame_dt(0.1, vec![egui::Event::Text("ب 2".into())], None);
    assert_eq!(h.app.search, "لعبة الحرب 2");
    // The status bar draws the same text through the label path, for comparison.
    h.app.status = "لعبة الحرب 2".into();
    h.shot("p4c-search-mixed.png");
    h.app.status.clear();
    h.app.search.clear();
    h.key(egui::Key::Escape);
    h.idle(2);
    h.app.add_modal = Some(h.app.new_add_modal(PathBuf::from("/home/u/Games/Hollow Knight/hollow_knight.exe")));
    h.shot("p5-add.png");
    h.app.add_modal = None;
    h.page(Page::Settings);
    h.scroll(-1600.0);
    h.shot("p6-settings-lower.png");
    h.scroll(-1600.0);
    h.shot("p7-settings-bottom.png");
    h.page(Page::Dependencies);
    h.shot("p8-deps.png");
    h.app.selected = h.app.cfg.custom_games.first().map(|g| g.id.clone());
    h.page(Page::Game);
    h.click(egui::pos2(92.0, 352.0), Primary);
    h.shot("p9-more-menu.png");
    h.key(egui::Key::Escape);
    h.idle(2);
    assert!(h.app.page == Page::Game, "Escape closing a menu also left the page");
    h.app.game_tab = GameTab::Settings;
    h.idle(2);
    h.scroll(-400.0);
    h.shot("p10-game-settings.png");
    h.scroll(-600.0);
    h.shot("p11-game-settings-2.png");
    // The "Advanced" header sits on the reading side and opens on click.
    h.click(egui::pos2(935.0, 741.0), Primary);
    h.scroll(-600.0);
    h.shot("p11b-advanced-open.png");
    let log = data_dir().join("logs/test.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "info: start\n0024:err:module:import_dll Library VCRUNTIME140.dll (needed by game.exe) not found\n").unwrap();
    let (lines, hints) = extras::read_log(&log);
    h.app.extras.log = Some(extras::LogView { title: "Test".into(), path: log, lines, hints, errors_only: false });
    h.shot("p12-log.png");
    h.app.extras.log = None;
    let games = PathBuf::from(std::env::var("GAMES").unwrap_or_default());
    if games.is_dir() {
        h.app.page = Page::Library;
        h.app.scan.results = Some(extras::scan_folder(&games, &[]));
        h.app.scan.root = games;
        h.shot("p13-scan.png");
    }
    h.app.scan.results = None;
    let i = h.app.selected_index().unwrap();
    h.app.open_artwork(&h.ctx.clone(), i);
    h.idle(4);
    h.shot("p14-artwork.png");
    i18n::set_lang(i18n::Lang::En);
}

/// Game page tabs, gamescope, the log viewer and a folder scan:
/// SHOTS=dir cargo test -- --ignored extras_shots --test-threads=1
#[test]
#[ignore]
fn extras_shots() {
    let mut h = Harness::new("extras");
    let id = h.app.cfg.custom_games.iter().find(|g| g.kind == GameKind::Windows).map(|g| g.id.clone()).unwrap();
    h.app.selected = Some(id.clone());
    let i = h.app.index_of(&id).unwrap();
    h.app.cfg.custom_games[i].flags.gamescope = true;
    h.app.cfg.custom_games[i].env = "DXVK_HUD=fps".into();
    h.app.stats.insert(id.clone(), GameStats { playtime_secs: 4000, last_played: unix_now() - 3600, last_session_secs: 4, last_exit: Some(1), sessions: 3 });
    h.app.cfg.settings.no_hltb = true;
    h.page(Page::Game);
    h.scroll(-330.0);
    // Give the disk-use measurement a moment.
    for _ in 0..50 {
        if h.app.extras.disk.lock().unwrap().get(&id).is_some_and(|v| v.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    h.shot("e1-overview.png");
    h.app.game_tab = GameTab::Settings;
    h.idle(2);
    h.scroll(-330.0);
    h.shot("e2-settings-gamescope.png");
    h.scroll(-500.0);
    h.shot("e3-settings-env.png");
    h.app.game_tab = GameTab::Mods;
    h.scroll(3000.0);
    h.scroll(-600.0);
    h.shot("e4-mods.png");
    // A log with a known problem.
    let log = data_dir().join("logs/test.log");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, "info: start\nwarn: something slow\n0024:err:module:import_dll Library VCRUNTIME140.dll (needed by game.exe) not found\nwine: Unhandled page fault on read access\n").unwrap();
    let (lines, hints) = extras::read_log(&log);
    h.app.extras.log = Some(extras::LogView { title: "Test".into(), path: log, lines, hints, errors_only: false });
    h.shot("e5-log.png");
    h.app.extras.log = None;
    // Favourites lead the grid, under the "Jump back in" banner.
    for g in h.app.cfg.custom_games.iter_mut().skip(3).take(2) {
        g.favorite = true;
    }
    h.page(Page::Library);
    h.shot("e7-library-favorites.png");
    h.app.lib_filter = LibFilter::Favorites;
    h.idle(2);
    h.shot("e8-favorites-only.png");
    h.app.lib_filter = LibFilter::All;
    for size in ["small", "large"] {
        h.app.cfg.settings.tile_size = size.into();
        h.idle(6);
        h.shot(&format!("e12-tiles-{size}.png"));
    }
    h.app.cfg.settings.tile_size = String::new();
    h.idle(6);
    // Arrow keys move a focus ring; Enter opens that game.
    for k in [egui::Key::ArrowRight, egui::Key::ArrowRight, egui::Key::ArrowRight, egui::Key::ArrowDown] {
        h.key(k);
    }
    h.shot("e11-keys.png");
    let order = h.app.library_order();
    let want = h.app.cfg.custom_games[order[7]].id.clone();
    assert_eq!(h.app.lib_cursor.as_deref(), Some(want.as_str()));
    h.key(egui::Key::Enter);
    h.idle(2);
    assert!(h.app.page == Page::Game && h.app.selected.as_deref() == Some(want.as_str()));
    // ] and [ (a controller's RB/LB) step through the game page's tabs.
    h.app.game_tab = GameTab::Overview;
    h.key(egui::Key::CloseBracket);
    assert!(h.app.game_tab == GameTab::Settings);
    h.key(egui::Key::OpenBracket);
    h.key(egui::Key::OpenBracket);
    let last = if h.app.cfg.custom_games[h.app.selected_index().unwrap()].kind == GameKind::Windows { GameTab::Mods } else { GameTab::Settings };
    assert!(h.app.game_tab == last, "[ wraps around to the last tab");
    h.page(Page::Library);
    h.app.cfg.settings.language = "ar".into();
    h.idle(2);
    h.shot("e9-library-ar.png");
    h.page(Page::Game);
    h.scroll(-330.0);
    h.shot("e10-overview-ar.png");
    h.app.cfg.settings.language = "en".into();
    let games = PathBuf::from(std::env::var("GAMES").unwrap_or_default());
    if games.is_dir() {
        h.app.page = Page::Library;
        h.app.scan.results = Some(extras::scan_folder(&games, &[]));
        h.app.scan.root = games;
        h.shot("e6-scan.png");
    }
}

fn temp_file(name: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("plaunch-tmp-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, contents).unwrap();
    p
}

/// A Windows game with a fake exe, Proton dir and umu-run on disk.
fn windows_game(umu_id: &str) -> (CustomGame, Settings) {
    let exe = temp_file("game.exe", "");
    let proton_dir =
        std::env::temp_dir().join(format!("plaunch-tmp-{}/fake-proton", std::process::id()));
    fs::create_dir_all(&proton_dir).unwrap();
    let umu = temp_file("umu-run", "");
    fs::set_permissions(&umu, fs::Permissions::from_mode(0o755)).unwrap();

    let g = CustomGame {
        name: "Test Game".into(),
        kind: GameKind::Windows,
        exe: exe.to_string_lossy().into(),
        args: "-windowed".into(),
        proton_dir: proton_dir.to_string_lossy().into(),
        flags: GameFlags {
            mangohud: true,
            fsr4: Fsr4Mode::Upgrade,
            ..Default::default()
        },
        umu_id: umu_id.into(),
        ..Default::default()
    };
    let settings = Settings {
        umu_bin: umu.to_string_lossy().into(),
        ..Default::default()
    };
    (g, settings)
}

#[test]
fn windows_command_uses_umu() {
    let (g, settings) = windows_game("");
    let cmd = launch_command_string(&g, &settings).unwrap();
    assert!(cmd.contains("PROTON_FSR4_UPGRADE=1"));
    assert!(cmd.contains("MANGOHUD=1"));
    assert!(cmd.contains("WINEPREFIX="));
    assert!(cmd.contains("PROTONPATH="));
    assert!(cmd.contains("GAMEID=umu-test_game"));
    assert!(cmd.contains("umu-run"));
    assert!(cmd.ends_with("game.exe -windowed"));
}

#[test]
fn openxr_bridge_is_off_unless_vr_is_allowed() {
    let (mut g, mut s) = windows_game("");
    g.env = "WINEDLLOVERRIDES=dinput8=n,b".into();
    let ov = |s: &Settings| build_launch(&g, s).unwrap().envs.into_iter().find(|(k, _)| k == "WINEDLLOVERRIDES").map(|(_, v)| v);
    // Off by default, and the user's own overrides are added to it.
    assert_eq!(ov(&s).as_deref(), Some("wineopenxr=d;dinput8=n,b"));
    s.allow_openxr = true;
    assert_eq!(ov(&s).as_deref(), Some("dinput8=n,b"));
}

#[test]
fn tuning_becomes_env() {
    let t = |fps: u32, overlay: &str| Tuning { fps_limit: fps, overlay: overlay.into(), position: "top-right".into(), custom: "fps,\n gpu_temp".into() };
    // No overlay preset: DXVK/VKD3D cap, MangoHud's own config untouched.
    let e = tuning_envs(&t(60, ""), true);
    assert!(e.contains(&("DXVK_FRAME_RATE".into(), "60".into())) && !e.iter().any(|(k, _)| k == "MANGOHUD_CONFIG"));
    // A preset with MangoHud on: everything in MANGOHUD_CONFIG.
    let e = tuning_envs(&t(90, "minimal"), true);
    assert_eq!(e, vec![("MANGOHUD_CONFIG".to_string(), "fps_only,position=top-right,fps_limit=90".to_string())]);
    assert_eq!(tuning_envs(&t(0, "custom"), true), vec![("MANGOHUD_CONFIG".to_string(), "fps,gpu_temp,position=top-right".to_string())]);
    // MangoHud off: presets don't apply, the cap still does.
    assert_eq!(tuning_envs(&t(30, "full"), false).len(), 2);
    assert!(tuning_envs(&t(0, ""), true).is_empty());
}

#[test]
fn gamescope_wraps_and_env_merges() {
    let (mut g, settings) = windows_game("");
    g.flags.gamescope = true;
    g.gamescope_opts = GamescopeOpts { width: 2560, height: 1440, refresh: 144, fullscreen: true, filter: "fsr".into(), ..Default::default() };
    g.env = "# comment\nDXVK_HUD=fps\nexport WINEDLLOVERRIDES=\"xinput1_3=n,b\"\nbad line\n".into();
    let l = build_launch(&g, &settings).unwrap();
    assert_eq!(l.program, "gamescope");
    let a = l.args.join(" ");
    assert!(a.starts_with("-W 2560 -H 1440 -r 144 -f -F fsr --mangoapp -- "), "{a}");
    assert!(a.contains("umu-run"));
    assert!(!l.envs.iter().any(|(k, _)| k == "MANGOHUD"), "mangoapp replaces MANGOHUD");
    assert!(l.envs.contains(&("DXVK_HUD".into(), "fps".into())));
    assert!(l.envs.contains(&("WINEDLLOVERRIDES".into(), "wineopenxr=d;xinput1_3=n,b".into())));
    assert_eq!(parse_env("A=1\n=2\nB C=3\n"), vec![("A".to_string(), "1".to_string())]);
}

#[test]
fn custom_umu_id_is_used() {
    let (g, settings) = windows_game("  umu-1091500 ");
    let cmd = launch_command_string(&g, &settings).unwrap();
    assert!(cmd.contains("GAMEID=umu-1091500 "));
    assert!(!cmd.contains("umu-test_game"));
}

#[test]
fn linux_command_is_direct() {
    let exe = temp_file("native-run", "");
    let g = CustomGame {
        name: "Native".into(),
        kind: GameKind::Linux,
        exe: exe.to_string_lossy().into(),
        flags: GameFlags {
            mangohud: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let cmd = launch_command_string(&g, &Settings::default()).unwrap();
    assert!(!cmd.contains("GAMEID"));
    assert!(!cmd.contains("PROTONPATH"));
    assert!(cmd.contains("mangohud "));
    assert!(cmd.contains("native-run"));
}

/// How long start-up takes on this machine:
/// cargo test --release -- --ignored startup_time --nocapture
#[test]
#[ignore]
fn startup_time() {
    let t = Instant::now();
    let steam = find_steam_root();
    eprintln!("steam root: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = detect_proton_tools(steam.as_deref());
    eprintln!("proton tools: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = load_config();
    eprintln!("config: {:?}", t.elapsed());
    let t = Instant::now();
    let app = App::new(steam);
    eprintln!("App::new total: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = app.dep_rows();
    eprintln!("dep rows: {:?}", t.elapsed());
    let ctx = egui::Context::default();
    let t = Instant::now();
    configure_style(&ctx);
    eprintln!("fonts/style: {:?}", t.elapsed());
    let mut app = app;
    let t = Instant::now();
    let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))), ..Default::default() }, |ui| app.draw(ui));
    eprintln!("first frame: {:?}", t.elapsed());
    let t = Instant::now();
    let _ = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))), ..Default::default() }, |ui| app.draw(ui));
    eprintln!("second frame: {:?}", t.elapsed());
    app.cfg.settings.no_store = true;
    for page in [Page::Home, Page::Library, Page::Stats, Page::Stores, Page::Settings, Page::Emulation, Page::Launchers] {
        app.page = page;
        let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1240.0, 860.0))), ..Default::default() };
        for _ in 0..5 {
            let _ = ctx.run_ui(input(), |ui| app.draw(ui));
        }
        let t = Instant::now();
        for _ in 0..10 {
            let _ = ctx.run_ui(input(), |ui| app.draw(ui));
        }
        eprintln!("{page:?}: {:?} per frame", t.elapsed() / 10);
    }
}
