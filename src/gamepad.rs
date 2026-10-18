//! Game controller navigation, read straight from the kernel's input devices
//! (/dev/input/event*), which the desktop lets the logged-in user read. No
//! extra libraries, so the AppImage stays self-contained.
//!
//! A background thread finds controllers (also ones plugged in later) and
//! forwards their buttons and axes; `Nav` turns those into the keys the app
//! already understands: the d-pad or left stick are arrow keys (repeating
//! while held), A is Enter, B is Escape, Start is Space, LB/RB are [ and ],
//! Guide is F11 (couch mode), Y/X are I (details) and View is F (favourite).
//! The right stick scrolls.

use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_MODE: u16 = 0x13c;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_RY: u16 = 0x04;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

/// A change on a controller: a button (pressed or not), or an axis (-1..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Msg {
    Button(u16, bool),
    Axis(u16, f32),
}

/// Controller state turned into key presses, with key repeat for held directions.
#[derive(Default)]
pub struct Nav {
    buttons: HashSet<u16>,
    axes: HashMap<u16, f32>,
    pressed: Vec<egui::Key>,
    held: Option<egui::Key>,
    next_repeat: f64,
    /// When the controller was last touched (for the button hints).
    pub last_used: Option<f64>,
}

impl Nav {
    pub fn feed(&mut self, msg: Msg, now: f64) {
        self.last_used = Some(now);
        match msg {
            Msg::Button(code, true) => {
                if self.buttons.insert(code) {
                    let key = match code {
                        BTN_SOUTH => Some(egui::Key::Enter),
                        BTN_EAST => Some(egui::Key::Escape),
                        BTN_START => Some(egui::Key::Space),
                        BTN_TL => Some(egui::Key::OpenBracket),
                        BTN_TR => Some(egui::Key::CloseBracket),
                        // Couch mode: Guide toggles it, Y/X show details, View stars.
                        BTN_MODE => Some(egui::Key::F11),
                        BTN_NORTH | BTN_WEST => Some(egui::Key::I),
                        BTN_SELECT => Some(egui::Key::F),
                        _ => None,
                    };
                    self.pressed.extend(key);
                }
            }
            Msg::Button(code, false) => {
                self.buttons.remove(&code);
            }
            Msg::Axis(code, v) => {
                self.axes.insert(code, v);
            }
        }
    }

    fn axis(&self, code: u16) -> f32 {
        self.axes.get(&code).copied().unwrap_or(0.0)
    }

    /// The direction being held: d-pad (hat or buttons) first, else the left stick.
    fn direction(&self) -> Option<egui::Key> {
        let b = |c| self.buttons.contains(&c);
        let (hx, hy) = (self.axis(ABS_HAT0X), self.axis(ABS_HAT0Y));
        let x = if b(BTN_DPAD_LEFT) { -1.0 } else if b(BTN_DPAD_RIGHT) { 1.0 } else { hx };
        let y = if b(BTN_DPAD_UP) { -1.0 } else if b(BTN_DPAD_DOWN) { 1.0 } else { hy };
        let (x, y) = if x != 0.0 || y != 0.0 { (x, y) } else { (self.axis(ABS_X), self.axis(ABS_Y)) };
        const DEAD: f32 = 0.55;
        if x.abs() < DEAD && y.abs() < DEAD {
            return None;
        }
        Some(if x.abs() > y.abs() {
            if x > 0.0 { egui::Key::ArrowRight } else { egui::Key::ArrowLeft }
        } else if y > 0.0 {
            egui::Key::ArrowDown
        } else {
            egui::Key::ArrowUp
        })
    }

    /// Keys pressed since the last frame, and how fast the right stick asks to
    /// scroll (points per second, positive = down).
    pub fn frame(&mut self, now: f64) -> (Vec<egui::Key>, f32) {
        let mut keys = std::mem::take(&mut self.pressed);
        match self.direction() {
            Some(d) if self.held != Some(d) => {
                keys.push(d);
                self.held = Some(d);
                self.next_repeat = now + 0.4;
            }
            Some(d) if now >= self.next_repeat => {
                keys.push(d);
                self.next_repeat = now + 0.11;
            }
            Some(_) => {}
            None => self.held = None,
        }
        let ry = self.axis(ABS_RY);
        let scroll = if ry.abs() > 0.2 { ry * 1400.0 } else { 0.0 };
        (keys, scroll)
    }

