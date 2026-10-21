//! Shared UI pieces: cards, chips, buttons, fields, posters and layout helpers.

use crate::*;

/// Gamescope settings. Returns true if anything changed.
pub(crate) fn gamescope_ui(ui: &mut egui::Ui, o: &mut GamescopeOpts) -> bool {
    let before = o.clone();
    const SIZES: &[(&str, u32, u32)] = &[("Native", 0, 0), ("1280×720", 1280, 720), ("1920×1080", 1920, 1080), ("2560×1440", 2560, 1440), ("3840×2160", 3840, 2160)];
    let label = |w: u32, h: u32| {
        SIZES.iter().find(|s| s.1 == w && s.2 == h).map_or_else(|| format!("{w}×{h}"), |s| tr(s.0))
    };
    if find_in_path("gamescope").is_none() {
        ui.horizontal(|ui| {
            status_dot(ui, pal().warn, true);
            let cmd = install_command(&["gamescope"]).unwrap_or_default();
            ui.label(egui::RichText::new(trf("gamescope isn't installed — {}", &[&cmd])).color(pal().warn).small());
        });
    }
    modal_row(ui, "Output", |ui| {
        egui::ComboBox::from_id_salt("gs-out").selected_text(label(o.width, o.height)).show_ui(ui, |ui| menu_ui(ui, |ui| {
            for (n, w, h) in SIZES {
                if ui.selectable_label(o.width == *w && o.height == *h, tr(n)).clicked() {
                    (o.width, o.height) = (*w, *h);
                }
            }
        }));
        ui.label(egui::RichText::new(tr("game renders at")).color(pal().subtext).small());
        egui::ComboBox::from_id_salt("gs-game").selected_text(if o.game_width == 0 { tr("same") } else { label(o.game_width, o.game_height) }).show_ui(ui, |ui| menu_ui(ui, |ui| {
            for (n, w, h) in SIZES {
                let text = if *w == 0 { tr("same") } else { tr(n) };
                if ui.selectable_label(o.game_width == *w && o.game_height == *h, text).clicked() {
                    (o.game_width, o.game_height) = (*w, *h);
                }
            }
        }));
    });
    modal_row(ui, "Upscaling", |ui| {
        for (key, name) in [("", "None"), ("fsr", "FSR"), ("nis", "NIS"), ("pixel", "Pixel"), ("linear", "Linear")] {
            if choice_chip(ui, o.filter == key, name).clicked() {
                o.filter = key.into();
            }
        }
    });
    modal_row(ui, "Options", |ui| {
        flag_chip(ui, &mut o.fullscreen, "Fullscreen");
        flag_chip(ui, &mut o.hdr, "HDR");
        ui.label(egui::RichText::new(tr("FPS limit")).color(pal().subtext).small());
        let mut fps = if o.refresh == 0 { String::new() } else { o.refresh.to_string() };
        if ui.add(egui::TextEdit::singleline(&mut fps).hint_text("∞").desired_width(48.0)).changed() {
            o.refresh = fps.trim().parse().unwrap_or(0);
        }
    });
    modal_row(ui, "Extra options", |ui| {
        let (r, by_menu) = text_field(ui, &mut o.extra, "e.g. --adaptive-sync", f32::INFINITY);
        let _ = (r, by_menu);
    });
    *o != before
}

/// FPS limit and MangoHud overlay rows. Returns true if anything changed.
pub(crate) fn tuning_ui(ui: &mut egui::Ui, t: &mut Tuning, mangohud: bool, windows: bool) -> bool {
    let before = t.clone();
    const CAPS: [u32; 7] = [0, 30, 40, 60, 90, 120, 144];
    modal_row(ui, "FPS limit", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for cap in CAPS {
                let label = if cap == 0 { tr("Off") } else { cap.to_string() };
                if choice_chip(ui, t.fps_limit == cap, &label).clicked() {
                    t.fps_limit = cap;
                }
            }
            let custom = !CAPS.contains(&t.fps_limit);
            if choice_chip(ui, custom, "Other").clicked() && !custom {
                t.fps_limit = 75;
            }
            if custom {
                ui.add(egui::DragValue::new(&mut t.fps_limit).range(10..=500).suffix(" fps"));
            }
        });
    });
    let how = match (t.fps_limit, mangohud && !t.overlay.is_empty(), windows) {
        (0, _, _) => None,
        (_, true, _) => Some("Capped by MangoHud."),
        (_, false, true) => Some("Capped by DXVK/VKD3D (DirectX games). Pick an overlay style below to use MangoHud's limiter instead."),
        (_, false, false) => Some("Native Linux games need MangoHud on (with an overlay style) for the cap."),
    };
    if let Some(how) = how {
        note(ui, 128.0, how);
    }
    if mangohud {
        ui.add_space(4.0);
        modal_row(ui, "Overlay", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for (key, label) in [("", "Your MangoHud settings"), ("minimal", "Minimal"), ("standard", "Standard"), ("full", "Full"), ("custom", "Custom")] {
                    if choice_chip(ui, t.overlay == key, label).clicked() {
                        t.overlay = key.into();
                    }
                }
            });
        });
        if !t.overlay.is_empty() {
            modal_row(ui, "Position", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for (key, label) in [("", "Default"), ("top-left", "Top left"), ("top-right", "Top right"), ("bottom-left", "Bottom left"), ("bottom-right", "Bottom right")] {
                        if choice_chip(ui, t.position == key, label).clicked() {
                            t.position = key.into();
                        }
                    }
                });
            });
        }
        if t.overlay == "custom" {
            modal_row(ui, "Options", |ui| {
                text_field(ui, &mut t.custom, "e.g. fps,gpu_temp,cpu_temp,vram", f32::INFINITY);
            });
        }
    }
    *t != before
}

/// The performance toggles. Returns true if any of them changed.
pub(crate) fn flags_ui(ui: &mut egui::Ui, f: &mut GameFlags) -> bool {
    let before = *f;
    ui.horizontal_wrapped(|ui| {
        // Chips move to the next row rather than squeezing their text.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        flag_chip(ui, &mut f.mangohud, "MangoHud");
        flag_chip(ui, &mut f.ntsync, "NTSync");
        flag_chip(ui, &mut f.wayland, "Wayland");
        flag_chip(ui, &mut f.gamemode, "GameMode");
        flag_chip(ui, &mut f.gamescope, "Gamescope");
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("fsr4")
            .selected_text(tr(f.fsr4.label()))
            .show_ui(ui, |ui| menu_ui(ui, |ui| {
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Off, tr("Off"));
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Upgrade, tr("Upgrade"));
                ui.selectable_value(&mut f.fsr4, Fsr4Mode::Rrna3, tr("RDNA3 upgrade"));
            }));
        ui.add_enabled_ui(f.fsr4 != Fsr4Mode::Off, |ui| {
            flag_chip(ui, &mut f.watermark, "FSR4 HUD");
        });
    });
    *f != before
}

