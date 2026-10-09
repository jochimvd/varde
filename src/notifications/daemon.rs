use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

use zbus::{interface, zvariant::OwnedValue};

use super::{
    image::{self, Thumbnail},
    model::{self, Snapshot},
    sound::{Player, Sound},
    state::{Action, CloseReason, Incoming, Picture, Store, Urgency},
};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const MAX_TEXT_BYTES: usize = 4 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_ACTIONS: usize = 16;
const MAX_ACTION_KEY_BYTES: usize = 256;
const MAX_ACTION_LABEL_BYTES: usize = 1024;

#[derive(Clone)]
pub(super) struct Control {
    commands: mpsc::Sender<Command>,
    store: Arc<Mutex<Store>>,
}

impl Control {
    pub fn snapshot(&self) -> Snapshot {
        model::from_state(&self.store.lock().expect("notification store poisoned"))
    }

    pub fn toggle_dnd(&self) {
        let _ = self.commands.send(Command::ToggleDnd);
    }

    pub fn clear(&self) {
        let _ = self.commands.send(Command::Clear);
    }

    pub fn dismiss(&self, id: u32) {
        let _ = self.commands.send(Command::Dismiss(id));
    }

    pub fn dismiss_group(&self, notifications: Vec<u32>) {
        let _ = self.commands.send(Command::DismissGroup(notifications));
    }

    pub fn invoke_action(&self, id: u32, key: String, activation_token: Option<String>) {
        let _ = self
            .commands
            .send(Command::InvokeAction(id, key, activation_token));
    }

    pub fn displayed(&self, notifications: Vec<(u32, u64)>) {
        let _ = self.commands.send(Command::Displayed(notifications));
    }
}

enum Command {
    Wake,
    EmitClosed(u32, CloseReason),
    ToggleDnd,
    Clear,
    Dismiss(u32),
    DismissGroup(Vec<u32>),
    InvokeAction(u32, String, Option<String>),
    Displayed(Vec<(u32, u64)>),
}

#[derive(Clone)]
struct Shared {
    store: Arc<Mutex<Store>>,
    changes: async_channel::Sender<()>,
    commands: mpsc::Sender<Command>,
    sounds: Option<Player>,
}

impl Shared {
    fn publish(&self) {
        let _ = self.changes.try_send(());
    }

    fn wake(&self) {
        let _ = self.commands.send(Command::Wake);
    }
}

pub(super) fn start(changes: async_channel::Sender<()>) -> Option<Control> {
    let (commands, receiver) = mpsc::channel();
    let store = Arc::new(Mutex::new(Store::default()));
    let control = Control {
        commands: commands.clone(),
        store: Arc::clone(&store),
    };
    let shared = Shared {
        store,
        changes,
        commands,
        sounds: Player::start(),
    };
    crate::background::spawn("notification-daemon", move || {
        if let Err(error) = run(shared, receiver) {
            eprintln!("varde: notification daemon failed: {error}");
        }
    })
    .then_some(control)
}

