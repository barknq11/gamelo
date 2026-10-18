//! Game icons: pulled from executables and image files, normalised and cached.

use crate::*;

// ---------------------------------------------------------------------------
// Game icons
// ---------------------------------------------------------------------------

/// Where normalized (already square, 64×64) icon PNGs are cached.
pub(crate) fn icons_dir() -> PathBuf {
    data_dir().join("icons")
}

/// Where artwork downloaded from SteamGridDB is stored.
pub(crate) fn art_dir() -> PathBuf {
    data_dir().join("art")
}

/// Where an icon should come from.
pub(crate) enum IconSource {
    /// An image file (custom or downloaded artwork) to crop/resize.
    Image(PathBuf),
    /// A Windows PE executable to extract the embedded icon from.
    Exe(PathBuf),
    /// Nothing available; the UI draws a lettered placeholder.
    None,
}

/// Decode any supported image file into an RGBA buffer (format sniffed from content).
pub(crate) fn decode_image(path: &Path) -> Option<image::RgbaImage> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
        .map(|i| i.to_rgba8())
}

/// Center-crop to a square and scale to `size`×`size`.
pub(crate) fn square_icon(img: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let (w, h) = (img.width(), img.height());
    let side = w.min(h).max(1);
    let cropped = image::imageops::crop_imm(img, (w - side) / 2, (h - side) / 2, side, side)
        .to_image();
    if side == size {
        cropped
    } else {
        image::imageops::resize(&cropped, size, size, image::imageops::FilterType::Triangle)
    }
}

/// Scale down (never up) to fit inside `max_w`×`max_h`, keeping the aspect ratio.
pub(crate) fn fit_within(img: &image::RgbaImage, max_w: u32, max_h: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max_w && h <= max_h {
        return img.clone();
    }
    let s = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    let nw = ((w as f32 * s).round() as u32).max(1);
    let nh = ((h as f32 * s).round() as u32).max(1);
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

/// Center-crop to the `w`:`h` aspect ratio and scale to exactly `w`×`h`.
pub(crate) fn cover_fit(img: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let (iw, ih) = img.dimensions();
    let (cw, ch) = if iw as u64 * h as u64 > ih as u64 * w as u64 {
        ((((ih as u64) * w as u64) / h as u64).max(1) as u32, ih)
    } else {
        (iw, (((iw as u64) * h as u64) / w as u64).max(1) as u32)
    };
    let cropped = image::imageops::crop_imm(img, (iw - cw) / 2, (ih - ch) / 2, cw, ch).to_image();
    image::imageops::resize(&cropped, w, h, image::imageops::FilterType::Triangle)
}

/// Darken an image for use behind text: brightest top-right, darkest bottom-left
/// (where the game page puts its title and buttons).
pub(crate) fn darken_for_text(img: &mut image::RgbaImage) {
    let (w, h) = img.dimensions();
    let (fw, fh) = ((w.max(2) - 1) as f32, (h.max(2) - 1) as f32);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let k = (0.22 + 0.45 * (x as f32 / fw) + 0.33 * (1.0 - y as f32 / fh)).min(1.0);
        for c in &mut px.0[..3] {
            *c = (*c as f32 * k) as u8;
        }
        px.0[3] = 255;
    }
}

/// A wide game-page banner from a hero image (sharp) or a cover (blurred).
pub(crate) fn hero_from(img: &image::RgbaImage, blurred: bool) -> image::RgbaImage {
    let mut out = if blurred {
        image::imageops::blur(&cover_fit(img, 640, 210), 5.0)
    } else {
        cover_fit(img, 1280, 420)
    };
    darken_for_text(&mut out);
    out
}

/// Two pleasant dark colours derived from a string, for icon-less placeholders.
pub(crate) fn seed_colors(seed: &str) -> ([u8; 3], [u8; 3]) {
    let h = seed
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619));
    let hue = (h % 360) as f32;
    let rgb = |hue: f32, l: f32| {
        // HSL with fixed saturation, enough for muted backgrounds.
        let c = (1.0 - (2.0 * l - 1.0).abs()) * 0.45;
        let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let (r, g, b) = match (hue / 60.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        [((r + m) * 255.0) as u8, ((g + m) * 255.0) as u8, ((b + m) * 255.0) as u8]
    };
    (rgb(hue, 0.26), rgb((hue + 40.0) % 360.0, 0.12))
}