/// The arrow of a collapsible section: pointing along the reading direction
/// when closed, down when open.
pub(crate) fn disclosure_icon(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let c = response.rect.center();
    let r = 4.5;
    let dir = if rtl() { -1.0 } else { 1.0 };
    let angle = openness * std::f32::consts::FRAC_PI_2 * dir;
    let rot = egui::emath::Rot2::from_angle(angle);
    let pts = [egui::vec2(-r * 0.6 * dir, -r), egui::vec2(-r * 0.6 * dir, r), egui::vec2(r * dir, 0.0)]
        .map(|v| c + rot * v)
        .to_vec();
    let color = ui.style().interact(response).fg_stroke.color;
    ui.painter().add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

/// A rounded card container. The accent is drawn implicitly via icons and
/// status pills so cards stay calm instead of every card shouting with a
/// coloured border.
pub(crate) fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(20, 18))
        .corner_radius(rad(16))
        .fill(pal().card)
        .stroke(egui::Stroke::new(1.0, pal().border))
}

pub(crate) fn icon_slot(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    accent: egui::Color32,
    initial: char,
) -> egui::Response {
    let size = 44.0;
    let resp = match tex {
        Some(t) => ui.add(
            egui::Image::new(egui::load::SizedTexture::from_handle(t))
                .fit_to_exact_size(egui::vec2(size, size))
                .corner_radius(rad(10))
                .sense(egui::Sense::click()),
        ),
        None => {
            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, rad(10), pal().card2);
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial.to_ascii_uppercase().to_string(),
                egui::FontId::proportional(20.0),
                accent,
            );
            resp
        }
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn load_rgba(ctx: &egui::Context, name: String, img: &image::RgbaImage) -> egui::TextureHandle {
    let (w, h) = img.dimensions();
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    ctx.load_texture(name, color, egui::TextureOptions::LINEAR)
}

/// A rounded rect filled with `tex`, cropped (not stretched) to cover `rect`.
pub(crate) fn cover_fill_shape(tex: &egui::TextureHandle, rect: egui::Rect, radius: u8, opacity: f32) -> egui::Shape {
    cover_fill_shape_ex(tex, rect, radius, opacity, 1.0, 0.5)
}

/// `cover_fill_shape`, zoomed in by `zoom` and panned vertically within the
/// spare image (`pan` 0 = top, 1 = bottom) — the banner parallax uses this.
pub(crate) fn cover_fill_shape_ex(
    tex: &egui::TextureHandle,
    rect: egui::Rect,
    radius: u8,
    opacity: f32,
    zoom: f32,
    pan: f32,
) -> egui::Shape {
    cover_fill_shape_pan(tex, rect, radius, opacity, zoom, egui::vec2(0.5, pan))
}

/// `cover_fill_shape_ex`, panned both ways (0..1 each; 0.5 = centred).
pub(crate) fn cover_fill_shape_pan(
    tex: &egui::TextureHandle,
    rect: egui::Rect,
    radius: u8,
    opacity: f32,
    zoom: f32,
    pan: egui::Vec2,
) -> egui::Shape {
    let [tw, th] = tex.size();
    let ta = tw as f32 / th.max(1) as f32;
    let ra = rect.width() / rect.height().max(1.0);
    let (w, h) = if ra > ta { (1.0, ta / ra) } else { (ra / ta, 1.0) };
    let (w, h) = (w / zoom, h / zoom);
    let x = (1.0 - w) * pan.x;
    let y = (1.0 - h) * pan.y;
    let uv = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    egui::epaint::RectShape::filled(
        rect,
        rad(radius),
        egui::Color32::WHITE.gamma_multiply(opacity),
    )
    .with_texture(tex.id(), uv)
    .into()
}

/// A soft diagonal band of light at `t` (0..1) across `rect`: a "loading" sweep.
pub(crate) fn sheen(painter: &egui::Painter, rect: egui::Rect, t: f32) {
    let w = rect.width() * 0.5;
    let x = rect.left() - w + t * (rect.width() + 2.0 * w);
    let skew = rect.height() * 0.3;
    // Nested bands give the light a soft falloff.
    for (half, alpha) in [(w * 0.5, 9u8), (w * 0.28, 12), (w * 0.1, 16)] {
        let pts = vec![
            egui::pos2(x - half + skew, rect.top()),
            egui::pos2(x + half + skew, rect.top()),
            egui::pos2(x + half - skew, rect.bottom()),
            egui::pos2(x - half - skew, rect.bottom()),
        ];
        painter.add(egui::Shape::convex_polygon(pts, egui::Color32::from_white_alpha(alpha), egui::Stroke::NONE));
    }
}

/// A partial ring rotated to `angle`, over a faint full ring: a quiet "working" mark.
pub(crate) fn spinner_arc(painter: &egui::Painter, c: egui::Pos2, r: f32, angle: f32, color: egui::Color32) {
    painter.circle_stroke(c, r, egui::Stroke::new(3.0, color.gamma_multiply(0.2)));
    let pts: Vec<egui::Pos2> = (0..=24)
        .map(|k| {
            let a = angle + k as f32 / 24.0 * 1.8;
            c + r * egui::vec2(a.cos(), a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(pts, egui::Stroke::new(3.0, color)));
}

/// A clickable cover poster.
pub(crate) fn cover_slot(ui: &mut egui::Ui, tex: &egui::TextureHandle, size: egui::Vec2) -> egui::Response {
    ui.add(
        egui::Image::new(egui::load::SizedTexture::from_handle(tex))
            .fit_to_exact_size(size)
            .corner_radius(rad(8))
            .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// An empty dashed-looking slot where a cover would go.
pub(crate) fn cover_placeholder(ui: &mut egui::Ui, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, rad(8), pal().card2);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        tr("none"),
        egui::FontId::proportional(11.0),
        pal().faint,
    );
}

/// Centered, muted placeholder shown when a list is empty.
pub(crate) fn empty_hint(ui: &mut egui::Ui, text: &str) {
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 30))
        .corner_radius(rad(12))
        .fill(pal().card.gamma_multiply(0.45))
        .stroke(egui::Stroke::new(1.0, pal().border));
    frame.show(ui, |ui| {
        ui.vertical_centered(|ui| {
            para(ui, text, pal().subtext, false);
        });
    });
}