fn run(shared: Shared, commands: mpsc::Receiver<Command>) -> zbus::Result<()> {
    let builder = match std::env::var("VARDE_NOTIFICATION_BUS_ADDRESS") {
        Ok(address) => zbus::blocking::connection::Builder::address(address.as_str())?,
        Err(_) => zbus::blocking::connection::Builder::session()?,
    };
    let connection = builder
        .name(SERVICE)?
        .serve_at(PATH, Notifications(shared.clone()))?
        .build()?;
    shared.publish();

    loop {
        let next = shared
            .store
            .lock()
            .expect("notification store poisoned")
            .next_popup_deadline();
        let command = match next {
            Some(deadline) => {
                match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let closed = shared
                            .store
                            .lock()
                            .expect("notification store poisoned")
                            .hide_due_popups(Instant::now());
                        emit_closed(&connection, &closed);
                        shared.publish();
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            None => match commands.recv() {
                Ok(command) => command,
                Err(_) => break,
            },
        };

        match command {
            Command::Wake => continue,
            Command::EmitClosed(id, reason) => {
                emit_closed(&connection, &[(id, reason)]);
            }
            Command::ToggleDnd => {
                let mut store = shared.store.lock().expect("notification store poisoned");
                let dnd = !store.dnd();
                store.set_dnd(dnd);
            }
            Command::Clear => {
                let closed = shared
                    .store
                    .lock()
                    .expect("notification store poisoned")
                    .clear();
                emit_closed(&connection, &closed);
            }
            Command::Dismiss(id) => {
                let closed = shared
                    .store
                    .lock()
                    .expect("notification store poisoned")
                    .close(id)
                    .then_some(vec![(id, CloseReason::Dismissed)])
                    .unwrap_or_default();
                emit_closed(&connection, &closed);
            }
            Command::DismissGroup(notifications) => {
                let mut store = shared.store.lock().expect("notification store poisoned");
                let mut closed = Vec::new();
                for id in notifications {
                    if store.close(id) {
                        closed.push((id, CloseReason::Dismissed));
                    }
                }
                drop(store);
                emit_closed(&connection, &closed);
            }
            Command::InvokeAction(id, key, activation_token) => {
                let mut store = shared.store.lock().expect("notification store poisoned");
                let (action_invoked, closed) = invoke_action(&mut store, id, &key);
                drop(store);
                if action_invoked {
                    emit_action(&connection, id, &key, activation_token.as_deref());
                }
                if closed {
                    emit_closed(&connection, &[(id, CloseReason::Dismissed)]);
                }
            }
            Command::Displayed(notifications) => {
                let now = Instant::now();
                let mut store = shared.store.lock().expect("notification store poisoned");
                for (id, revision) in notifications {
                    store.displayed(id, revision, now);
                }
                continue;
            }
        }
        shared.publish();
    }
    Ok(())
}

fn invoke_action(store: &mut Store, id: u32, key: &str) -> (bool, bool) {
    let resident = store
        .notifications()
        .map(|(notification, _)| notification)
        .find(|notification| notification.id == id)
        .filter(|notification| notification.actions.iter().any(|action| action.key == key))
        .map(|notification| notification.resident);
    match resident {
        Some(true) => (true, false),
        Some(false) => {
            let closed = store.close(id);
            (closed, closed)
        }
        None => (false, false),
    }
}

fn emit_closed(connection: &zbus::blocking::Connection, closed: &[(u32, CloseReason)]) {
    for (id, reason) in closed {
        let _ = connection.emit_signal(
            None::<&str>,
            PATH,
            INTERFACE,
            "NotificationClosed",
            &(*id, *reason as u32),
        );
    }
}

fn emit_action(
    connection: &zbus::blocking::Connection,
    id: u32,
    action: &str,
    activation_token: Option<&str>,
) {
    if let Some(token) = activation_token.filter(|token| !token.is_empty()) {
        let _ = connection.emit_signal(
            None::<&str>,
            PATH,
            INTERFACE,
            "ActivationToken",
            &(id, token),
        );
    }
    let _ = connection.emit_signal(
        None::<&str>,
        PATH,
        INTERFACE,
        "ActionInvoked",
        &(id, action),
    );
}

struct Notifications(Shared);

#[interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[zbus(name = "GetCapabilities")]
    fn get_capabilities(&self) -> Vec<&str> {
        let mut capabilities = vec![
            "actions",
            "body",
            "icon-static",
            "persistence",
            "x-canonical-private-synchronous",
            "x-dunst-stack-tag",
        ];
        if self.0.sounds.is_some() {
            capabilities.push("sound");
        }
        capabilities
    }

    #[zbus(name = "Notify")]
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let sound = sound_hint(&hints);
        let incoming = Incoming {
            replaces_id,
            app_name: truncate_utf8(app_name, MAX_TEXT_BYTES),
            app_icon: truncate_utf8(app_icon, MAX_TEXT_BYTES),
            picture: picture_hint(&hints),
            progress: progress_hint(&hints),
            summary: truncate_utf8(summary, MAX_TEXT_BYTES),
            body: truncate_utf8(body, MAX_BODY_BYTES),
            actions: notification_actions(&actions),
            urgency: urgency(&hints),
            desktop_entry: string_hint(&hints, "desktop-entry")
                .map(|value| truncate_utf8(&value, MAX_TEXT_BYTES))
                .unwrap_or_default(),
            tag: string_hint(&hints, "x-canonical-private-synchronous")
                .or_else(|| string_hint(&hints, "x-dunst-stack-tag"))
                .map(|value| truncate_utf8(&value, MAX_TEXT_BYTES))
                .unwrap_or_default(),
            transient: bool_hint(&hints, "transient"),
            resident: bool_hint(&hints, "resident"),
        };
        let mut store = self.0.store.lock().expect("notification store poisoned");
        let play_sound = !store.dnd();
        let (id, evicted) = store.notify_with_eviction(incoming);
        drop(store);
        if play_sound
            && let Some(sound) = sound
            && let Some(player) = &self.0.sounds
        {
            player.play(sound);
        }
        self.0.publish();
        if let Some((id, reason)) = evicted {
            let _ = self.0.commands.send(Command::EmitClosed(id, reason));
        } else {
            self.0.wake();
        }
        id
    }

    #[zbus(name = "CloseNotification")]
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<()> {
        let closed = self
            .0
            .store
            .lock()
            .expect("notification store poisoned")
            .close(id);
        if !closed {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "unknown notification ID {id}"
            )));
        }
        connection
            .emit_signal(
                None::<&str>,
                PATH,
                INTERFACE,
                "NotificationClosed",
                &(id, CloseReason::Requested as u32),
            )
            .await?;
        self.0.publish();
        self.0.wake();
        Ok(())
    }

    #[zbus(name = "GetServerInformation")]
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("Varde", "Varde", env!("CARGO_PKG_VERSION"), "1.3")
    }

    #[zbus(signal, name = "NotificationClosed")]
    async fn notification_closed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "ActionInvoked")]
    async fn action_invoked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        action: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal, name = "ActivationToken")]
    async fn activation_token(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        activation_token: &str,
    ) -> zbus::Result<()>;
}

