mod model;
mod view;

use std::{cell::RefCell, rc::Rc, thread, time::Duration};

use gio::prelude::*;
use gtk::glib;
use zbus::{
    MatchRule,
    blocking::{Connection, MessageIterator, fdo::MonitoringProxy},
    message::Type,
};

use model::Snapshot;
use view::{Bell, Center};

const FALLBACK_INTERVAL: Duration = Duration::from_secs(30);
const MONITOR_RETRY_DELAY: Duration = Duration::from_secs(5);
const EVENT_SETTLE_DELAY: Duration = Duration::from_millis(50);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const NOTIFICATION_SERVICE: &str = "org.freedesktop.Notifications";
const NOTIFICATION_INTERFACE: &str = "org.freedesktop.Notifications";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const DND_MODE: &str = "do-not-disturb";
const MAX_CLEAR_COUNT: usize = 100;

pub struct Manager {
    snapshot: RefCell<Snapshot>,
    bell: RefCell<Option<Bell>>,
    center: RefCell<Option<Center>>,
    refresh: async_channel::Sender<()>,
    startup: RefCell<Option<Startup>>,
}

type Startup = (
    async_channel::Receiver<()>,
    async_channel::Sender<Snapshot>,
    async_channel::Receiver<Snapshot>,
);

impl Manager {
    pub fn new() -> Rc<Self> {
        let (refresh, requests) = async_channel::bounded(1);
        let (results, snapshots) = async_channel::bounded(1);
        Rc::new(Self {
            snapshot: RefCell::new(Snapshot::unavailable()),
            bell: RefCell::new(None),
            center: RefCell::new(None),
            refresh,
            startup: RefCell::new(Some((requests, results, snapshots))),
        })
    }

    pub fn start(self: &Rc<Self>) {
        let Some((requests, results, snapshots)) = self.startup.take() else {
            return;
        };

        crate::background::spawn("notification-state", move || {
            while requests.recv_blocking().is_ok() {
                let _ = results.send_blocking(fetch_snapshot());
            }
        });
        crate::background::listen(snapshots, {
            let manager = Rc::downgrade(self);
            move |snapshot| {
                if let Some(manager) = manager.upgrade() {
                    manager.apply(snapshot);
                }
            }
        });
        subscribe(self.refresh.clone());
        let refresh = self.refresh.clone();
        glib::timeout_add_local(FALLBACK_INTERVAL, move || {
            request(&refresh);
            glib::ControlFlow::Continue
        });
        self.request_refresh();
    }

    pub fn install_action(
        self: &Rc<Self>,
        app: &gtk::Application,
        before_open: impl Fn() + 'static,
    ) {
        let action = gio::SimpleAction::new("notifications", None);
        action.connect_activate({
            let app = app.clone();
            let manager = self.clone();
            move |_, _| {
                app.activate();
                before_open();
                manager.toggle(&app);
            }
        });
        app.add_action(&action);
    }

    pub fn button(self: &Rc<Self>, app: &gtk::Application) -> gtk::Button {
        if self.bell.borrow().is_none() {
            let bell = Bell::new(self, app);
            bell.update(&self.snapshot.borrow());
            self.bell.replace(Some(bell));
        }
        self.bell
            .borrow()
            .as_ref()
            .expect("bell was just constructed")
            .button
            .clone()
    }

    pub fn close(&self) {
        if let Some(center) = self.center.borrow().as_ref() {
            center.hide();
        }
    }

    pub(super) fn clear(&self) {
        let refresh = self.refresh.clone();
        crate::background::spawn("notification-clear", move || {
            clear_all();
            request(&refresh);
        });
    }

    pub(super) fn toggle_dnd(&self) {
        let refresh = self.refresh.clone();
        crate::background::spawn("notification-dnd", move || {
            command(&["mode", "-t", DND_MODE]);
            request(&refresh);
        });
    }