    /// A direction is held or the right stick is pushed: keep frames coming.
    pub fn active(&self) -> bool {
        self.held.is_some() || self.axis(ABS_RY).abs() > 0.2
    }
}

/// The reader thread's handle.
pub struct Gamepad {
    rx: Receiver<Msg>,
    /// Names of the controllers currently connected.
    pub connected: Arc<Mutex<Vec<String>>>,
    pub nav: Nav,
}

impl Gamepad {
    pub fn start(ctx: egui::Context) -> Gamepad {
        let (tx, rx) = std::sync::mpsc::channel();
        let connected = Arc::new(Mutex::new(Vec::new()));
        let names = connected.clone();
        std::thread::Builder::new()
            .name("gamepad".into())
            .spawn(move || read_loop(tx, names, ctx))
            .ok();
        Gamepad { rx, connected, nav: Nav::default() }
    }

    /// Pass on everything the thread read since the last frame.
    pub fn drain(&mut self, now: f64) {
        while let Ok(msg) = self.rx.try_recv() {
            self.nav.feed(msg, now);
        }
    }
}

/// An open controller.
struct Device {
    sys: PathBuf,
    file: File,
    /// Axis ranges (min, max), for scaling to -1..1.
    ranges: HashMap<u16, (i32, i32)>,
}

/// Does this input device have a gamepad's south (A) button?
fn is_gamepad(sys: &Path) -> bool {
    let Ok(bits) = std::fs::read_to_string(sys.join("device/capabilities/key")) else {
        return false;
    };
    // Space-separated 64-bit words, most significant first.
    let words: Vec<u64> = bits.split_whitespace().rev().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
    let bit = BTN_SOUTH as usize;
    words.get(bit / 64).is_some_and(|w| w >> (bit % 64) & 1 == 1)
}

/// The range of axis `code` (EVIOCGABS).
fn abs_range(file: &File, code: u16) -> Option<(i32, i32)> {
    let mut info = [0i32; 6]; // value, minimum, maximum, fuzz, flat, resolution
    let request = (2u64 << 30) | (24u64 << 16) | (0x45u64 << 8) | (0x40 + u64::from(code));
    // SAFETY: EVIOCGABS fills a 24-byte input_absinfo, which `info` is.
    let r = unsafe { libc::ioctl(file.as_raw_fd(), request as _, info.as_mut_ptr()) };
    (r == 0 && info[2] > info[1]).then_some((info[1], info[2]))
}

fn open(sys: &Path) -> Option<Device> {
    let dev = Path::new("/dev/input").join(sys.file_name()?);
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(dev).ok()?;
    let ranges = [ABS_X, ABS_Y, ABS_RY, ABS_HAT0X, ABS_HAT0Y]
        .into_iter()
        .filter_map(|c| abs_range(&file, c).map(|r| (c, r)))
        .collect();
    Some(Device { sys: sys.to_path_buf(), file, ranges })
}

