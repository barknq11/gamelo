//! The tray icon (while the window is open) and the "Show Gamelo" global
//! shortcut, registered through the desktop's GlobalShortcuts portal.

use crate::i18n::logical as lg;
use crate::*;
use ksni::blocking::TrayMethods;
use std::sync::mpsc::Sender;

pub enum TrayMsg {
    Show,
    Play(String),
    Quit,
}

struct GameloTray {
    tx: Sender<TrayMsg>,
    ctx: egui::Context,
    /// Recent games: (id, name).
    recent: Vec<(String, String)>,
    icon: Vec<ksni::Icon>,
}

impl GameloTray {
    fn send(&self, m: TrayMsg) {
        let _ = self.tx.send(m);
        self.ctx.request_repaint();
    }
}

/// The app icon as the tray wants it: ARGB, big-endian.
fn tray_icons() -> Vec<ksni::Icon> {
    let img = icon_rgba();
    [32u32, 64]
        .into_iter()
        .map(|s| {
            let small = image::imageops::resize(&img, s, s, image::imageops::FilterType::Triangle);
            let data = small.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
            ksni::Icon { width: s as i32, height: s as i32, data }
        })
        .collect()
}

impl ksni::Tray for GameloTray {
    fn id(&self) -> String {
        "gamelo".into()
    }
    fn title(&self) -> String {
        "Gamelo".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Gamelo".into(), description: lg("Your games, in one place").into(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayMsg::Show);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let mut items: Vec<ksni::MenuItem<Self>> = self
            .recent
            .iter()
            .map(|(id, name)| {
                let id = id.clone();
                StandardItem {
                    // A single underscore marks an access key in tray menus.
                    label: name.replace('_', "__"),
                    icon_name: "media-playback-start".into(),
                    activate: Box::new(move |t: &mut Self| t.send(TrayMsg::Play(id.clone()))),
                    ..Default::default()
                }
                .into()
            })
            .collect();
        if !items.is_empty() {
            items.push(ksni::MenuItem::Separator);
        }
        items.push(StandardItem { label: lg("Open Gamelo").into(), activate: Box::new(|t: &mut Self| t.send(TrayMsg::Show)), ..Default::default() }.into());
        items.push(StandardItem { label: lg("Quit").into(), icon_name: "application-exit".into(), activate: Box::new(|t: &mut Self| t.send(TrayMsg::Quit)), ..Default::default() }.into());
        items
    }
}

pub struct TrayHandle {
    handle: Option<ksni::blocking::Handle<GameloTray>>,
    pub rx: std::sync::mpsc::Receiver<TrayMsg>,
    /// What the menu last showed, to update it only on change.
    shown: Vec<(String, String)>,
}

impl TrayHandle {
    /// Start the tray icon and the global shortcut. Both fail quietly on
    /// desktops without them.
    pub fn start(ctx: &egui::Context) -> TrayHandle {
        let (tx, rx) = std::sync::mpsc::channel();
        let tray = GameloTray { tx: tx.clone(), ctx: ctx.clone(), recent: Vec::new(), icon: tray_icons() };
        let handle = match tray.spawn() {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!("gamelo: no tray icon: {e}");
                None
            }
        };
        let ctx = ctx.clone();
        // GAMELO_NO_SHORTCUT=1 skips it (the desktop asks to confirm a new shortcut).
        let skip = std::env::var_os("GAMELO_NO_SHORTCUT").is_some();
        std::thread::spawn(move || {
            if skip {
                return;
            }
            if let Err(e) = global_shortcut(&tx, &ctx) {
                eprintln!("gamelo: no global shortcut: {e}");
            }
        });
        TrayHandle { handle, rx, shown: Vec::new() }
    }

    pub fn set_recent(&mut self, recent: Vec<(String, String)>) {
        if recent != self.shown
            && let Some(h) = &self.handle
        {
            let r = recent.clone();
            h.update(move |t| t.recent = r);
            self.shown = recent;
        }
    }
}