/// The composed launch command, shown like a code block.
pub(crate) fn command_preview(ui: &mut egui::Ui, cmd: &str) {
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 9))
        .corner_radius(rad(9))
        .fill(pal().field)
        .stroke(egui::Stroke::new(1.0, pal().border));
    frame.show(ui, |ui| {
        ui.add(
            egui::Label::new(egui::RichText::new(cmd).monospace().small().color(pal().subtext))
                .wrap()
                .sense(egui::Sense::click()),
        )
        .context_menu(|ui| menu_ui(ui, |ui| {
            if ui.button(tr("Copy")).clicked() {
                ui.ctx().copy_text(cmd.to_string());
                ui.close();
            }
        }));
    });
}

/// A small rounded outline badge. Painted at its own size, so it never
/// stretches to the height of the row it sits in.
pub(crate) fn pill(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(tr(text), font, color);
    let size = galley.size() + egui::vec2(18.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, rad(255), color.gamma_multiply(0.12));
    p.rect_stroke(rect, rad(255), egui::Stroke::new(1.0, color.gamma_multiply(0.40)), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// A small painted status dot (filled when ok, hollow otherwise).
/// Drawn with the painter so it never depends on font glyph coverage.
pub(crate) fn status_dot(ui: &mut egui::Ui, color: egui::Color32, filled: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let c = rect.center();
    if filled {
        ui.painter().circle_filled(c, 5.0, color);
    } else {
        ui.painter()
            .circle_stroke(c, 5.0, egui::Stroke::new(1.6, color));
    }
}

/// A toggle chip used for launch flags.
pub(crate) fn flag_chip(ui: &mut egui::Ui, on: &mut bool, label: &str) {
    if choice_chip(ui, *on, label).clicked() {
        *on = !*on;
    }
}

/// A pill-shaped button drawn filled when `on`.
pub(crate) fn choice_chip(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    // Colours cross-fade when toggled.
    let id = ui.make_persistent_id(("chip", label));
    let t = ui.ctx().animate_bool_with_time(id, on, ui.style().animation_time * 1.5);
    let (fill, fg, stroke) = (
        pal().card2.lerp_to_gamma(pal().accent, t),
        pal().subtext.lerp_to_gamma(pal().on_accent, t),
        pal().border.lerp_to_gamma(pal().accent, t),
    );
    let fill = if t > 0.0 { fill } else { pal().card };
    let resp = ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(fg).family(semibold_family()).size(14.0))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(rad(255))
            .min_size(egui::vec2(0.0, 36.0)),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quieter card nested inside a section card.
pub(crate) fn card_inner() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 10))
        .corner_radius(rad(10))
        .fill(pal().card2.gamma_multiply(if pal().dark { 0.6 } else { 0.7 }))
        .stroke(egui::Stroke::new(1.0, pal().border.gamma_multiply(0.7)))
}

/// A filled red button for destructive confirmations.
pub(crate) fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(pal().on_accent).family(semibold_family()))
            .fill(pal().danger)
            .stroke(egui::Stroke::new(1.0, pal().danger))
            .corner_radius(rad(10)),
    )
}

/// A theme preview tile for Settings: a tiny window in the theme's colours.
pub(crate) fn theme_tile(ui: &mut egui::Ui, t: &theme::ThemeDef, selected: bool) -> egui::Response {
    let size = egui::vec2(100.0, 76.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let [bg, card, accent, text] = t.swatch();
    let p = ui.painter();
    let r = rad(8);
    let preview = egui::Rect::from_min_size(rect.min, egui::vec2(size.x, 54.0));
    p.rect_filled(preview, r, bg);
    // A sidebar strip, a card with two text lines and an accent button.
    let side = egui::Rect::from_min_size(preview.min + egui::vec2(6.0, 6.0), egui::vec2(18.0, 42.0));
    p.rect_filled(side, rad(4), card);
    let cardr = egui::Rect::from_min_max(preview.min + egui::vec2(30.0, 6.0), preview.max - egui::vec2(6.0, 6.0));
    p.rect_filled(cardr, rad(4), card);
    p.rect_filled(
        egui::Rect::from_min_size(cardr.min + egui::vec2(6.0, 7.0), egui::vec2(40.0, 4.0)),
        rad(2),
        text.gamma_multiply(0.85),
    );
    p.rect_filled(
        egui::Rect::from_min_size(cardr.min + egui::vec2(6.0, 15.0), egui::vec2(28.0, 3.0)),
        rad(2),
        text.gamma_multiply(0.45),
    );
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(cardr.right() - 26.0, cardr.bottom() - 13.0), egui::vec2(20.0, 8.0)),
        rad(3),
        accent,
    );
    let hovered = resp.hovered();
    let stroke = if selected {
        egui::Stroke::new(2.0, pal().accent)
    } else if hovered {
        egui::Stroke::new(1.0, pal().subtext)
    } else {
        egui::Stroke::new(1.0, pal().border)
    };
    p.rect_stroke(preview, r, stroke, egui::StrokeKind::Outside);
    p.text(
        egui::pos2(rect.center().x, preview.bottom() + 6.0),
        egui::Align2::CENTER_TOP,
        tr(t.name),
        egui::FontId::proportional(12.5),
        if selected { pal().text } else { pal().subtext },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A round (or, with square corners, square) colour swatch.
pub(crate) fn accent_swatch(ui: &mut egui::Ui, color: egui::Color32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let inner = egui::Rect::from_center_size(c, egui::vec2(20.0, 20.0));
    if pal().square {
        p.rect_filled(inner, 0, color);
    } else {
        p.circle_filled(c, 10.0, color);
    }
    if selected || resp.hovered() {
        let stroke = egui::Stroke::new(2.0, if selected { pal().text } else { pal().subtext });
        if pal().square {
            p.rect_stroke(inner.expand(3.0), 0, stroke, egui::StrokeKind::Outside);
        } else {
            p.circle_stroke(c, 13.0, stroke);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Accent-filled primary action button.
pub(crate) fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).color(pal().on_accent).family(semibold_family()))
            .fill(pal().accent)
            .stroke(egui::Stroke::new(1.0, pal().accent))
            .corner_radius(rad(10)),
    )
}