fn notification_actions(values: &[String]) -> Vec<Action> {
    values
        .chunks_exact(2)
        .take(MAX_ACTIONS)
        .map(|pair| Action {
            key: truncate_utf8(&pair[0], MAX_ACTION_KEY_BYTES),
            label: truncate_utf8(&pair[1], MAX_ACTION_LABEL_BYTES),
        })
        .collect()
}

fn urgency(hints: &HashMap<String, OwnedValue>) -> Urgency {
    let value = hints.get("urgency");
    let urgency = value
        .and_then(|value| u8::try_from(value).ok().map(i64::from))
        .or_else(|| value.and_then(|value| u32::try_from(value).ok().map(i64::from)))
        .or_else(|| value.and_then(|value| i32::try_from(value).ok().map(i64::from)))
        .unwrap_or(1);
    match urgency {
        i64::MIN..=0 => Urgency::Low,
        1 => Urgency::Normal,
        _ => Urgency::Critical,
    }
}

fn string_hint(hints: &HashMap<String, OwnedValue>, name: &str) -> Option<String> {
    hints
        .get(name)
        .and_then(|value| <&str>::try_from(value).ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn bool_hint(hints: &HashMap<String, OwnedValue>, name: &str) -> bool {
    hints
        .get(name)
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false)
}

fn sound_hint(hints: &HashMap<String, OwnedValue>) -> Option<Sound> {
    if bool_hint(hints, "suppress-sound") {
        return None;
    }
    if let Some(path) = string_hint(hints, "sound-file")
        .filter(|path| path.len() <= MAX_TEXT_BYTES)
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_file())
    {
        return Some(Sound::File(path));
    }
    string_hint(hints, "sound-name")
        .filter(|name| name.len() <= MAX_TEXT_BYTES)
        .map(Sound::Name)
}

fn progress_hint(hints: &HashMap<String, OwnedValue>) -> Option<u8> {
    let value = hints.get("value")?;
    let value = i32::try_from(value)
        .ok()
        .or_else(|| {
            u32::try_from(value)
                .ok()
                .and_then(|value| value.try_into().ok())
        })
        .or_else(|| u8::try_from(value).ok().map(i32::from))?;
    u8::try_from(value).ok().filter(|value| *value <= 100)
}

fn image_hint(hints: &HashMap<String, OwnedValue>) -> Option<Thumbnail> {
    ["image-data", "image_data"]
        .into_iter()
        .find_map(|name| hints.get(name).and_then(image_data))
}

fn picture_hint(hints: &HashMap<String, OwnedValue>) -> Option<Picture> {
    image_hint(hints)
        .map(Picture::Pixels)
        .or_else(|| {
            ["image-path", "image_path"]
                .into_iter()
                .find_map(|name| string_hint(hints, name).and_then(picture_path))
        })
        .or_else(|| {
            hints
                .get("icon_data")
                .and_then(image_data)
                .map(Picture::Pixels)
        })
}

