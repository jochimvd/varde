use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use async_channel::Sender;
use zbus::{
    MatchRule, Message,
    blocking::{Connection, MessageIterator, Proxy, fdo::DBusProxy},
    fdo::{RequestNameFlags, RequestNameReply},
    interface,
    message::{Header, Type},
    names::BusName,
    object_server::SignalEmitter,
};

use super::model::{Event, Item, ItemId, select_pixmap, tooltip};
use crate::background;

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";

#[derive(Clone, Default)]
pub(super) struct SharedConnection(Arc<Mutex<Option<Connection>>>);

impl SharedConnection {
    fn get(&self) -> Option<Connection> {
        self.0
            .lock()
            .expect("tray connection lock poisoned")
            .clone()
    }

    fn set(&self, connection: Option<Connection>) {
        *self.0.lock().expect("tray connection lock poisoned") = connection;
    }
}

/// Items whose thread is already running, so hosting the same item twice is a
/// no-op. The generation prevents an old worker from changing a replacement
/// registration after its item was removed and started again.
type Started = Arc<Mutex<StartedItems>>;

#[derive(Default)]
struct StartedItems {
    items: HashMap<ItemId, StartedItem>,
    next_generation: u64,
}

struct StartedItem {
    generation: u64,
    registered: Option<RegisteredItem>,
    owner: Option<String>,
    signals: mpsc::Sender<ItemSignal>,
}

enum ItemSignal {
    Refresh,
    Stop,
}

pub(super) fn run(events: Sender<Event>, shared: SharedConnection) {
    let started: Started = Arc::new(Mutex::new(StartedItems::default()));
    loop {
        let Ok(connection) = Connection::session() else {
            thread::sleep(background::RETRY_DELAY);
            continue;
        };
        let watcher = Watcher::new(events.clone(), started.clone(), shared.clone());
        if connection
            .object_server()
            .at(WATCHER_PATH, watcher)
            .is_err()
        {
            thread::sleep(background::RETRY_DELAY);
            continue;
        }
        let Ok(messages) = signal_messages(&connection) else {
            thread::sleep(background::RETRY_DELAY);
            continue;
        };

        shared.set(Some(connection.clone()));
        let router_started = started.clone();
        let router_connection = connection.clone();
        if !background::spawn("tray-signals", move || {
            route_signals(messages, &router_started);
            let _ = router_connection.close();
        }) {
            shared.set(None);
            wake_all(&started);
            let _ = connection.close();
            thread::sleep(background::RETRY_DELAY);
            continue;
        }

        while !connection.is_closed() {
            host(&connection, &events, &started, &shared);
            if !connection.is_closed() {
                thread::sleep(background::RETRY_DELAY);
            }
        }
        shared.set(None);
        wake_all(&started);
        thread::sleep(background::RETRY_DELAY);
    }
}

/// Owns the watcher name when it is free and hosts whoever owns it otherwise.
/// Returns once the connection fails or the hosted watcher disappears, so the
/// caller can claim the name that watcher left behind.
fn host(
    connection: &Connection,
    events: &Sender<Event>,
    started: &Started,
    shared: &SharedConnection,
) {
    match connection.request_name_with_flags(WATCHER_NAME, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            register_host(connection);
            // The name and the served object are lost with the bus, so waiting on
            // the connection is what tells the caller to build both again.
            connection.closed();
        }
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(_) => {
            host_existing_watcher(connection, events, started, shared);
        }
    }
}

fn register_host(connection: &Connection) {
    let host_name = format!("org.kde.StatusNotifierHost-{}-0", std::process::id());
    let registered_name = match connection
        .request_name_with_flags(host_name.as_str(), RequestNameFlags::DoNotQueue.into())
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => host_name,
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(_) => connection
            .unique_name()
            .map(ToString::to_string)
            .unwrap_or_default(),
    };
    if registered_name.is_empty() {
        return;
    }

    if let Ok(watcher) = Proxy::new(connection, WATCHER_NAME, WATCHER_PATH, WATCHER_INTERFACE) {
        let _ = watcher.call_noreply("RegisterStatusNotifierHost", &registered_name);
    }
}