/// A stand-in 300×450 poster for games without a cover: the icon over a
/// blurred wash of its own colours (or a colour gradient if there's no icon).
pub(crate) fn generated_poster(icon: Option<&image::RgbaImage>, seed: &str) -> image::RgbaImage {
    let (w, h) = (300u32, 450u32);
    let (top, bottom) = seed_colors(seed);
    let mut img = image::RgbaImage::from_fn(w, h, |_, y| {
        let t = y as f32 / (h - 1) as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        image::Rgba([mix(top[0], bottom[0]), mix(top[1], bottom[1]), mix(top[2], bottom[2]), 255])
    });
    if let Some(ic) = icon {
        let wash = image::imageops::blur(
            &image::imageops::resize(ic, w, h, image::imageops::FilterType::Triangle),
            22.0,
        );
        image::imageops::overlay(&mut img, &wash, 0, 0);
        for px in img.pixels_mut() {
            for c in &mut px.0[..3] {
                *c = (*c as f32 * 0.5) as u8;
            }
            px.0[3] = 255;
        }
        let big = square_icon(ic, 128);
        image::imageops::overlay(&mut img, &big, i64::from((w - 128) / 2), 110);
    }
    img
}

pub(crate) fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
pub(crate) fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read an IMAGE_RESOURCE_DATA_ENTRY and return the bytes it points at.
pub(crate) fn res_data<'a>(data: &'a [u8], entry: usize, foff: &dyn Fn(usize) -> Option<usize>) -> Option<&'a [u8]> {
    if entry + 8 > data.len() {
        return None;
    }
    let rva = le32(data, entry) as usize;
    let size = le32(data, entry + 4) as usize;
    let fo = foff(rva)?;
    if fo + size <= data.len() {
        Some(&data[fo..fo + size])
    } else {
        None
    }
}

/// Walk one resource-type directory down to its leaf data (id → bytes).
pub(crate) fn res_leaves<'a>(
    data: &'a [u8],
    res_off: usize,
    dir_off: usize,
    foff: &dyn Fn(usize) -> Option<usize>,
) -> Vec<(u16, &'a [u8])> {
    let mut out = Vec::new();
    const MASK: u32 = 0x7fff_ffff;
    if dir_off + 16 > data.len() {
        return out;
    }
    let n = le16(data, dir_off + 12) as usize + le16(data, dir_off + 14) as usize;
    for i in 0..n {
        let e = dir_off + 16 + i * 8;
        if e + 8 > data.len() {
            break;
        }
        let name = le32(data, e);
        let off = le32(data, e + 4);
        if name & 0x8000_0000 != 0 || off & 0x8000_0000 == 0 {
            continue;
        }
        let id = name as u16;
        let level2 = res_off + (off & MASK) as usize;
        if level2 + 16 > data.len() {
            continue;
        }
        let n2 = le16(data, level2 + 12) as usize + le16(data, level2 + 14) as usize;
        for j in 0..n2 {
            let e2 = level2 + 16 + j * 8;
            if e2 + 8 > data.len() {
                break;
            }
            let off2 = le32(data, e2 + 4);
            if off2 & 0x8000_0000 == 0 {
                if let Some(b) = res_data(data, res_off + off2 as usize, foff) {
                    out.push((id, b));
                }
            } else {
                let lang = res_off + (off2 & MASK) as usize;
                if lang + 16 > data.len() {
                    continue;
                }
                let n3 = le16(data, lang + 12) as usize + le16(data, lang + 14) as usize;
                for k in 0..n3 {
                    let e3 = lang + 16 + k * 8;
                    if e3 + 8 > data.len() {
                        break;
                    }
                    let off3 = le32(data, e3 + 4);
                    if off3 & 0x8000_0000 == 0
                        && let Some(b) = res_data(data, res_off + off3 as usize, foff)
                    {
                        out.push((id, b));
                    }
                }
            }
        }
    }
    out
}