fn picture_path(value: String) -> Option<Picture> {
    if value.len() > MAX_TEXT_BYTES {
        return None;
    }
    if value.starts_with("file://") || std::path::Path::new(&value).is_absolute() {
        image::from_path(&value).map(Picture::Pixels)
    } else {
        Some(Picture::Themed(value))
    }
}

fn image_data(value: &OwnedValue) -> Option<Thumbnail> {
    let structure = zbus::zvariant::Structure::try_from(value.try_clone().ok()?).ok()?;
    let (width, height, rowstride, has_alpha, bits, channels, bytes): (
        i32,
        i32,
        i32,
        bool,
        i32,
        i32,
        Vec<u8>,
    ) = structure.try_into().ok()?;
    image::from_raw(width, height, rowstride, has_alpha, bits, channels, bytes)
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use gdk_pixbuf::{Colorspace, Pixbuf};

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn png(width: i32, height: i32, color: u32) -> std::path::PathBuf {
        let number = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "varde-notification-hint-{}-{number}.png",
            std::process::id()
        ));
        let pixbuf = Pixbuf::new(Colorspace::Rgb, true, 8, width, height).unwrap();
        pixbuf.fill(color);
        pixbuf.savev(&path, "png", &[]).unwrap();
        path
    }

    #[test]
    fn resident_actions_do_not_consume_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            actions: vec![Action {
                key: "reply".into(),
                label: "Reply".into(),
            }],
            resident: true,
            ..Incoming::default()
        });

        assert_eq!(invoke_action(&mut store, id, "reply"), (true, false));
        assert!(
            store
                .notifications()
                .any(|(notification, _)| notification.id == id)
        );
        assert!(store.close(id));
    }

    #[test]
    fn nonresident_actions_consume_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            actions: vec![Action {
                key: "default".into(),
                label: "Open".into(),
            }],
            ..Incoming::default()
        });

        assert_eq!(invoke_action(&mut store, id, "default"), (true, true));
        assert!(
            !store
                .notifications()
                .any(|(notification, _)| notification.id == id)
        );
    }

    #[test]
    fn unknown_actions_do_not_consume_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming::default());

        assert_eq!(invoke_action(&mut store, id, "default"), (false, false));
        assert!(
            store
                .notifications()
                .any(|(notification, _)| notification.id == id)
        );
    }

    #[test]
    fn actions_remain_active_after_the_popup_timeout() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming {
            actions: vec![Action {
                key: "archive".into(),
                label: "Archive".into(),
            }],
            ..Incoming::default()
        });
        assert!(store.displayed(id, 1, now));
        assert!(
            store
                .hide_due_popups(now + std::time::Duration::from_secs(5))
                .is_empty()
        );

        assert_eq!(invoke_action(&mut store, id, "archive"), (true, true));
        assert!(
            !store
                .notifications()
                .any(|(notification, _)| notification.id == id)
        );
    }

    #[test]
    fn parses_action_pairs_in_order_and_ignores_a_trailing_value() {
        let values = ["default", "Open", "reply", "Reply", "mute", "", "odd"].map(String::from);

        assert_eq!(
            notification_actions(&values),
            vec![
                Action {
                    key: "default".into(),
                    label: "Open".into(),
                },
                Action {
                    key: "reply".into(),
                    label: "Reply".into(),
                },
                Action {
                    key: "mute".into(),
                    label: "".into(),
                },
            ]
        );
    }

    #[test]
    fn bounds_action_count_and_text() {
        let mut values = Vec::new();
        for index in 0..MAX_ACTIONS + 2 {
            values.push(if index == 0 {
                "k".repeat(MAX_ACTION_KEY_BYTES + 1)
            } else {
                format!("key-{index}")
            });
            values.push("l".repeat(MAX_ACTION_LABEL_BYTES + 1));
        }

        let actions = notification_actions(&values);

        assert_eq!(actions.len(), MAX_ACTIONS);
        assert_eq!(actions[0].key.len(), MAX_ACTION_KEY_BYTES);
        assert_eq!(actions[0].label.len(), MAX_ACTION_LABEL_BYTES);
        assert_eq!(
            actions.last().unwrap().key,
            format!("key-{}", MAX_ACTIONS - 1)
        );
    }

    #[test]
    fn accepts_standard_and_chromium_urgency_types() {
        for (value, expected) in [
            (OwnedValue::from(0_u8), Urgency::Low),
            (OwnedValue::from(1_u32), Urgency::Normal),
            (OwnedValue::from(2_i32), Urgency::Critical),
        ] {
            let hints = HashMap::from([("urgency".into(), value)]);
            assert_eq!(urgency(&hints), expected);
        }
    }

    #[test]
    fn canonical_stack_tag_takes_priority() {
        let hints = HashMap::from([
            (
                "x-canonical-private-synchronous".into(),
                OwnedValue::from(zbus::zvariant::Str::from("canonical")),
            ),
            (
                "x-dunst-stack-tag".into(),
                OwnedValue::from(zbus::zvariant::Str::from("dunst")),
            ),
        ]);
        assert_eq!(
            string_hint(&hints, "x-canonical-private-synchronous"),
            Some("canonical".into())
        );
    }

    #[test]
    fn reads_the_standard_image_path_hint() {
        let hints = HashMap::from([(
            "image-path".into(),
            OwnedValue::from(zbus::zvariant::Str::from("/tmp/picture.png")),
        )]);
        let icon = string_hint(&hints, "image-path")
            .or_else(|| string_hint(&hints, "image_path"))
            .unwrap_or_else(|| "fallback".into());
        assert_eq!(icon, "/tmp/picture.png");
    }

    #[test]
    fn accepts_percentage_progress_hints() {
        for value in [
            OwnedValue::from(42_i32),
            OwnedValue::from(42_u32),
            OwnedValue::from(42_u8),
        ] {
            assert_eq!(
                progress_hint(&HashMap::from([("value".into(), value)])),
                Some(42)
            );
        }
        for value in [OwnedValue::from(-1_i32), OwnedValue::from(101_i32)] {
            assert_eq!(
                progress_hint(&HashMap::from([("value".into(), value)])),
                None
            );
        }
    }

    fn raw_image_value(
        width: i32,
        height: i32,
        rowstride: i32,
        alpha: bool,
        bits: i32,
        channels: i32,
        bytes: Vec<u8>,
    ) -> OwnedValue {
        OwnedValue::try_from(zbus::zvariant::Value::Structure(
            (width, height, rowstride, alpha, bits, channels, bytes).into(),
        ))
        .unwrap()
    }

    fn picture_thumbnail(hints: &HashMap<String, OwnedValue>) -> Thumbnail {
        match picture_hint(hints).unwrap() {
            Picture::Pixels(thumbnail) => thumbnail,
            Picture::Themed(icon) => panic!("expected thumbnail, got icon {icon}"),
        }
    }

    #[test]
    fn accepts_standard_raw_images_and_hint_aliases() {
        for name in ["image-data", "image_data", "icon_data"] {
            let hints = HashMap::from([(
                name.into(),
                raw_image_value(2, 1, 8, true, 8, 4, vec![0; 8]),
            )]);
            let image = picture_thumbnail(&hints);
            assert_eq!((image.width, image.height, image.rowstride), (2, 1, 8));
            assert_eq!(image.bytes.len(), 8);
        }
    }

    #[test]
    fn rejects_invalid_or_unreasonably_large_raw_images() {
        for value in [
            raw_image_value(2, 1, 7, true, 8, 4, vec![0; 8]),
            raw_image_value(2, 1, 8, true, 16, 4, vec![0; 8]),
            raw_image_value(2, 1, 8, false, 4, 4, vec![0; 8]),
            raw_image_value(2, 2, 8, true, 8, 4, vec![0; 8]),
            raw_image_value(20_000, 1, 80_000, true, 8, 4, vec![]),
        ] {
            assert!(image_data(&value).is_none());
        }
    }

    #[test]
    fn standard_raw_image_takes_priority_over_deprecated_aliases() {
        let hints = HashMap::from([
            (
                "image-data".into(),
                raw_image_value(1, 1, 4, true, 8, 4, vec![1; 4]),
            ),
            (
                "image_data".into(),
                raw_image_value(1, 1, 4, true, 8, 4, vec![2; 4]),
            ),
        ]);
        assert_eq!(image_hint(&hints).unwrap().bytes.as_ref(), &[1; 4]);
    }

    #[test]
    fn valid_raw_images_take_priority_over_paths() {
        let path = png(1, 1, 0x112233ff);
        let hints = HashMap::from([
            (
                "image-data".into(),
                raw_image_value(1, 1, 4, true, 8, 4, vec![9, 8, 7, 6]),
            ),
            (
                "image-path".into(),
                OwnedValue::from(zbus::zvariant::Str::from(path.to_str().unwrap())),
            ),
        ]);
        assert_eq!(picture_thumbnail(&hints).bytes.as_ref(), &[9, 8, 7, 6]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn image_paths_take_priority_over_deprecated_icon_data() {
        let path = png(1, 1, 0x112233ff);
        let hints = HashMap::from([
            (
                "image-path".into(),
                OwnedValue::from(zbus::zvariant::Str::from(path.to_str().unwrap())),
            ),
            (
                "icon_data".into(),
                raw_image_value(1, 1, 4, true, 8, 4, vec![9, 8, 7, 6]),
            ),
        ]);

        assert_eq!(
            picture_thumbnail(&hints).bytes.as_ref(),
            &[0x11, 0x22, 0x33, 0xff]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn accepts_themed_image_path_names() {
        let hints = HashMap::from([(
            "image-path".into(),
            OwnedValue::from(zbus::zvariant::Str::from("dialog-information")),
        )]);

        assert_eq!(
            picture_hint(&hints),
            Some(Picture::Themed("dialog-information".into()))
        );
    }

    #[test]
    fn invalid_sources_fall_through_in_alias_order() {
        let path = png(1, 1, 0x112233ff);
        let hints = HashMap::from([
            (
                "image-data".into(),
                raw_image_value(1, 1, 3, true, 8, 4, vec![0; 4]),
            ),
            (
                "image-path".into(),
                OwnedValue::from(zbus::zvariant::Str::from("/missing/image.png")),
            ),
            (
                "image_path".into(),
                OwnedValue::from(zbus::zvariant::Str::from(path.to_str().unwrap())),
            ),
        ]);
        assert_eq!(
            picture_thumbnail(&hints).bytes.as_ref(),
            &[0x11, 0x22, 0x33, 0xff]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn ignores_overlong_image_paths() {
        let hints = HashMap::from([(
            "image-path".into(),
            OwnedValue::from(zbus::zvariant::Str::from("x".repeat(MAX_TEXT_BYTES + 1))),
        )]);
        assert!(picture_hint(&hints).is_none());
    }

    #[test]
    fn skips_invalid_aliases_before_the_first_valid_raw_image() {
        let hints = HashMap::from([
            (
                "image-data".into(),
                raw_image_value(1, 1, 3, true, 8, 4, vec![1; 4]),
            ),
            (
                "image_data".into(),
                raw_image_value(1, 1, 4, true, 8, 4, vec![2; 4]),
            ),
        ]);
        assert_eq!(image_hint(&hints).unwrap().bytes.as_ref(), &[2; 4]);
    }

    #[test]
    fn selects_and_suppresses_notification_sounds() {
        let path = png(1, 1, 0x112233ff);
        let file = OwnedValue::from(zbus::zvariant::Str::from(path.to_str().unwrap()));
        let name = || OwnedValue::from(zbus::zvariant::Str::from("message-new-instant"));

        assert_eq!(
            sound_hint(&HashMap::from([("sound-name".into(), name())])),
            Some(Sound::Name("message-new-instant".into()))
        );
        assert_eq!(
            sound_hint(&HashMap::from([
                ("sound-file".into(), file.try_clone().unwrap()),
                ("sound-name".into(), name()),
            ])),
            Some(Sound::File(path.clone()))
        );
        assert_eq!(
            sound_hint(&HashMap::from([
                ("sound-name".into(), name()),
                ("suppress-sound".into(), OwnedValue::from(true)),
            ])),
            None
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_relative_and_missing_sound_files() {
        for path in ["relative.oga", "/missing/notification.oga"] {
            let hints = HashMap::from([(
                "sound-file".into(),
                OwnedValue::from(zbus::zvariant::Str::from(path)),
            )]);
            assert_eq!(sound_hint(&hints), None);
        }
    }

    #[test]
    fn truncates_utf8_without_splitting_code_points() {
        assert_eq!(truncate_utf8("abéz", 3), "ab");
        assert_eq!(truncate_utf8("abéz", 4), "abé");
        assert_eq!(truncate_utf8("short", MAX_TEXT_BYTES), "short");
        assert_eq!(
            truncate_utf8(&"x".repeat(MAX_TEXT_BYTES + 1), MAX_TEXT_BYTES).len(),
            MAX_TEXT_BYTES
        );
        assert_eq!(
            truncate_utf8(&"x".repeat(MAX_BODY_BYTES + 1), MAX_BODY_BYTES).len(),
            MAX_BODY_BYTES
        );
    }
}