fn host_existing_watcher(
    connection: &Connection,
    events: &Sender<Event>,
    started: &Started,
    shared: &SharedConnection,
) {
    // Subscribed before the item list is read so nothing registered in between is missed.
    let Ok((messages, rules)) = watcher_messages(connection) else {
        return;
    };
    // The watcher can exit before that subscription lands, which emits no signal
    // for this iterator to see and would leave its free name unclaimed.
    if service_is_gone(connection, WATCHER_NAME) {
        remove_match_rules(connection, rules);
        return;
    }
    let Some(watcher_owner) = service_owner(connection, WATCHER_NAME) else {
        remove_match_rules(connection, rules);
        return;
    };
    register_host(connection);

    if let Ok(watcher) = Proxy::new(connection, WATCHER_NAME, WATCHER_PATH, WATCHER_INTERFACE)
        && let Ok(registered) = watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems")
    {
        for item in registered
            .iter()
            .filter_map(|registration| ItemId::from_registration(registration))
        {
            start_item(item, events.clone(), started, shared, None);
        }
    }

    for message in messages.flatten() {
        let header = message.header();
        let Some(member) = header.member() else {
            continue;
        };
        match member.as_str() {
            "NameOwnerChanged"
                if message
                    .body()
                    .deserialize::<(String, String, String)>()
                    .is_ok_and(|(name, _, _)| name == WATCHER_NAME) =>
            {
                break;
            }
            "StatusNotifierItemRegistered" | "StatusNotifierItemUnregistered"
                if header
                    .sender()
                    .is_some_and(|sender| sender.as_str() == watcher_owner)
                    && header
                        .interface()
                        .is_some_and(|interface| interface.as_str() == WATCHER_INTERFACE)
                    && header
                        .path()
                        .is_some_and(|path| path.as_str() == WATCHER_PATH) =>
            {
                let Ok(registration) = message.body().deserialize::<String>() else {
                    continue;
                };
                let Some(item) = ItemId::from_registration(&registration) else {
                    continue;
                };
                if member == "StatusNotifierItemRegistered" {
                    start_item(item, events.clone(), started, shared, None);
                } else {
                    cancel_item(started, &item, events);
                }
            }
            _ => {}
        }
    }
    remove_match_rules(connection, rules);
}

/// Signals from the hosted watcher, plus the loss of its bus name.
fn watcher_messages(
    connection: &Connection,
) -> zbus::Result<(MessageIterator, Vec<MatchRule<'static>>)> {
    let messages = MessageIterator::from(connection.clone());
    let dbus = DBusProxy::new(connection)?;
    let rules = vec![
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(WATCHER_NAME)?
            .interface(WATCHER_INTERFACE)?
            .path(WATCHER_PATH)?
            .build(),
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS_SERVICE)?
            .interface(DBUS_INTERFACE)?
            .member("NameOwnerChanged")?
            .add_arg(WATCHER_NAME)?
            .build(),
    ];
    for rule in &rules {
        dbus.add_match_rule(rule.clone())?;
    }
    Ok((messages, rules))
}

fn remove_match_rules(connection: &Connection, rules: Vec<MatchRule<'static>>) {
    let Ok(dbus) = DBusProxy::new(connection) else {
        return;
    };
    for rule in rules {
        let _ = dbus.remove_match_rule(rule);
    }
}

