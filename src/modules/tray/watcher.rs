use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use async_channel::Sender;
use zbus::{
    blocking::{Connection, Proxy},
    fdo::{RequestNameFlags, RequestNameReply},
    interface,
    message::Header,
    object_server::SignalEmitter,
};

use super::model::{Event, Item, ItemId, select_pixmap, tooltip};
use crate::background;

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";

pub(super) fn run(events: Sender<Event>) {
    let Ok(connection) = Connection::session() else {
        return;
    };

    if connection
        .object_server()
        .at(WATCHER_PATH, Watcher::new(events.clone()))
        .is_err()
    {
        return;
    }

    loop {
        match connection.request_name_with_flags(WATCHER_NAME, RequestNameFlags::DoNotQueue.into())
        {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                register_host(&connection);
                loop {
                    thread::sleep(Duration::from_secs(60));
                }
            }
            Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(_) => {
                host_existing_watcher(&connection, events.clone());
                thread::sleep(Duration::from_secs(1));
            }
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

fn host_existing_watcher(connection: &Connection, events: Sender<Event>) {
    register_host(connection);

    let Ok(watcher) = Proxy::new(connection, WATCHER_NAME, WATCHER_PATH, WATCHER_INTERFACE) else {
        return;
    };
    let Ok(signals) = watcher.receive_all_signals() else {
        return;
    };
    if let Ok(registered) = watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems") {
        for item in registered
            .iter()
            .filter_map(|registration| ItemId::from_registration(registration))
        {
            start_item(item, events.clone(), None);
        }
    }

    for signal in signals {
        let header = signal.header();
        let Some(member) = header.member() else {
            continue;
        };
        let Ok(registration) = signal.body().deserialize::<String>() else {
            continue;
        };
        let Some(item) = ItemId::from_registration(&registration) else {
            continue;
        };
        match member.as_str() {
            "StatusNotifierItemRegistered" => start_item(item, events.clone(), None),
            "StatusNotifierItemUnregistered" => {
                let _ = events.try_send(Event::Remove(item));
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct Watcher {
    items: Arc<Mutex<HashSet<ItemId>>>,
    events: Sender<Event>,
    host_registered: Arc<Mutex<bool>>,
}

impl Watcher {
    fn new(events: Sender<Event>) -> Self {
        Self {
            items: Arc::new(Mutex::new(HashSet::new())),
            events,
            host_registered: Arc::new(Mutex::new(false)),
        }
    }

    fn register_item(&self, item: ItemId) -> bool {
        self.items
            .lock()
            .expect("tray item lock poisoned")
            .insert(item)
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

    fn unregister_item(&self, item: &ItemId) -> bool {
        self.items
            .lock()
            .expect("tray item lock poisoned")
            .remove(item)
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
        if !self.register_item(item.clone()) {
            return;
        }

        let registered = item.registration();
        let _ = zbus::block_on(Self::status_notifier_item_registered(&emitter, &registered));
        let _ = zbus::block_on(self.registered_status_notifier_items_changed(&emitter));
        start_item(
            item,
            self.events.clone(),
            Some(RegisteredItem {
                watcher: self.clone(),
                emitter: emitter.to_owned(),
            }),
        );
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
        self.items
            .lock()
            .expect("tray item lock poisoned")
            .iter()
            .map(ItemId::registration)
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

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct RegisteredItem {
    watcher: Watcher,
    emitter: SignalEmitter<'static>,
}

fn start_item(id: ItemId, events: Sender<Event>, registered: Option<RegisteredItem>) {
    background::spawn("tray-item", {
        let id = id.clone();
        let events = events.clone();
        move || {
            let Ok(connection) = Connection::session() else {
                return;
            };
            let Ok(item) = load_item(&connection, &id) else {
                return;
            };
            if events.send_blocking(Event::Upsert(item)).is_err() {
                return;
            }

            let Ok(proxy) = Proxy::new(
                &connection,
                id.service.as_str(),
                id.path.as_str(),
                ITEM_INTERFACE,
            ) else {
                return;
            };
            let Ok(signals) = proxy.receive_all_signals() else {
                return;
            };
            for signal in signals {
                let header = signal.header();
                let Some(member) = header.member() else {
                    continue;
                };
                if matches!(
                    member.as_str(),
                    "NewIcon"
                        | "NewAttentionIcon"
                        | "NewStatus"
                        | "NewTitle"
                        | "NewToolTip"
                        | "NewMenu"
                ) && let Ok(item) = load_item(&connection, &id)
                    && events.send_blocking(Event::Upsert(item)).is_err()
                {
                    return;
                }
            }
        }
    });

    background::spawn("tray-item-owner", move || {
        watch_item_owner(id, events, registered)
    });
}

fn watch_item_owner(id: ItemId, events: Sender<Event>, registered: Option<RegisteredItem>) {
    let Ok(connection) = Connection::session() else {
        return;
    };
    let lost_owner = {
        let Ok(proxy) = Proxy::new(
            &connection,
            id.service.as_str(),
            id.path.as_str(),
            ITEM_INTERFACE,
        ) else {
            return;
        };
        let Ok(mut owners) = proxy.receive_owner_changed() else {
            return;
        };
        owner_was_lost(owners.next())
    };
    if lost_owner {
        let _ = events.try_send(Event::Remove(id.clone()));
        if let Some(registered) = registered
            && registered.watcher.unregister_item(&id)
        {
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
}

fn owner_was_lost<T>(change: Option<Option<T>>) -> bool {
    matches!(change, Some(None))
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

pub(super) fn call_item(id: &ItemId, method: &'static str, position: (i32, i32)) {
    let id = id.clone();
    background::spawn("tray-item-call", move || {
        let Ok(connection) = Connection::session() else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_owner_loss_from_iterator_termination() {
        assert!(owner_was_lost::<()>(Some(None)));
        assert!(!owner_was_lost(Some(Some(()))));
        assert!(!owner_was_lost::<()>(None));
    }
}
