use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

use zbus::{interface, zvariant::OwnedValue};

use super::{
    model::{self, Snapshot},
    state::{CloseReason, ImageData, Incoming, Store, Urgency},
};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";
const MAX_IMAGE_DIMENSION: i32 = 16_384;
const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

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

    pub fn dismiss(&self, id: u32, active: bool) {
        let command = if active {
            Command::Dismiss(id)
        } else {
            Command::RemoveHistory(id)
        };
        let _ = self.commands.send(command);
    }

    pub fn dismiss_group(&self, notifications: Vec<(u32, bool)>) {
        let _ = self.commands.send(Command::DismissGroup(notifications));
    }

    pub fn invoke_default(&self, id: u32) {
        let _ = self.commands.send(Command::InvokeDefault(id));
    }

    pub fn displayed(&self, notifications: Vec<(u32, u64)>) {
        let _ = self.commands.send(Command::Displayed(notifications));
    }
}

enum Command {
    Wake,
    ToggleDnd,
    Clear,
    Dismiss(u32),
    DismissGroup(Vec<(u32, bool)>),
    InvokeDefault(u32),
    Displayed(Vec<(u32, u64)>),
    RemoveHistory(u32),
}

#[derive(Clone)]
struct Shared {
    store: Arc<Mutex<Store>>,
    changes: async_channel::Sender<()>,
    commands: mpsc::Sender<Command>,
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
    };
    crate::background::spawn("notification-daemon", move || {
        if let Err(error) = run(shared, receiver) {
            eprintln!("shell: notification daemon failed: {error}");
        }
    })
    .then_some(control)
}

fn run(shared: Shared, commands: mpsc::Receiver<Command>) -> zbus::Result<()> {
    let builder = match std::env::var("SHELL_NOTIFICATION_BUS_ADDRESS") {
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
            .next_expiration();
        let command = match next {
            Some(deadline) => {
                match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(command) => command,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let closed = shared
                            .store
                            .lock()
                            .expect("notification store poisoned")
                            .expire(Instant::now());
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
                    .close(id, false)
                    .then_some(vec![(id, CloseReason::Dismissed)])
                    .unwrap_or_default();
                emit_closed(&connection, &closed);
            }
            Command::DismissGroup(notifications) => {
                let mut store = shared.store.lock().expect("notification store poisoned");
                let mut closed = Vec::new();
                for (id, active) in notifications {
                    if active {
                        if store.close(id, false) {
                            closed.push((id, CloseReason::Dismissed));
                        }
                    } else {
                        store.remove_history(id);
                    }
                }
                drop(store);
                emit_closed(&connection, &closed);
            }
            Command::InvokeDefault(id) => {
                let mut store = shared.store.lock().expect("notification store poisoned");
                let (action_invoked, active_closed) = invoke_default(&mut store, id);
                drop(store);
                if action_invoked {
                    emit_action(&connection, id, "default");
                }
                if active_closed {
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
            Command::RemoveHistory(id) => {
                shared
                    .store
                    .lock()
                    .expect("notification store poisoned")
                    .remove_history(id);
            }
        }
        shared.publish();
    }
    Ok(())
}

fn invoke_default(store: &mut Store, id: u32) -> (bool, bool) {
    let actionable = store.has_default_action(id);
    let active = store.is_active(id);
    if store.is_resident(id) {
        return (actionable, false);
    }
    let removed = if active {
        store.close(id, false)
    } else {
        store.remove_history(id)
    };
    (actionable && removed, active && removed)
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

fn emit_action(connection: &zbus::blocking::Connection, id: u32, action: &str) {
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
        // TODO: Render and invoke named notification actions.
        vec![
            "actions",
            "body",
            "persistence",
            "x-canonical-private-synchronous",
            "x-dunst-stack-tag",
        ]
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
        expire_timeout: i32,
    ) -> u32 {
        let image_data = image_hint(&hints);
        let image = string_hint(&hints, "image-path")
            .or_else(|| string_hint(&hints, "image_path"))
            .unwrap_or_default();
        let incoming = Incoming {
            replaces_id,
            app_name: app_name.into(),
            app_icon: app_icon.into(),
            image,
            image_data,
            progress: progress_hint(&hints),
            summary: summary.into(),
            body: body.into(),
            has_default_action: actions.chunks_exact(2).any(|pair| pair[0] == "default"),
            urgency: urgency(&hints),
            desktop_entry: string_hint(&hints, "desktop-entry").unwrap_or_default(),
            tag: string_hint(&hints, "x-canonical-private-synchronous")
                .or_else(|| string_hint(&hints, "x-dunst-stack-tag"))
                .unwrap_or_default(),
            transient: bool_hint(&hints, "transient"),
            resident: bool_hint(&hints, "resident"),
            timeout_ms: expire_timeout,
        };
        let id = self
            .0
            .store
            .lock()
            .expect("notification store poisoned")
            .notify(incoming);
        self.0.publish();
        self.0.wake();
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
            .close(id, false);
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
        ("shell", "jochim", env!("CARGO_PKG_VERSION"), "1.2")
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

fn image_hint(hints: &HashMap<String, OwnedValue>) -> Option<ImageData> {
    ["image-data", "image_data", "icon_data"]
        .into_iter()
        .find_map(|name| hints.get(name).and_then(image_data))
}

fn image_data(value: &OwnedValue) -> Option<ImageData> {
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
    let channels = usize::try_from(channels).ok()?;
    let rowstride = usize::try_from(rowstride).ok()?;
    let height_usize = usize::try_from(height).ok()?;
    let width_usize = usize::try_from(width).ok()?;
    let expected_channels = if has_alpha { 4 } else { 3 };
    let minimum_stride = width_usize.checked_mul(channels)?;
    let required = rowstride.checked_mul(height_usize)?;
    if width <= 0
        || height <= 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || bits != 8
        || channels != expected_channels
        || rowstride < minimum_stride
        || required > MAX_IMAGE_BYTES
        || bytes.len() < required
    {
        return None;
    }
    Some(ImageData {
        width,
        height,
        rowstride,
        has_alpha,
        bytes: Arc::from(&bytes[..required]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_actions_do_not_consume_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            has_default_action: true,
            resident: true,
            ..Incoming::default()
        });

        assert_eq!(invoke_default(&mut store, id), (true, false));
        assert!(store.is_active(id));
        assert!(store.close(id, false));
    }

    #[test]
    fn nonresident_actions_consume_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            has_default_action: true,
            ..Incoming::default()
        });

        assert_eq!(invoke_default(&mut store, id), (true, true));
        assert!(!store.is_active(id));
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

    #[test]
    fn accepts_standard_raw_images_and_hint_aliases() {
        for name in ["image-data", "image_data", "icon_data"] {
            let hints = HashMap::from([(
                name.into(),
                raw_image_value(2, 1, 8, true, 8, 4, vec![0; 8]),
            )]);
            let image = image_hint(&hints).unwrap();
            assert_eq!((image.width, image.height, image.rowstride), (2, 1, 8));
            assert!(image.has_alpha);
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
}