fn read_loop(tx: Sender<Msg>, names: Arc<Mutex<Vec<String>>>, ctx: egui::Context) {
    let mut devices: Vec<Device> = Vec::new();
    let mut last_scan: Option<Instant> = None;
    loop {
        if last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            last_scan = Some(Instant::now());
            if let Ok(rd) = std::fs::read_dir("/sys/class/input") {
                for e in rd.flatten() {
                    let sys = e.path();
                    let is_event = e.file_name().to_string_lossy().starts_with("event");
                    if is_event && !devices.iter().any(|d| d.sys == sys) && is_gamepad(&sys) {
                        devices.extend(open(&sys));
                    }
                }
            }
            let list: Vec<String> = devices
                .iter()
                .map(|d| std::fs::read_to_string(d.sys.join("device/name")).unwrap_or_default().trim().to_string())
                .collect();
            *names.lock().unwrap_or_else(|e| e.into_inner()) = list;
        }
        if devices.is_empty() {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        let mut fds: Vec<libc::pollfd> =
            devices.iter().map(|d| libc::pollfd { fd: d.file.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: `fds` is a valid array of pollfd for the duration of the call.
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, 250) };
        let mut sent = false;
        let mut gone = Vec::new();
        for (n, d) in devices.iter_mut().enumerate() {
            let mut buf = [0u8; 24 * 64];
            match d.file.read(&mut buf) {
                Ok(len) => {
                    for ev in buf[..len].as_chunks::<24>().0 {
                        // struct input_event: timeval (16 bytes), type, code, value.
                        let kind = u16::from_ne_bytes([ev[16], ev[17]]);
                        let code = u16::from_ne_bytes([ev[18], ev[19]]);
                        let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                        let msg = match kind {
                            EV_KEY if value != 2 => Some(Msg::Button(code, value == 1)),
                            EV_ABS => d.ranges.get(&code).map(|&(lo, hi)| {
                                Msg::Axis(code, (2.0 * (value - lo) as f32 / (hi - lo) as f32 - 1.0).clamp(-1.0, 1.0))
                            }),
                            _ => None,
                        };
                        if let Some(m) = msg {
                            sent |= tx.send(m).is_ok();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                // Unplugged.
                Err(_) => gone.push(n),
            }
        }
        for n in gone.into_iter().rev() {
            devices.remove(n);
            last_scan = None;
        }
        if sent {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpad_repeats_and_buttons_map_to_keys() {
        let mut nav = Nav::default();
        nav.feed(Msg::Axis(ABS_HAT0X, 1.0), 0.0);
        assert_eq!(nav.frame(0.0).0, vec![egui::Key::ArrowRight]);
        assert!(nav.frame(0.2).0.is_empty(), "no repeat before the delay");
        assert_eq!(nav.frame(0.45).0, vec![egui::Key::ArrowRight]);
        nav.feed(Msg::Axis(ABS_HAT0X, 0.0), 0.5);
        assert!(nav.frame(0.6).0.is_empty());
        // A small stick wobble is ignored; a full push moves.
        nav.feed(Msg::Axis(ABS_Y, 0.3), 1.0);
        assert!(nav.frame(1.0).0.is_empty());
        nav.feed(Msg::Axis(ABS_Y, 0.9), 1.1);
        assert_eq!(nav.frame(1.1).0, vec![egui::Key::ArrowDown]);
        nav.feed(Msg::Button(BTN_SOUTH, true), 1.2);
        nav.feed(Msg::Button(BTN_SOUTH, false), 1.25);
        assert!(nav.frame(1.3).0.contains(&egui::Key::Enter));
        nav.feed(Msg::Axis(ABS_RY, -1.0), 1.4);
        assert!(nav.frame(1.4).1 < 0.0, "right stick up scrolls up");
    }

    /// cargo test -- --ignored gamepad_live --nocapture (with a controller plugged in)
    #[test]
    #[ignore]
    fn gamepad_live() {
        let pads: Vec<_> = std::fs::read_dir("/sys/class/input")
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("event") && is_gamepad(p))
            .collect();
        assert!(!pads.is_empty(), "no controller found");
        for p in &pads {
            let d = open(p).expect("can't open the controller");
            eprintln!("{}: {:?}", std::fs::read_to_string(p.join("device/name")).unwrap().trim(), d.ranges);
            assert!(d.ranges.contains_key(&ABS_X), "axis ranges unreadable");
        }
    }

    #[test]
    fn gamepads_are_told_apart_from_keyboards() {
        let dir = std::env::temp_dir().join(format!("plaunch-pad-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("device/capabilities")).unwrap();
        // An Xbox pad's key bits (BTN_SOUTH is bit 0x130).
        std::fs::write(dir.join("device/capabilities/key"), "7cdb000000000000 0 0 0 0\n").unwrap();
        assert!(is_gamepad(&dir));
        std::fs::write(dir.join("device/capabilities/key"), "1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe\n").unwrap();
        assert!(!is_gamepad(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