/// What sits before a sidebar entry's label.
#[derive(Clone, Copy)]
pub(crate) enum NavMark {
    None,
    Icon(Glyph),
    Dot(egui::Color32),
}

/// A full-width sidebar entry: main pages are taller with an icon, sources
/// and tools smaller. Returns the click response.
pub(crate) fn nav_item(ui: &mut egui::Ui, selected: bool, mark: NavMark, label: &str, count: Option<usize>, big: bool) -> egui::Response {
    let height = if big { 42.0 } else { 36.0 };
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    // The selected highlight is drawn (and slid) by the sidebar itself.
    if hovered && !selected {
        p.rect_filled(rect, rad(10), pal().card);
    }
    let fg = if selected { pal().accent } else if hovered { pal().text } else { pal().subtext };
    let x = |off: f32| if rtl() { rect.right() - off } else { rect.left() + off };
    let mut text_off = 12.0;
    match mark {
        NavMark::Icon(g) => {
            glyph(p, g, egui::Rect::from_center_size(egui::pos2(x(21.0), rect.center().y), egui::vec2(18.0, 18.0)), fg);
            text_off = 42.0;
        }
        NavMark::Dot(c) => {
            p.circle_filled(egui::pos2(x(16.0), rect.center().y), 4.0, c);
            text_off = 32.0;
        }
        NavMark::None => {}
    }
    let (start_align, end, end_align) = if rtl() {
        (egui::Align2::RIGHT_CENTER, rect.left() + 12.0, egui::Align2::LEFT_CENTER)
    } else {
        (egui::Align2::LEFT_CENTER, rect.right() - 12.0, egui::Align2::RIGHT_CENTER)
    };
    let family = if selected { semibold_family() } else { egui::FontFamily::Proportional };
    let font = egui::FontId::new(if big { 15.0 } else { 14.0 }, family);
    p.text(egui::pos2(x(text_off), rect.center().y), start_align, tr(label), font, fg);
    if let Some(c) = count {
        p.text(egui::pos2(end, rect.center().y), end_align, c.to_string(), egui::FontId::proportional(12.0), pal().faint);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small upper-case label over a group of sidebar entries.
pub(crate) fn nav_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let label = tr(text);
        let label = if rtl() { label } else { label.to_uppercase() };
        ui.label(egui::RichText::new(label).family(semibold_family()).size(11.5).color(pal().faint));
    });
    ui.add_space(2.0);
}

/// A compact status line for the sidebar footer: dot + label + value.
pub(crate) fn status_line(ui: &mut egui::Ui, warn: bool, label: &str, value: &str) {
    ui.horizontal(|ui| {
        status_dot(ui, if warn { pal().warn } else { pal().ok }, true);
        ui.label(egui::RichText::new(tr(label)).color(pal().subtext).small());
        ui.with_layout(far_end(), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(tr(value)).color(pal().text).small()).truncate());
        });
    });
}

/// A labelled form row used inside the modal dialogs.
pub(crate) fn modal_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(120.0, 22.0),
            egui::Label::new(egui::RichText::new(tr(label)).color(pal().subtext))
                .wrap_mode(egui::TextWrapMode::Extend),
        );
        add(ui);
    });
    ui.add_space(6.0);
}

/// Room a button labelled `label` takes in a row, spacing included (labels
/// differ in length between languages).
pub(crate) fn button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = ui.painter().layout_no_wrap(tr(label), font, egui::Color32::WHITE).size().x;
    text + 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x + 2.0
}

/// A menu, dropdown or dialog is open: Escape, Enter and the arrow keys
/// belong to it, not to the page underneath.
pub(crate) fn overlay_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_any_open(ctx) || ctx.memory(|m| m.top_modal_layer().is_some())
}

/// "‹ label" button; also fires on Escape when no text field has focus.
pub(crate) fn back_button(ui: &mut egui::Ui, ctx: &egui::Context, label: &str) -> bool {
    let no_focus = ctx.memory(|m| m.focused().is_none()) && !overlay_open(ctx);
    let text = if rtl() { format!("{}  ›", tr(label)) } else { format!("‹  {}", tr(label)) };
    ui.button(text).clicked()
        || (no_focus && ctx.input(|i| i.key_pressed(egui::Key::Escape)))
}

/// A page title with a muted subtitle underneath.
pub(crate) fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(title_text(tr(title), 30.0));
    para(ui, subtitle, pal().subtext, true);
    ui.add_space(14.0);
}

/// A titled settings card on the game page.
pub(crate) fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tr(title)).family(semibold_family()).size(16.0).color(pal().text));
        ui.add_space(10.0);
        add(ui);
    });
}

/// A titled settings card with a one-line description.
pub(crate) fn section_sub(ui: &mut egui::Ui, title: &str, sub: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(tr(title)).family(semibold_family()).size(16.0).color(pal().text));
        para(ui, sub, pal().subtext, true);
        ui.add_space(12.0);
        add(ui);
    });
}

/// A form row with buttons on the right: `buttons` is laid out right-to-left
/// first, then `field` fills exactly the width that's left.
pub(crate) fn row_with_buttons(
    ui: &mut egui::Ui,
    label: &str,
    buttons: impl FnOnce(&mut egui::Ui),
    field: impl FnOnce(&mut egui::Ui),
) {
    modal_row(ui, label, |ui| {
        ui.with_layout(far_end(), |ui| {
            buttons(ui);
            ui.with_layout(near_start(), field);
        });
    });
}

/// A small muted note lined up under a form row's field.
pub(crate) fn note(ui: &mut egui::Ui, indent: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        vcol(ui, |ui| para(ui, text, pal().subtext, true));
    });
}

