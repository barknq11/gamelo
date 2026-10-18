//! Fonts, the egui style, the app icon, and moving data over from the ProtonLaunch name.

use crate::*;

/// Find a regular-weight system UI font. egui's bundled Ubuntu Light is thin
/// and hard to read at small sizes.
pub(crate) fn find_ui_font() -> Option<(String, egui::FontData)> {
    use skrifa::MetadataProvider;
    for family in ["Inter", "Noto Sans", "Cantarell", "Roboto", "Open Sans", "DejaVu Sans"] {
        let out = Command::new("fc-match")
            .args(["-f", "%{family[0]}\n%{file}\n%{index}", &format!("{family}:style=Regular")])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let (Some(found), Some(file)) = (lines.next(), lines.next()) else {
            continue;
        };
        // fc-match always answers with *something*; only take the family we asked for.
        if !found.eq_ignore_ascii_case(family) {
            continue;
        }
        let index: u32 = lines.next().and_then(|i| i.parse().ok()).unwrap_or(0);
        let Ok(bytes) = fs::read(file) else { continue };
        // Validate before handing it to egui: a bad font would abort at startup.
        let usable = skrifa::FontRef::from_index(&bytes, index)
            .is_ok_and(|f| "Aa0".chars().all(|c| f.charmap().map(c).is_some()));
        if !usable {
            continue;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        return Some((family.to_string(), data));
    }
    None
}

/// Arabic glyphs (including the joined presentation forms the shaper emits),
/// bundled so Arabic works whatever fonts the system has. SIL OFL, see assets/.
pub(crate) const ARABIC_FONT: &[u8] = include_bytes!("../assets/NotoSansArabicUI-subset.ttf");

/// Put the system UI font first and egui's bundled fonts after it as glyph
/// fallbacks. The Arabic font goes before all of them: it has nothing but
/// Arabic glyphs, and owning every Arabic character keeps egui's shaping runs
/// predictable (see `shaping`) whatever the system font covers.
pub(crate) fn load_ui_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some((name, data)) = find_ui_font() {
        fonts.font_data.insert(name.clone(), Arc::new(data));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, name);
    }
    fonts
        .font_data
        .insert("arabic".into(), Arc::new(egui::FontData::from_static(ARABIC_FONT)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().insert(0, "arabic".into());
    }
    ctx.set_fonts(fonts);
}

pub(crate) fn configure_style(ctx: &egui::Context) {
    load_ui_font(ctx);
    ctx.all_styles_mut(|style| {
        // egui's defaults (Small = 9 px, Body = 12.5 px) are too small to read comfortably.
        use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
        style.text_styles = [
            (TextStyle::Small, FontId::new(12.0, Proportional)),
            (TextStyle::Body, FontId::new(14.5, Proportional)),
            (TextStyle::Button, FontId::new(14.5, Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, Monospace)),
            (TextStyle::Heading, FontId::new(20.0, Proportional)),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(14.0, 8.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.window_margin = egui::Margin::same(16);
        style.spacing.menu_margin = egui::Margin::same(8);

    });
}

/// ProtonLaunch was renamed Gamelo. Move its folders over once, leaving a link
/// at each old path so anything still pointing there (prefixes, Steam, logs)
/// keeps working, and point saved paths and game shortcuts at the new names.
pub(crate) fn migrate_old_name() {
    let cache = thumb_cache_dir().parent().map(Path::to_path_buf).unwrap_or_default();
    let old_data = home().join(".local/share/protonlaunch");
    for (old, new) in [
        (home().join(".config/protonlaunch"), config_path().parent().map(Path::to_path_buf).unwrap_or_default()),
        (old_data.clone(), data_dir()),
        (cache.with_file_name("protonlaunch"), cache),
    ] {
        if !new.exists() && old.is_dir() && !old.is_symlink() && fs::rename(&old, &new).is_ok() {
            let _ = std::os::unix::fs::symlink(&new, &old);
        }
    }
    let old_prefix = format!("{}/", old_data.display());
    if let Ok(s) = fs::read_to_string(config_path())
        && s.contains(&old_prefix)
    {
        let _ = fs::write(config_path(), s.replace(&old_prefix, &format!("{}/", data_dir().display())));
    }
    // Game shortcuts in the app menu; not from a development build, which
    // would point them at itself.
    let exe = self_exe();
    if exe.components().any(|c| c.as_os_str() == "target") {
        return;
    }
    let apps = home().join(".local/share/applications");
    for e in fs::read_dir(&apps).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix("protonlaunch-").and_then(|n| n.strip_suffix(".desktop")) else { continue };
        let Ok(s) = fs::read_to_string(e.path()) else { continue };
        let exec = format!("Exec={} --run {id}", desktop_exec_arg(&exe.to_string_lossy()));
        let s: Vec<String> = s
            .lines()
            .map(|l| if l.starts_with("Exec=") { exec.clone() } else { l.replace("ProtonLaunch", "Gamelo") })
            .collect();
        if fs::write(apps.join(format!("gamelo-{id}.desktop")), s.join("\n") + "\n").is_ok() {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// The app icon, shared by the window, the sidebar and Settings.
pub(crate) const ICON_PNG: &[u8] = include_bytes!("../packaging/gamelo.png");

pub(crate) fn icon_rgba() -> image::RgbaImage {
    image::load_from_memory(ICON_PNG).map(|i| i.to_rgba8()).unwrap_or_default()
}

/// The icon as a texture (made once, with mipmaps so it stays crisp small).
pub(crate) fn brand_icon(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("gamelo-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = icon_rgba();
    let image = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img);
    let options = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
    let tex = ctx.load_texture("gamelo-icon", image, options);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Paint the app icon into `rect`.
pub(crate) fn brand_mark(ui: &egui::Ui, rect: egui::Rect) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(brand_icon(ui.ctx()).id(), rect, uv, egui::Color32::WHITE);
}
