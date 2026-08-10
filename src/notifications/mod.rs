mod daemon;
mod image;
mod model;
mod state;
mod view;

use std::{cell::RefCell, rc::Rc};

use gio::prelude::*;

use model::Snapshot;
use view::{Bell, Center, Popups};

pub struct Manager {
    snapshot: RefCell<Snapshot>,
    bell: RefCell<Option<Bell>>,
    center_anchor: RefCell<Option<gtk::ApplicationWindow>>,
    center: RefCell<Option<Center>>,
    popups: RefCell<Option<Popups>>,
    daemon: RefCell<Option<daemon::Control>>,
}

impl Manager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            snapshot: RefCell::new(Snapshot::unavailable()),
            bell: RefCell::new(None),
            center_anchor: RefCell::new(None),
            center: RefCell::new(None),
            popups: RefCell::new(None),
            daemon: RefCell::new(None),
        })
    }

    pub fn start(self: &Rc<Self>, app: &gtk::Application) {
        self.popups.replace(Some(Popups::new(app, self)));
        let (changes, views) = async_channel::bounded(1);
        let Some(daemon) = daemon::start(changes) else {
            return;
        };
        crate::background::listen(views, {
            let manager = Rc::downgrade(self);
            let daemon = daemon.clone();
            move |()| {
                if let Some(manager) = manager.upgrade() {
                    manager.apply(daemon.snapshot());
                }
            }
        });
        self.daemon.replace(Some(daemon));
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
                manager.toggle();
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

    pub fn set_center_anchor(&self, anchor: &gtk::ApplicationWindow) {
        self.center_anchor.replace(Some(anchor.clone()));
    }

    pub fn close(&self) {
        if let Some(center) = self.center.borrow().as_ref() {
            center.hide();
        }
    }

    pub(super) fn center_closed(&self) {
        if let Some(popups) = self.popups.borrow().as_ref() {
            popups.update(&self.snapshot.borrow(), false);
        }
    }

    pub(super) fn clear(&self) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.clear();
        }
    }

    pub(super) fn toggle_dnd(&self) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.toggle_dnd();
        }
    }

    pub(super) fn dismiss(&self, id: u32, active: bool) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.dismiss(id, active);
        }
    }

    pub(super) fn dismiss_group(&self, notifications: Vec<(u32, bool)>) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.dismiss_group(notifications);
        }
    }

    pub(super) fn invoke_action(&self, id: u32, key: &str) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.invoke_action(id, key.to_string());
        }
    }

    pub(super) fn displayed(&self, notifications: Vec<(u32, u64)>) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.displayed(notifications);
        }
    }

    fn toggle(self: &Rc<Self>) {
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
            let anchor = self
                .center_anchor
                .borrow()
                .as_ref()
                .expect("the bar creates the notification center anchor before opening it")
                .clone();
            let center = Center::new(&anchor, self, self.daemon.borrow().is_some());
            center.update(&self.snapshot.borrow());
            self.center.replace(Some(center));
        }
        self.center
            .borrow()
            .as_ref()
            .expect("center was just constructed")
            .show(&self.snapshot.borrow());
        if let Some(popups) = self.popups.borrow().as_ref() {
            popups.hide();
        }
    }

    fn apply(&self, snapshot: Snapshot) {
        if let Some(bell) = self.bell.borrow().as_ref() {
            bell.update(&snapshot);
        }
        if let Some(center) = self.center.borrow().as_ref() {
            center.update(&snapshot);
        }
        if let Some(popups) = self.popups.borrow().as_ref() {
            let center_open = self
                .center
                .borrow()
                .as_ref()
                .is_some_and(Center::is_visible);
            popups.update(&snapshot, center_open);
        }
        self.snapshot.replace(snapshot);
    }
}