/// Wrapped text that also reads right-to-left: Arabic is broken into lines
/// in reading order first, then each line is shaped (egui would otherwise
/// wrap the already-reversed text and the lines would read bottom-up).
pub(crate) fn para(ui: &mut egui::Ui, text: &str, color: egui::Color32, small: bool) -> egui::Response {
    let rich = |t: String| {
        let r = egui::RichText::new(t).color(color);
        if small { r.small() } else { r }
    };
    if let Some(logical) = i18n::for_wrap(text).filter(|l| shaping::needs_shaping(l)) {
        let style = if small { egui::TextStyle::Small } else { egui::TextStyle::Body };
        let font = style.resolve(ui.style());
        let width = ui.available_width().max(60.0);
        let wrapped = ui.ctx().fonts_mut(|f| {
            shaping::wrap_visual(&logical, width, rtl(), |s| f.layout_no_wrap(s.to_string(), font.clone(), color).size().x)
        });
        ui.add(egui::Label::new(rich(wrapped)).wrap_mode(egui::TextWrapMode::Extend))
    } else {
        ui.add(egui::Label::new(rich(tr(text))).wrap())
    }
}

/// A checkbox; in right-to-left layouts the box sits to the right of its text.
pub(crate) fn check(ui: &mut egui::Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    if !rtl() {
        return ui.checkbox(on, text);
    }
    ui.horizontal(|ui| {
        let mut boxed = ui.add(egui::Checkbox::without_text(on));
        let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
        if label.clicked() {
            *on = !*on;
            boxed.mark_changed();
        }
        boxed.union(label)
    })
    .inner
}

/// `ui.vertical`, right-aligned when the interface is right-to-left.
pub(crate) fn vcol<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if rtl() {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), add)
    } else {
        ui.vertical(add)
    }
}

/// Layout for things pushed to the far end of a row (right, or left in RTL).
pub(crate) fn far_end() -> egui::Layout {
    if rtl() {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::right_to_left(egui::Align::Center)
    }
}

/// Layout that flows from the start of a row (left, or right in RTL).
pub(crate) fn near_start() -> egui::Layout {
    if rtl() {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    }
}

/// Run `add` in a top-down layout that runs right-to-left for Arabic, so the
/// rows inside mirror automatically.
pub(crate) fn dir_ui<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // A modal's first (sizing) pass measures left-aligned, as `menu_ui` explains.
    if rtl() && !ui.is_sizing_pass() {
        ui.with_layout(egui::Layout::top_down(egui::Align::Max), add).inner
    } else {
        add(ui)
    }
}

/// Menu and dropdown contents, right-aligned in a right-to-left interface
/// (egui lays popups out left to right whatever the language). Popups
/// measure their content in a first "sizing pass", where right-aligned items
/// would claim the whole screen width, so that pass stays left-aligned.
pub(crate) fn menu_ui<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if rtl() && !ui.is_sizing_pass() {
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Max), add).inner
    } else {
        add(ui)
    }
}

/// A labelled text field. Returns true when editing ends (Enter or click away).
pub(crate) fn edit_row(ui: &mut egui::Ui, label: &str, buf: &mut String, hint: &str) -> bool {
    let mut done = false;
    modal_row(ui, label, |ui| {
        let (r, by_menu) = text_field(ui, buf, hint, f32::INFINITY);
        done = r.lost_focus() || by_menu;
    });
    done
}

/// A single-line text field with a Cut / Copy / Paste / Select all / Clear menu
/// on right-click. Returns its response and whether the menu changed the text.
pub(crate) fn text_field(ui: &mut egui::Ui, buf: &mut String, hint: &str, width: f32) -> (egui::Response, bool) {
    // Arabic reads from the right: lay it out in reading order (see rtl_edit).
    let style = rtl_edit::FieldStyle {
        font: egui::TextStyle::Body.resolve(ui.style()),
        color: ui.visuals().text_color(),
        line_height: ui.fonts_mut(|f| f.row_height(&egui::TextStyle::Body.resolve(ui.style()))) + ui.spacing().extra_text_line_spacing,
    };
    let rtl_text = rtl_edit::rtl_base(buf);
    let has_rtl = shaping::needs_shaping(buf);
    let align = if rtl_text { egui::Align::Max } else { egui::Align::Min };
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap: f32| rtl_edit::layout(ui, text.as_str(), &style, align);
    let out = egui::TextEdit::singleline(buf)
        .hint_text(tr(hint))
        .desired_width(width)
        .font(style.font.clone())
        .text_color(style.color)
        .align(egui::Align2([align, egui::Align::Center]))
        .layouter(&mut layouter)
        .show(ui);
    let id = out.response.id;
    let ctx = ui.ctx().clone();
    if has_rtl {
        // egui's click-to-caret and selection highlight assume left-to-right text.
        if out.response.response.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_pressed())
            && let Some(p) = out.response.response.interact_pointer_pos()
        {
            let at = rtl_edit::cursor_at(&out.galley, buf, p.x - out.galley_pos.x);
            let mut state = out.state.clone();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
            state.store(&ctx, id);
        }
        if let Some(r) = out.cursor_range.filter(|r| !r.is_empty()) {
            let r = r.as_sorted_char_range();
            rtl_edit::paint_selection(ui, &out.galley, out.galley_pos, r.start.0..r.end.0, out.text_clip_rect);
        }
    }
    // The cursor survives losing focus, so a selection made before the
    // right-click is still known.
    let range = out
        .cursor_range
        .or_else(|| egui::TextEdit::load_state(&ctx, id).and_then(|s| s.cursor.char_range()));
    let resp = out.response.response;
    let mut changed = false;
    resp.context_menu(|ui| menu_ui(ui, |ui| {
        let byte = |text: &str, ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
        let sel = range.map(|r| {
            let r = r.as_sorted_char_range();
            (byte(buf, r.start.0), byte(buf, r.end.0))
        });
        let picked = sel.filter(|(a, b)| a < b);
        if ui.add_enabled(picked.is_some(), egui::Button::new(tr("Cut"))).clicked() {
            if let Some((a, b)) = picked {
                ctx.copy_text(buf[a..b].to_string());
                buf.replace_range(a..b, "");
                changed = true;
            }
            ui.close();
        }
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Copy"))).clicked() {
            let (a, b) = picked.unwrap_or((0, buf.len()));
            ctx.copy_text(buf[a..b].to_string());
            ui.close();
        }
        if ui.button(tr("Paste")).clicked() {
            if let Some(text) = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()) {
                let text = text.replace(['\n', '\r'], " ");
                let (a, b) = sel.unwrap_or((buf.len(), buf.len()));
                buf.replace_range(a..b, &text);
                changed = true;
            }
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Select all"))).clicked() {
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::select_all_str(buf)));
            state.store(&ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
            ui.close();
        }
        if ui.add_enabled(!buf.is_empty(), egui::Button::new(tr("Clear"))).clicked() {
            buf.clear();
            changed = true;
            ui.close();
        }
    }));
    (resp, changed)
}

