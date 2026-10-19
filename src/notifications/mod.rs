mod daemon;
mod hook;
mod image;
mod model;
mod persist;
mod sound;
mod state;
mod view;

use std::{cell::RefCell, rc::Rc};

use model::Snapshot;
use state::Action;
use view::{Bell, Center, Popups};

/// The notification that keyboard commands act on, with its actions.
pub struct Target {
    id: u32,
    actions: Vec<Action>,
}

impl Target {
    pub fn has_default(&self) -> bool {
        self.actions.iter().any(|action| action.key == "default")
    }

    /// One picker line per action; unlabelled ones fall back to their key.
    pub fn labels(&self) -> Vec<String> {
        self.actions.iter().map(label).map(str::to_string).collect()
    }

    fn key(&self, label_or_default: Option<&str>) -> Option<&str> {
        let Some(chosen) = label_or_default else {
            return self.has_default().then_some("default");
        };
        self.actions
            .iter()
            .find(|action| label(action) == chosen)
            .map(|action| action.key.as_str())
    }
}

fn label(action: &Action) -> &str {
    match (action.label.trim(), action.key.as_str()) {
        ("", "default") => "Open",
        ("", key) => key,
        (label, _) => label,
    }
}

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
        let Some(daemon) = daemon::start(changes, persist::load()) else {
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

    pub fn save(&self) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            persist::save(&daemon.saved());
        }
    }

    pub fn button(self: &Rc<Self>) -> gtk::Button {
        if self.bell.borrow().is_none() {
            let bell = Bell::new(self);
            bell.update(&self.snapshot.borrow(), self.center_open());
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
        if !self.center_open() {
            return;
        }
        if let Some(center) = self.center.borrow().as_ref() {
            center.hide();
        }
        if let Some(bell) = self.bell.borrow().as_ref() {
            bell.update(&self.snapshot.borrow(), false);
        }
        if let Some(popups) = self.popups.borrow().as_ref() {
            popups.update(&self.snapshot.borrow(), false);
        }
    }

    pub(crate) fn clear(&self) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.clear();
        }
    }

    pub fn latest_actionable(&self) -> Option<Target> {
        let daemon = self.daemon.borrow();
        let (id, actions) = daemon
            .as_ref()?
            .latest(|notification| !notification.actions.is_empty())?;
        Some(Target { id, actions })
    }

    /// Invokes the action with the picked label, or the default action.
    pub fn invoke(&self, target: &Target, label: Option<&str>) -> bool {
        let Some(key) = target.key(label) else {
            return false;
        };
        let token = gtk::gdk::Display::default()
            .and_then(|display| view::display_activation_token(&display));
        self.invoke_action(target.id, key, token);
        true
    }

    pub fn dismiss_latest(&self) -> bool {
        let daemon = self.daemon.borrow();
        let Some(daemon) = daemon.as_ref() else {
            return false;
        };
        let Some((id, _)) = daemon.latest(|_| true) else {
            return false;
        };
        daemon.dismiss(id);
        true
    }

    pub(crate) fn toggle_dnd(&self) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.toggle_dnd();
        }
    }

    pub(super) fn dismiss(&self, id: u32) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.dismiss(id);
        }
    }

    pub(super) fn dismiss_group(&self, notifications: Vec<u32>) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.dismiss_group(notifications);
        }
    }

    pub(super) fn invoke_action(&self, id: u32, key: &str, activation_token: Option<String>) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.invoke_action(id, key.to_string(), activation_token);
        }
    }

    pub(super) fn pause_popups(&self, paused: bool) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.pause_popups(paused);
        }
    }

    pub(super) fn displayed(&self, notifications: Vec<(u32, u64)>) {
        if let Some(daemon) = self.daemon.borrow().as_ref() {
            daemon.displayed(notifications);
        }
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.center_open() {
            self.close();
            return;
        }
        if let Some(bell) = self.bell.borrow().as_ref() {
            bell.update(&self.snapshot.borrow(), true);
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
        let center_open = self.center_open();
        if let Some(bell) = self.bell.borrow().as_ref() {
            bell.update(&snapshot, center_open);
        }
        if let Some(center) = self.center.borrow().as_ref() {
            center.update(&snapshot);
        }
        if let Some(popups) = self.popups.borrow().as_ref() {
            popups.update(&snapshot, center_open);
        }
        self.snapshot.replace(snapshot);
    }

    fn center_open(&self) -> bool {
        self.center
            .borrow()
            .as_ref()
            .is_some_and(Center::is_visible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(actions: &[(&str, &str)]) -> Target {
        Target {
            id: 1,
            actions: actions
                .iter()
                .map(|(key, label)| Action {
                    key: (*key).into(),
                    label: (*label).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn picker_labels_map_back_to_action_keys() {
        let target = target(&[("default", ""), ("reply", " Reply "), ("mute", "")]);
        assert_eq!(target.labels(), ["Open", "Reply", "mute"]);
        assert_eq!(target.key(None), Some("default"));
        assert_eq!(target.key(Some("Open")), Some("default"));
        assert_eq!(target.key(Some("Reply")), Some("reply"));
        assert_eq!(target.key(Some("mute")), Some("mute"));
        assert_eq!(target.key(Some("Other")), None);
    }

    #[test]
    fn targets_without_a_default_action_have_no_default_key() {
        assert_eq!(target(&[("reply", "Reply")]).key(None), None);
    }
}