    fn toggle(self: &Rc<Self>, app: &gtk::Application) {
        if self
            .center
            .borrow()
            .as_ref()
            .is_some_and(Center::is_visible)
        {
            self.close();
            return;
        }
        if self.center.borrow().is_none() {
            let center = Center::new(app, self);
            center.update(&self.snapshot.borrow());
            self.center.replace(Some(center));
        }
        self.center
            .borrow()
            .as_ref()
            .expect("center was just constructed")
            .show();
        self.request_refresh();
    }

    fn request_refresh(&self) {
        request(&self.refresh);
    }

    fn apply(&self, snapshot: Snapshot) {
        if let Some(bell) = self.bell.borrow().as_ref() {
            bell.update(&snapshot);
        }
        if let Some(center) = self.center.borrow().as_ref() {
            center.update(&snapshot);
        }
        self.snapshot.replace(snapshot);
    }
}

fn fetch_snapshot() -> Snapshot {
    let Some(active) = command(&["list", "-j"]) else {
        return Snapshot::unavailable();
    };
    let Some(history) = command(&["history", "-j"]) else {
        return Snapshot::unavailable();
    };
    let Some(modes) = command(&["mode"]) else {
        return Snapshot::unavailable();
    };
    model::parse(&active, &history, dnd_enabled(&modes)).unwrap_or_else(Snapshot::unavailable)
}

fn clear_all() {
    let Some(modes) = command(&["mode"]) else {
        return;
    };
    let Some(history) = command(&["history", "-j"])
        .and_then(|json| serde_json::from_slice::<Vec<serde_json::Value>>(&json).ok())
    else {
        return;
    };
    let dnd = dnd_enabled(&modes);
    let history_count = history.len().min(MAX_CLEAR_COUNT);

    if !dnd && command(&["mode", "-a", DND_MODE]).is_none() {
        return;
    }
    for _ in 0..history_count {
        if command(&["restore"]).is_none() {
            break;
        }
    }
    command(&["dismiss", "--all", "--no-history"]);
    if !dnd {
        command(&["mode", "-r", DND_MODE]);
    }
}

fn dnd_enabled(modes: &[u8]) -> bool {
    String::from_utf8_lossy(modes)
        .lines()
        .any(|mode| mode.trim() == DND_MODE)
}

fn command(args: &[&str]) -> Option<Vec<u8>> {
    crate::background::command_output("makoctl", args, COMMAND_TIMEOUT)
}

fn request(refresh: &async_channel::Sender<()>) {
    let _ = refresh.try_send(());
}

fn subscribe(refresh: async_channel::Sender<()>) {
    crate::background::spawn("notification-events", move || {
        loop {
            let _ = monitor_notifications(&refresh);
            thread::sleep(MONITOR_RETRY_DELAY);
            request(&refresh);
        }
    });
}

/// The notification specification has no signal for a newly posted notification.
/// Monitoring calls to the daemon provides that event without polling continuously.
fn monitor_notifications(refresh: &async_channel::Sender<()>) -> zbus::Result<()> {
    let connection = Connection::session()?;
    let rules = [
        MatchRule::builder()
            .msg_type(Type::MethodCall)
            .destination(NOTIFICATION_SERVICE)?
            .interface(NOTIFICATION_INTERFACE)?
            .build(),
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(NOTIFICATION_SERVICE)?
            .interface(NOTIFICATION_INTERFACE)?
            .build(),
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(DBUS_SERVICE)?
            .interface(DBUS_INTERFACE)?
            .member("NameOwnerChanged")?
            .add_arg(NOTIFICATION_SERVICE)?
            .build(),
    ];
    MonitoringProxy::new(&connection)?.become_monitor(&rules, 0)?;
    for message in MessageIterator::from(connection).flatten() {
        if message.header().message_type() == Type::MethodCall {
            thread::sleep(EVENT_SETTLE_DELAY);
        }
        request(refresh);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_the_exact_dnd_mode() {
        assert!(dnd_enabled(b"default\ndo-not-disturb\n"));
        assert!(!dnd_enabled(b"default\ndo-not-disturbing\n"));
    }
}