/// The right-click menu for a game, shared by its poster and its page.
pub(crate) fn game_menu(ui: &mut egui::Ui, g: &CustomGame, running: bool, on_page: bool, collections: &[String], acts: &mut Acts) {
    let (id, windows, fav) = (g.id.as_str(), g.kind == GameKind::Windows, g.favorite);
    let item = |ui: &mut egui::Ui, acts: &mut Acts, label: &str, act: GameAct| {
        if ui.button(tr(label)).clicked() {
            acts.game.push((id.to_string(), act));
            ui.close();
        }
    };
    if running {
        item(ui, acts, "Stop", GameAct::Stop);
    } else {
        item(ui, acts, "Play", GameAct::Play);
    }
    if !on_page {
        item(ui, acts, "Open game page", GameAct::Open);
    }
    item(ui, acts, "Change artwork", GameAct::Artwork);
    item(ui, acts, "Saves…", GameAct::Saves);
    item(ui, acts, if fav { "Remove from favorites" } else { "Add to favorites" }, GameAct::Favorite);
    ui.menu_button(tr("Status"), |ui| menu_ui(ui, |ui| {
        for st in std::iter::once(GameStatus::None).chain(GameStatus::ALL) {
            if ui.selectable_label(g.status == st, tr(st.label())).clicked() {
                acts.game.push((id.to_string(), GameAct::SetStatus(st)));
                ui.close();
            }
        }
    }));
    ui.menu_button(tr("Collections"), |ui| menu_ui(ui, |ui| {
        for c in collections {
            let on = g.collections.contains(c);
            let mut checked = on;
            if check(ui, &mut checked, disp(c)).changed() {
                acts.game.push((id.to_string(), GameAct::ToggleCollection(c.clone())));
            }
        }
        if !collections.is_empty() {
            ui.separator();
        }
        if ui.button(tr("New collection…")).clicked() {
            acts.collection = Some(CollectionEdit { game: Some(id.to_string()), ..Default::default() });
            ui.close();
        }
    }));
    ui.separator();
    item(ui, acts, "Open game folder", GameAct::OpenGameFolder);
    if windows {
        item(ui, acts, "Open prefix folder", GameAct::OpenPrefix);
    }
    item(ui, acts, "Open log", GameAct::OpenLog);
    item(ui, acts, "Copy launch command", GameAct::CopyCommand);
    item(ui, acts, "Add to app menu", GameAct::Shortcut);
    ui.separator();
    if ui
        .button(egui::RichText::new(tr("Remove from library…")).color(pal().danger))
        .clicked()
    {
        acts.game.push((id.to_string(), GameAct::AskRemove));
        ui.close();
    }
}

/// A small pill drawn straight onto a poster. Returns the height used.
/// `glow` (0..1) brightens the dot, so it can breathe for live states.
pub(crate) fn poster_badge(painter: &egui::Painter, pos: egui::Pos2, text: &str, color: egui::Color32, glow: f32) -> f32 {
    let galley = painter.layout_no_wrap(tr(text), egui::FontId::proportional(11.5), color);
    let size = galley.size() + egui::vec2(20.0, 6.0);
    let rect = egui::Rect::from_min_size(pos, size);
    painter.rect_filled(rect, rad(255), egui::Color32::from_black_alpha(190));
    painter.rect_stroke(
        rect,
        rad(255),
        egui::Stroke::new(1.0, color.gamma_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    let dot = rect.left_center() + egui::vec2(8.0, 0.0);
    painter.circle_filled(dot, 5.0 * glow, color.gamma_multiply(0.25 * glow));
    painter.circle_filled(dot, 3.0, color.gamma_multiply(0.55 + 0.45 * glow));
    painter.galley(rect.min + egui::vec2(14.0, 3.0), galley, color);
    size.y + 4.0
}

/// A soft round light: `color` at the centre fading to nothing at `radius`.
pub(crate) fn glow_spot(painter: &egui::Painter, center: egui::Pos2, radius: f32, color: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    let n = 40;
    for k in 0..=n {
        // Offset half a step: no spoke lies exactly horizontal.
        let a = (k as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + radius * egui::vec2(a.cos(), a.sin()), egui::Color32::TRANSPARENT);
    }
    for k in 1..=n as u32 {
        mesh.add_triangle(0, k, k + 1);
    }
    painter.add(mesh);
}

/// A right-pointing play triangle centred on `c`.
pub(crate) fn play_triangle(painter: &egui::Painter, c: egui::Pos2, size: f32, color: egui::Color32) {
    let pts = vec![
        c + egui::vec2(-size * 0.45, -size * 0.6),
        c + egui::vec2(-size * 0.45, size * 0.6),
        c + egui::vec2(size * 0.65, 0.0),
    ];
    painter.add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
}

impl Settings {
    /// The install folder for a store's games.
    pub(crate) fn store_dir(&self, store: stores::Store) -> PathBuf {
        let (set, default) = match store {
            stores::Store::Epic => (&self.epic_dir, "Games/Epic"),
            stores::Store::Gog => (&self.gog_dir, "Games/GOG"),
        };
        if set.is_empty() {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(default)
        } else {
            PathBuf::from(set)
        }
    }

    /// The tile size key in effect ("medium" when unset).
    pub(crate) fn tile_size_key(&self) -> &str {
        if TILE_SIZES.iter().any(|(k, ..)| *k == self.tile_size) { &self.tile_size } else { "medium" }
    }
}

/// The library layout buttons: three grid sizes (many small squares … one big
/// one) and a list. Returns "small", "medium", "large" or "list" when clicked.
pub(crate) fn tile_size_toggle(ui: &mut egui::Ui, settings: &Settings) -> Option<&'static str> {
    let current = if settings.library_view == "list" { "list" } else { settings.tile_size_key() };
    let mut picked = None;
    ui.spacing_mut().item_spacing.x = 4.0;
    let options = [("small", "Small tiles"), ("medium", "Medium tiles"), ("large", "Large tiles"), ("list", "List")];
    // Laid out from the far end, so add them in reverse to read small → list.
    let order: Vec<_> = if rtl() { options.to_vec() } else { options.iter().rev().copied().collect() };
    for (key, tip) in order {
        let on = key == current;
        let resp = ui
            .add(egui::Button::new("").min_size(egui::vec2(30.0, 30.0)).selected(on))
            .on_hover_text(tr(tip))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = if on { pal().accent } else if resp.hovered() { pal().text } else { pal().subtext };
        let area = egui::Rect::from_center_size(resp.rect.center(), egui::vec2(14.0, 14.0));
        if key == "list" {
            for r in 0..3 {
                let y = area.top() + 1.0 + r as f32 * 5.0;
                ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(area.left(), y), egui::vec2(3.0, 3.0)), rad(1), color);
                ui.painter().rect_filled(egui::Rect::from_min_size(egui::pos2(area.left() + 5.0, y), egui::vec2(9.0, 3.0)), rad(1), color);
            }
        } else {
            let n = match key {
                "small" => 3,
                "medium" => 2,
                _ => 1,
            };
            let gap = 2.0;
            let cell = (area.width() - gap * (n as f32 - 1.0)) / n as f32;
            for r in 0..n {
                for c in 0..n {
                    let min = area.min + egui::vec2(c as f32 * (cell + gap), r as f32 * (cell + gap));
                    ui.painter().rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell, cell)), rad(1), color);
                }
            }
        }
        if resp.clicked() {
            picked = Some(key);
        }
    }
    picked
}