/// One stream receives all item updates and owner changes, then forwards only
/// the relevant wake-up to each item worker.
fn signal_messages(connection: &Connection) -> zbus::Result<MessageIterator> {
    let messages = MessageIterator::from(connection.clone());
    let dbus = DBusProxy::new(connection)?;
    dbus.add_match_rule(
        MatchRule::builder()
            .msg_type(Type::Signal)
            .interface(ITEM_INTERFACE)?
            .build(),
    )?;
    dbus.add_match_rule(
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS_SERVICE)?
            .interface(DBUS_INTERFACE)?
            .member("NameOwnerChanged")?
            .build(),
    )?;
    Ok(messages)
}

fn route_signals(messages: MessageIterator, started: &Started) {
    for message in messages.flatten() {
        let header = message.header();
        let Some(interface) = header.interface() else {
            continue;
        };
        if interface.as_str() == ITEM_INTERFACE {
            route_item_signal(&message, started);
        } else if interface.as_str() == DBUS_INTERFACE
            && header
                .member()
                .is_some_and(|member| member.as_str() == "NameOwnerChanged")
        {
            route_owner_change(&message, started);
        }
    }
    wake_all(started);
}

fn route_item_signal(message: &Message, started: &Started) {
    let header = message.header();
    let Some(member) = header.member() else {
        return;
    };
    if !matches!(
        member.as_str(),
        "NewIcon" | "NewAttentionIcon" | "NewStatus" | "NewTitle" | "NewToolTip" | "NewMenu"
    ) {
        return;
    }
    let (Some(sender), Some(path)) = (header.sender(), header.path()) else {
        return;
    };
    let started = started.lock().expect("tray item lock poisoned");
    for (id, item) in &started.items {
        if id.path == path.as_str() && item.owner.as_deref() == Some(sender.as_str()) {
            let _ = item.signals.send(ItemSignal::Refresh);
        }
    }
}

fn route_owner_change(message: &Message, started: &Started) {
    let Ok(change) = message.body().deserialize::<(String, String, String)>() else {
        return;
    };
    let (name, _, new_owner) = &change;
    let mut started = started.lock().expect("tray item lock poisoned");
    for (id, item) in &mut started.items {
        if id.service != *name {
            continue;
        }
        if name_owner_lost(&change, &id.service) {
            let _ = item.signals.send(ItemSignal::Stop);
        } else {
            item.owner = Some(new_owner.clone());
            let _ = item.signals.send(ItemSignal::Refresh);
        }
    }
}

fn wake_all(started: &Started) {
    let started = started.lock().expect("tray item lock poisoned");
    for item in started.items.values() {
        let _ = item.signals.send(ItemSignal::Refresh);
    }
}

fn name_owner_lost(args: &(String, String, String), name: &str) -> bool {
    let (changed, _, new_owner) = args;
    changed == name && new_owner.is_empty()
}

#[derive(Clone)]
struct Watcher {
    events: Sender<Event>,
    started: Started,
    shared: SharedConnection,
    host_registered: Arc<Mutex<bool>>,
}

impl Watcher {
    fn new(events: Sender<Event>, started: Started, shared: SharedConnection) -> Self {
        Self {
            events,
            started,
            shared,
            host_registered: Arc::new(Mutex::new(false)),
        }
    }

    fn register_host(&self) -> bool {
        let mut registered = self
            .host_registered
            .lock()
            .expect("tray host lock poisoned");
        if *registered {
            false
        } else {
            *registered = true;
            true
        }
    }

    fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.host_registered, &other.host_registered)
    }
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let Some(sender) = header.sender() else {
            return;
        };
        let Some(item) = ItemId::parse(service, sender.as_str()) else {
            return;
        };
        let registered = RegisteredItem {
            watcher: self.clone(),
            emitter: emitter.to_owned(),
        };
        if !start_item(
            item.clone(),
            self.events.clone(),
            &self.started,
            &self.shared,
            Some(registered),
        ) {
            return;
        }

        let registration = item.registration();
        let _ = zbus::block_on(Self::status_notifier_item_registered(
            &emitter,
            &registration,
        ));
        let _ = zbus::block_on(self.registered_status_notifier_items_changed(&emitter));
    }

    fn register_status_notifier_host(
        &self,
        _service: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        if self.register_host() {
            let _ = zbus::block_on(Self::status_notifier_host_registered(&emitter));
            let _ = zbus::block_on(self.is_status_notifier_host_registered_changed(&emitter));
        }
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.started
            .lock()
            .expect("tray item lock poisoned")
            .items
            .iter()
            .filter(|(_, item)| item.registered.is_some())
            .map(|(id, _)| id.registration())
            .collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        *self
            .host_registered
            .lock()
            .expect("tray host lock poisoned")
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct RegisteredItem {
    watcher: Watcher,
    emitter: SignalEmitter<'static>,
}

/// Starts following the item unless it is already followed, and reports whether
/// this call is what registered it with `registered`'s watcher.
fn start_item(
    id: ItemId,
    events: Sender<Event>,
    started: &Started,
    shared: &SharedConnection,
    registered: Option<RegisteredItem>,
) -> bool {
    let (generation, newly_registered, signals) = {
        let mut started = started.lock().expect("tray item lock poisoned");
        if let Some(current) = started.items.get_mut(&id) {
            let Some(registered) = registered else {
                return false;
            };
            if current
                .registered
                .as_ref()
                .is_some_and(|current| current.watcher.same_instance(&registered.watcher))
            {
                return false;
            }
            current.registered = Some(registered);
            return true;
        }

        started.next_generation = started.next_generation.wrapping_add(1);
        let generation = started.next_generation;
        let newly_registered = registered.is_some();
        let (signal_sender, signals) = mpsc::channel();
        started.items.insert(
            id.clone(),
            StartedItem {
                generation,
                registered,
                owner: None,
                signals: signal_sender,
            },
        );
        (generation, newly_registered, signals)
    };

    let worker_events = events.clone();
    let worker_started = Arc::clone(started);
    let worker_shared = shared.clone();
    let worker_id = id.clone();
    if background::spawn("tray-item", move || {
        run_item(
            worker_id,
            generation,
            worker_events,
            worker_started,
            worker_shared,
            signals,
        )
    }) {
        return newly_registered;
    }

    // Nothing was announced yet, so the reservation is all there is to undo.
    remove_generation(started, &id, generation, None);
    false
}

fn run_item(
    id: ItemId,
    generation: u64,
    events: Sender<Event>,
    started: Started,
    shared: SharedConnection,
    signals: mpsc::Receiver<ItemSignal>,
) {
    while is_current(&started, &id, generation) {
        match refresh_item(&id, generation, &events, &started, &shared) {
            RefreshResult::Gone => break,
            RefreshResult::Loaded => match signals.recv() {
                Ok(ItemSignal::Refresh) => {}
                Ok(ItemSignal::Stop) | Err(_) => break,
            },
            RefreshResult::Retry => match signals.recv_timeout(background::RETRY_DELAY) {
                Ok(ItemSignal::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(ItemSignal::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            },
        }
    }

    finish_item(&id, generation, &events, &started);
}

enum RefreshResult {
    Loaded,
    Retry,
    Gone,
}

fn finish_item(id: &ItemId, generation: u64, events: &Sender<Event>, started: &Started) {
    let Some(registered) = remove_generation(started, id, generation, Some(events)) else {
        return;
    };
    // Holding a registration is what says this watcher announced the item.
    if let Some(registered) = registered {
        let item = id.registration();
        let _ = zbus::block_on(Watcher::status_notifier_item_unregistered(
            &registered.emitter,
            &item,
        ));
        let _ = zbus::block_on(
            registered
                .watcher
                .registered_status_notifier_items_changed(&registered.emitter),
        );
    }
}

fn refresh_item(
    id: &ItemId,
    generation: u64,
    events: &Sender<Event>,
    started: &Started,
    shared: &SharedConnection,
) -> RefreshResult {
    let Some(connection) = shared.get() else {
        return RefreshResult::Retry;
    };
    let Some(owner) = service_owner(&connection, &id.service) else {
        return if service_is_gone(&connection, &id.service) {
            RefreshResult::Gone
        } else {
            RefreshResult::Retry
        };
    };
    if !set_owner(started, id, generation, owner) {
        return RefreshResult::Gone;
    }
    let Ok(item) = load_item(&connection, id) else {
        return if service_is_gone(&connection, &id.service) {
            RefreshResult::Gone
        } else {
            RefreshResult::Retry
        };
    };
    if send_if_current(started, id, generation, events, item) {
        RefreshResult::Loaded
    } else {
        RefreshResult::Gone
    }
}

fn service_owner(connection: &Connection, service: &str) -> Option<String> {
    let dbus = DBusProxy::new(connection).ok()?;
    let name = BusName::try_from(service).ok()?;
    dbus.get_name_owner(name)
        .ok()
        .map(|owner| owner.to_string())
}

fn service_is_gone(connection: &Connection, service: &str) -> bool {
    let Ok(dbus) = DBusProxy::new(connection) else {
        return false;
    };
    let Ok(name) = BusName::try_from(service) else {
        return false;
    };
    matches!(dbus.name_has_owner(name), Ok(false))
}

fn is_current(started: &Started, id: &ItemId, generation: u64) -> bool {
    started
        .lock()
        .expect("tray item lock poisoned")
        .items
        .get(id)
        .is_some_and(|item| item.generation == generation)
}

fn set_owner(started: &Started, id: &ItemId, generation: u64, owner: String) -> bool {
    let mut started = started.lock().expect("tray item lock poisoned");
    let Some(item) = started
        .items
        .get_mut(id)
        .filter(|item| item.generation == generation)
    else {
        return false;
    };
    item.owner = Some(owner);
    true
}

fn send_if_current(
    started: &Started,
    id: &ItemId,
    generation: u64,
    events: &Sender<Event>,
    item: Item,
) -> bool {
    let started = started.lock().expect("tray item lock poisoned");
    started
        .items
        .get(id)
        .is_some_and(|current| current.generation == generation)
        && events.send_blocking(Event::Upsert(item)).is_ok()
}

fn cancel_item(started: &Started, id: &ItemId, events: &Sender<Event>) {
    let signals = {
        let mut started = started.lock().expect("tray item lock poisoned");
        let Some(item) = started.items.remove(id) else {
            return;
        };
        let _ = events.try_send(Event::Remove(id.clone()));
        item.signals
    };
    let _ = signals.send(ItemSignal::Stop);
}

/// Removes only the worker generation making the request. The outer `Option`
/// says whether it was still current; the inner one is its watcher registration.
fn remove_generation(
    started: &Started,
    id: &ItemId,
    generation: u64,
    events: Option<&Sender<Event>>,
) -> Option<Option<RegisteredItem>> {
    let mut started = started.lock().expect("tray item lock poisoned");
    if started
        .items
        .get(id)
        .is_none_or(|item| item.generation != generation)
    {
        return None;
    }
    let registered = started.items.remove(id)?.registered;
    if let Some(events) = events {
        let _ = events.try_send(Event::Remove(id.clone()));
    }
    Some(registered)
}

fn load_item(connection: &Connection, id: &ItemId) -> zbus::Result<Item> {
    let proxy = Proxy::new(
        connection,
        id.service.as_str(),
        id.path.as_str(),
        ITEM_INTERFACE,
    )?;
    let status = proxy
        .get_property::<String>("Status")
        .unwrap_or_else(|_| "Active".into());
    let title = proxy
        .get_property::<String>("Title")
        .unwrap_or_else(|_| id.service.clone());
    let icon_name = if status == "NeedsAttention" {
        proxy
            .get_property::<String>("AttentionIconName")
            .or_else(|_| proxy.get_property("IconName"))
            .unwrap_or_default()
    } else {
        proxy.get_property("IconName").unwrap_or_default()
    };
    let pixmaps = if status == "NeedsAttention" {
        proxy
            .get_property::<Vec<(i32, i32, Vec<u8>)>>("AttentionIconPixmap")
            .or_else(|_| proxy.get_property("IconPixmap"))
            .unwrap_or_default()
    } else {
        proxy.get_property("IconPixmap").unwrap_or_default()
    };
    let tooltip = proxy
        .get_property::<(String, Vec<(i32, i32, Vec<u8>)>, String, String)>("ToolTip")
        .ok()
        .and_then(|(_, _, title, text)| tooltip(&title, &text))
        .or_else(|| tooltip(&title, ""));

    Ok(Item {
        id: id.clone(),
        status,
        tooltip,
        icon_name,
        pixmap: select_pixmap(pixmaps),
        item_is_menu: proxy.get_property("ItemIsMenu").unwrap_or(false),
    })
}

pub(super) fn call_item(
    shared: &SharedConnection,
    id: &ItemId,
    method: &'static str,
    position: (i32, i32),
) {
    let shared = shared.clone();
    let id = id.clone();
    background::spawn("tray-item-call", move || {
        let Some(connection) = shared.get() else {
            return;
        };
        let Ok(proxy) = Proxy::new(
            &connection,
            id.service.as_str(),
            id.path.as_str(),
            ITEM_INTERFACE,
        ) else {
            return;
        };
        let _ = proxy.call_noreply(method, &position);
    });
}

pub(super) fn call_scroll(
    shared: &SharedConnection,
    id: &ItemId,
    delta: i32,
    orientation: &'static str,
) {
    let shared = shared.clone();
    let id = id.clone();
    background::spawn("tray-item-scroll", move || {
        let Some(connection) = shared.get() else {
            return;
        };
        let Ok(proxy) = Proxy::new(
            &connection,
            id.service.as_str(),
            id.path.as_str(),
            ITEM_INTERFACE,
        ) else {
            return;
        };
        let _ = proxy.call_noreply("Scroll", &(delta, orientation));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_owner_loss_from_replacement() {
        let lost = (":1.42".to_string(), ":1.42".to_string(), String::new());
        let replaced = (
            ":1.42".to_string(),
            ":1.42".to_string(),
            ":1.43".to_string(),
        );
        let other = (":1.7".to_string(), ":1.7".to_string(), String::new());

        assert!(name_owner_lost(&lost, ":1.42"));
        assert!(!name_owner_lost(&replaced, ":1.42"));
        assert!(!name_owner_lost(&other, ":1.42"));
    }

    #[test]
    fn stale_worker_cannot_remove_replacement_generation() {
        let id = ItemId {
            service: ":1.42".into(),
            path: "/StatusNotifierItem".into(),
        };
        let (signals, _) = mpsc::channel();
        let started = Arc::new(Mutex::new(StartedItems {
            items: HashMap::from([(
                id.clone(),
                StartedItem {
                    generation: 2,
                    registered: None,
                    owner: None,
                    signals,
                },
            )]),
            next_generation: 2,
        }));

        assert!(remove_generation(&started, &id, 1, None).is_none());
        assert!(is_current(&started, &id, 2));
        assert!(remove_generation(&started, &id, 2, None).is_some());
    }

    #[test]
    fn rebuilt_watcher_is_a_new_registration_scope() {
        let (events, _) = async_channel::unbounded();
        let started = Arc::new(Mutex::new(StartedItems::default()));
        let shared = SharedConnection::default();
        let watcher = Watcher::new(events.clone(), started.clone(), shared.clone());
        let replacement = Watcher::new(events, started, shared);

        assert!(watcher.same_instance(&watcher.clone()));
        assert!(!watcher.same_instance(&replacement));
    }
}