/// Bind "Show Gamelo" (Meta+G by default; the user can change it in their
/// desktop's shortcut settings) and send Show whenever it's pressed.
fn global_shortcut(tx: &Sender<TrayMsg>, ctx: &egui::Context) -> zbus::Result<()> {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};
    const DEST: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    let conn = zbus::blocking::Connection::session()?;
    // Unsandboxed apps tell the portal who they are (matches gamelo.desktop).
    let _ = conn.call_method(Some(DEST), PATH, Some("org.freedesktop.host.portal.Registry"), "Register", &("gamelo", HashMap::<&str, Value>::new()));
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    // Portal calls answer through a Request object's Response signal.
    let request = |method: &str, token: &str, body: &dyn Fn(&str) -> zbus::Result<()>| -> zbus::Result<HashMap<String, OwnedValue>> {
        let path = format!("{PATH}/request/{sender}/{token}");
        let proxy = zbus::blocking::Proxy::new(&conn, DEST, path, "org.freedesktop.portal.Request")?;
        let mut responses = proxy.receive_signal("Response")?;
        body(token)?;
        let msg = responses.next().ok_or_else(|| zbus::Error::Failure(format!("{method}: no response")))?;
        let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
        if code != 0 {
            return Err(zbus::Error::Failure(format!("{method} was declined ({code})")));
        }
        Ok(results)
    };
    let shortcuts = zbus::blocking::Proxy::new(&conn, DEST, PATH, "org.freedesktop.portal.GlobalShortcuts")?;
    let results = request("CreateSession", "gamelo_session_req", &|token| {
        let opts: HashMap<&str, Value> = [("handle_token", Value::from(token)), ("session_handle_token", Value::from("gamelo_session"))].into();
        shortcuts.call_method("CreateSession", &(opts,)).map(|_| ())
    })?;
    let session: String = results
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok().or_else(|| zbus::zvariant::OwnedObjectPath::try_from(v.clone()).ok().map(|p| p.to_string())))
        .ok_or_else(|| zbus::Error::Failure("no session handle".into()))?;
    let session_path = zbus::zvariant::ObjectPath::try_from(session.as_str())?;
    let mut activated = shortcuts.receive_signal("Activated")?;
    request("BindShortcuts", "gamelo_bind_req", &|token| {
        let props: HashMap<&str, Value> = [("description", Value::from(lg("Show Gamelo"))), ("preferred_trigger", Value::from("LOGO+g"))].into();
        let opts: HashMap<&str, Value> = [("handle_token", Value::from(token))].into();
        shortcuts.call_method("BindShortcuts", &(&session_path, vec![("show", props)], "", opts)).map(|_| ())
    })?;
    for msg in activated.by_ref() {
        if let Ok((_, id, _, _)) = msg.body().deserialize::<(zbus::zvariant::OwnedObjectPath, String, u64, HashMap<String, OwnedValue>)>()
            && id == "show"
        {
            let _ = tx.send(TrayMsg::Show);
            ctx.request_repaint();
        }
    }
    Ok(())
}

impl App {
    /// Handle tray clicks and the shortcut, and keep the tray's recent games current.
    pub fn tray_poll(&mut self, ctx: &egui::Context, acts: &mut Acts) {
        let mut recent: Vec<(u64, String, String)> = self
            .cfg
            .custom_games
            .iter()
            .filter_map(|g| self.stats.get(&g.id).filter(|s| s.last_played > 0).map(|s| (s.last_played, g.id.clone(), g.name.clone())))
            .collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.0));
        let Some(tray) = self.tray.as_mut() else { return };
        tray.set_recent(recent.into_iter().take(5).map(|(_, id, name)| (id, name)).collect());
        while let Ok(m) = tray.rx.try_recv() {
            match m {
                TrayMsg::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayMsg::Play(id) => acts.game.push((id, GameAct::Play)),
                TrayMsg::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}