#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PlayState {
    Play,
    Starting,
    Stop,
    Stopping,
}

/// The big Play / Stop button on the game page (icons are painted, not glyphs).
/// While starting or stopping it can't be clicked and shows that it's working.
/// `dx` offsets the drawing (the failed-action shake) without moving the layout.
pub(crate) fn play_button(ui: &mut egui::Ui, state: PlayState, phase: f32, dx: f32) -> egui::Response {
    let (label, fill) = match state {
        PlayState::Play => ("Play", pal().accent),
        PlayState::Starting => ("Starting…", pal().accent),
        PlayState::Stop => ("Stop", pal().danger),
        PlayState::Stopping => ("Stopping…", pal().danger),
    };
    let busy = matches!(state, PlayState::Starting | PlayState::Stopping);
    let size = egui::vec2(if busy { 158.0 } else { 132.0 }, 44.0);
    let sense = if busy { egui::Sense::hover() } else { egui::Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let rect = rect.translate(egui::vec2(dx, 0.0));
    let fill = match (busy, resp.hovered()) {
        (true, _) => fill.gamma_multiply(0.6),
        (false, true) => fill,
        (false, false) => fill.gamma_multiply(0.92),
    };
    let p = ui.painter();
    p.rect_filled(rect, rad(10), fill);
    let icon_c = if rtl() {
        egui::pos2(rect.right() - 34.0, rect.center().y)
    } else {
        egui::pos2(rect.left() + 34.0, rect.center().y)
    };
    match state {
        PlayState::Play => play_triangle(p, icon_c, 12.0, pal().on_accent),
        PlayState::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(icon_c, egui::vec2(13.0, 13.0)),
                rad(2),
                pal().on_accent,
            );
        }
        PlayState::Starting | PlayState::Stopping => {
            spinner_arc(p, icon_c, 8.0, phase * std::f32::consts::TAU, pal().on_accent);
            sheen(&p.with_clip_rect(rect), rect, phase);
        }
    }
    let (tx, talign) = if rtl() {
        (rect.right() - 54.0, egui::Align2::RIGHT_CENTER)
    } else {
        (rect.left() + 54.0, egui::Align2::LEFT_CENTER)
    };
    p.text(
        egui::pos2(tx, rect.center().y),
        talign,
        tr(label),
        egui::FontId::proportional(17.0),
        pal().on_accent,
    );
    if busy {
        resp
    } else {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// The dashed "+ Add game" tile at the end of the library grid, drawn at `rect`.
pub(crate) fn add_tile(ui: &mut egui::Ui, rect: egui::Rect, resp: egui::Response, acts: &mut Acts) {
    let poster = egui::Rect::from_min_size(rect.min, egui::vec2(tile_w(), tile_h()));
    let color = if resp.hovered() { pal().accent } else { pal().faint };
    let p = ui.painter();
    p.rect_filled(poster, rad(10), pal().card.gamma_multiply(0.5));
    let dash = egui::Shape::dashed_line(
        &[
            poster.left_top(),
            poster.right_top(),
            poster.right_bottom(),
            poster.left_bottom(),
            poster.left_top(),
        ],
        egui::Stroke::new(1.5, color),
        6.0,
        5.0,
    );
    p.extend(dash);
    let c = poster.center();
    p.line_segment([c - egui::vec2(12.0, 0.0), c + egui::vec2(12.0, 0.0)], egui::Stroke::new(2.0, color));
    p.line_segment([c - egui::vec2(0.0, 12.0), c + egui::vec2(0.0, 12.0)], egui::Stroke::new(2.0, color));
    p.text(
        c + egui::vec2(0.0, 30.0),
        egui::Align2::CENTER_TOP,
        tr("Add game"),
        egui::FontId::proportional(14.0),
        color,
    );
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.add = true;
    }
}

/// One "current artwork" entry on the Artwork page: preview, note, actions.
pub(crate) fn art_current(
    ui: &mut egui::Ui,
    kind: ArtKind,
    note: &str,
    custom: bool,
    acts: &mut Acts,
    preview: impl FnOnce(&mut egui::Ui),
) {
    preview(ui);
    vcol(ui, |ui| {
        let title = match kind {
            ArtKind::Icon => "Icon",
            ArtKind::Cover => "Cover",
            ArtKind::Hero => "Hero",
        };
        ui.label(egui::RichText::new(tr(title)).strong().color(pal().text));
        ui.label(egui::RichText::new(tr(note)).color(pal().subtext).small());
        ui.horizontal(|ui| {
            if ui.small_button(tr("Local file…")).clicked() {
                acts.art.push(ArtCmd::Local(kind));
            }
            if custom && ui.small_button(tr("Reset")).clicked() {
                acts.art.push(ArtCmd::Reset(kind));
            }
        });
    });
}

/// The game's exe (or, for Windows games, its Proton build) is missing.
pub(crate) fn game_missing_files(g: &CustomGame) -> bool {
    !Path::new(&g.exe).is_file()
        || (g.kind == GameKind::Windows && !Path::new(&g.proton_dir).join("proton").is_file())
}