/// Rebuild a valid `.ico` file from a Windows PE's icon resources.
///
/// Reads RT_GROUP_ICON (the directory) and RT_ICON (the raw images) from the
/// resource section. The result can be decoded by the `image` crate.
pub(crate) fn pe_extract_ico(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 0x40 || &data[0..2] != b"MZ" {
        return None;
    }
    let pe = le32(data, 0x3C) as usize;
    if pe + 24 > data.len() || &data[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    let coff = pe + 4;
    let num_sections = le16(data, coff + 2) as usize;
    let size_opt = le16(data, coff + 16) as usize;
    let opt = coff + 20;
    if opt + 2 > data.len() {
        return None;
    }
    let magic = le16(data, opt);
    let (dd_off, num_rva_off) = match magic {
        0x10b => (opt + 96, opt + 92),  // PE32
        0x20b => (opt + 112, opt + 108), // PE32+
        _ => return None,
    };
    if le32(data, num_rva_off) < 3 || dd_off + 3 * 8 > data.len() {
        return None;
    }
    let res_rva = le32(data, dd_off + 2 * 8) as usize;
    if res_rva == 0 {
        return None;
    }
    let sec = opt + size_opt;
    let foff = |rva: usize| -> Option<usize> {
        for i in 0..num_sections {
            let s = sec + i * 40;
            if s + 40 > data.len() {
                return None;
            }
            let vsize = le32(data, s + 8) as usize;
            let va = le32(data, s + 12) as usize;
            let raw_size = le32(data, s + 16) as usize;
            let raw = le32(data, s + 20) as usize;
            if rva >= va && rva < va + vsize.max(raw_size) {
                return Some(rva - va + raw);
            }
        }
        None
    };
    let res_off = foff(res_rva)?;

    // Root directory: find RT_ICON (3) and RT_GROUP_ICON (14).
    if res_off + 16 > data.len() {
        return None;
    }
    let n = le16(data, res_off + 12) as usize + le16(data, res_off + 14) as usize;
    let (mut rt_icon, mut rt_group) = (None, None);
    for i in 0..n {
        let e = res_off + 16 + i * 8;
        if e + 8 > data.len() {
            break;
        }
        let name = le32(data, e);
        let off = le32(data, e + 4);
        if name & 0x8000_0000 != 0 || off & 0x8000_0000 == 0 {
            continue;
        }
        let sub = res_off + (off & 0x7fff_ffff) as usize;
        match name {
            3 => rt_icon = Some(sub),
            14
                if rt_group.is_none() => {
                    rt_group = Some(sub);
                }
            _ => {}
        }
        if rt_group.is_some() && rt_icon.is_some() { break; }
    }
    let icons = res_leaves(data, res_off, rt_icon?, &foff);
    let groups = res_leaves(data, res_off, rt_group?, &foff);
    let grp = groups.into_iter().next()?.1;
    if grp.len() < 6 {
        return None;
    }
    let count = le16(grp, 4) as usize;

    let mut entries = Vec::new();
    let mut image_data = Vec::new();
    let mut offset = 6 + count * 16;
    for i in 0..count {
        let ge = 6 + i * 14;
        if ge + 14 > grp.len() {
            break;
        }
        let (w, h, color) = (grp[ge], grp[ge + 1], grp[ge + 2]);
        let planes = le16(grp, ge + 4);
        let bitcount = le16(grp, ge + 6);
        let id = le16(grp, ge + 12);
        let Some((_, blob)) = icons.iter().find(|(bid, _)| *bid == id) else {
            continue;
        };
        entries.push((
            w,
            h,
            color,
            planes,
            bitcount,
            blob.len() as u32,
            offset as u32,
        ));
        image_data.extend_from_slice(blob);
        offset += blob.len();
    }
    if entries.is_empty() {
        return None;
    }

    let mut ico = Vec::with_capacity(offset);
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (w, h, color, planes, bitcount, size, off) in &entries {
        ico.push(*w);
        ico.push(*h);
        ico.push(*color);
        ico.push(0);
        ico.extend_from_slice(&planes.to_le_bytes());
        ico.extend_from_slice(&bitcount.to_le_bytes());
        ico.extend_from_slice(&size.to_le_bytes());
        ico.extend_from_slice(&off.to_le_bytes());
    }
    ico.extend_from_slice(&image_data);
    Some(ico)
}

/// Extract the icon from a Windows executable, decoded to RGBA.
pub(crate) fn extract_exe_icon(exe: &Path) -> Option<image::RgbaImage> {
    let data = fs::read(exe).ok()?;
    let ico = pe_extract_ico(&data)?;
    image::load_from_memory_with_format(&ico, image::ImageFormat::Ico)
        .ok()
        .map(|i| i.to_rgba8())
}

/// Locate an executable by name in PATH (plus ~/.local/bin and /usr/local/bin).
/// If `name` contains a slash it's treated as a path.
pub(crate) fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            let c = d.join(name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    for d in [home().join(".local/bin"), PathBuf::from("/usr/local/bin")] {
        let c = d.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

pub(crate) fn find_umu() -> Option<PathBuf> {
    find_in_path("umu-run")
}