/// The design's card for headline content: rounder and roomier than `card_frame`.
pub(crate) fn big_card() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(22))
        .corner_radius(rad(18))
        .fill(pal().card)
        .stroke(egui::Stroke::new(1.0, pal().border))
}

/// A tall secondary button: card fill, border, semibold label.
pub(crate) fn big_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).family(semibold_family()).size(14.0).color(pal().text))
            .fill(pal().card)
            .stroke(egui::Stroke::new(1.0, pal().border))
            .corner_radius(rad(10))
            .min_size(egui::vec2(0.0, 44.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A button for use over artwork: the page colour, no border.
pub(crate) fn dark_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(tr(label)).family(semibold_family()).size(15.0).color(pal().text))
            .fill(pal().bg)
            .stroke(egui::Stroke::NONE)
            .corner_radius(rad(12))
            .min_size(egui::vec2(0.0, 44.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small solid label ("Epic", "Playing", "JUMP BACK IN").
pub(crate) fn tag(ui: &mut egui::Ui, text: &str, bg: egui::Color32, fg: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(disp(text), egui::FontId::new(12.0, semibold_family()), fg);
    let (rect, _) = ui.allocate_exact_size(galley.size() + egui::vec2(18.0, 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, rad(6), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// Where a game comes from, for captions ("Epic Games", "Battle.net", "SNES", "Installed").
pub(crate) fn game_source(g: &CustomGame) -> String {
    if let Some(s) = &g.store {
        return s.store.name().to_string();
    }
    if let Some(l) = &g.launcher {
        return launchers::def(&l.key).map_or_else(|| l.key.clone(), |d| d.name.to_string());
    }
    if let Some(r) = &g.rom {
        return emulation::platform(&r.platform).map_or_else(|| r.platform.clone(), |p| p.name.to_string());
    }
    lg("Installed").to_string()
}

/// A filter chip with its count after the label ("Backlog 41").
pub(crate) fn count_chip(ui: &mut egui::Ui, on: bool, label: &str, count: usize) -> egui::Response {
    let id = ui.make_persistent_id(("count-chip", label));
    let t = ui.ctx().animate_bool_with_time(id, on, ui.style().animation_time * 1.5);
    let fg = pal().subtext.lerp_to_gamma(pal().on_accent, t);
    let fill = if t > 0.0 { pal().card.lerp_to_gamma(pal().accent, t) } else { pal().card };
    let stroke = pal().border.lerp_to_gamma(pal().accent, t);
    let mut job = egui::text::LayoutJob::default();
    let fmt = |family: egui::FontFamily, color: egui::Color32| egui::TextFormat { font_id: egui::FontId::new(14.0, family), color, ..Default::default() };
    job.append(&disp(&tr(label)), 0.0, fmt(semibold_family(), fg));
    job.append(&count.to_string(), 6.0, fmt(egui::FontFamily::Proportional, fg.gamma_multiply(0.75)));
    ui.add(egui::Button::new(job).fill(fill).stroke(egui::Stroke::new(1.0, stroke)).corner_radius(rad(255)).min_size(egui::vec2(0.0, 38.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The grid / list switch. Returns Some(true) for list, Some(false) for grid when clicked.
pub(crate) fn view_toggle(ui: &mut egui::Ui, list: bool) -> Option<bool> {
    let mut picked = None;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(88.0, 38.0), egui::Sense::hover());
    let p = ui.painter().clone();
    p.rect_stroke(rect, rad(10), egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);
    // Grid first in reading order.
    for (k, (is_list, g, tip)) in [(false, Glyph::Grid, "Grid"), (true, Glyph::List, "List")].into_iter().enumerate() {
        let col = if rtl() { 1 - k } else { k } as f32;
        let r = egui::Rect::from_min_size(rect.min + egui::vec2(col * 44.0, 0.0), egui::vec2(44.0, 38.0)).shrink(1.0);
        let resp = ui.interact(r, ui.scope_id().with(("view-toggle", k)), egui::Sense::click()).on_hover_text(tr(tip)).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = is_list == list;
        p.rect_filled(r, rad(9), if on { pal().card2 } else { pal().card });
        glyph(&p, g, egui::Rect::from_center_size(r.center(), egui::vec2(16.0, 16.0)), if on { pal().text } else if resp.hovered() { pal().subtext } else { pal().faint });
        if resp.clicked() && !on {
            picked = Some(is_list);
        }
    }
    picked
}

/// The design's quieter panel for charts and lists.
pub(crate) fn panel_card() -> egui::Frame {
    egui::Frame::new()
        .inner_margin(egui::Margin::same(22))
        .corner_radius(rad(18))
        .fill(pal().panel)
        .stroke(egui::Stroke::new(1.0, pal().border))
}

/// A segmented switch ("Week · Month · Year"). Returns the option clicked.
pub(crate) fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, options: &[(T, &str)], current: T) -> Option<T> {
    let font = egui::FontId::new(14.0, semibold_family());
    let widths: Vec<f32> = options.iter().map(|(_, l)| ui.painter().layout_no_wrap(tr(l), font.clone(), pal().text).size().x + 36.0).collect();
    let total = widths.iter().sum::<f32>() + 4.0 * (options.len() as f32 + 1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(total, 46.0), egui::Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, rad(12), pal().card);
    p.rect_stroke(rect, rad(12), egui::Stroke::new(1.0, pal().border), egui::StrokeKind::Inside);
    let mut picked = None;
    let mut x = 4.0;
    for (k, ((value, label), w)) in options.iter().zip(&widths).enumerate() {
        // Reading order: first option on the reading side.
        let left = if rtl() { rect.right() - x - w } else { rect.left() + x };
        let r = egui::Rect::from_min_size(egui::pos2(left, rect.top() + 4.0), egui::vec2(*w, 38.0));
        x += w + 4.0;
        let resp = ui.interact(r, ui.scope_id().with(("segmented", k)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = *value == current;
        if on {
            p.rect_filled(r, rad(9), pal().accent);
        } else if resp.hovered() {
            p.rect_filled(r, rad(9), pal().card2);
        }
        p.text(r.center(), egui::Align2::CENTER_CENTER, tr(label), font.clone(), if on { pal().on_accent } else { pal().subtext });
        if resp.clicked() && !on {
            picked = Some(*value);
        }
    }
    picked
}
