use std::collections::{HashMap, HashSet};

use super::state;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Snapshot {
    pub groups: Vec<Group>,
    pub count: usize,
    pub dnd: bool,
    pub available: bool,
}

impl Snapshot {
    pub fn unavailable() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn empty(dnd: bool) -> Self {
        Self {
            dnd,
            available: true,
            ..Self::default()
        }
    }

    pub fn alt(&self) -> &'static str {
        match (self.dnd, self.count > 0) {
            (true, true) => "dnd-notification",
            (true, false) => "dnd-none",
            (false, true) => "notification",
            (false, false) => "none",
        }
    }

    pub fn tooltip(&self) -> String {
        if !self.available {
            return "Notifications unavailable".into();
        }
        match (self.dnd, self.count) {
            (true, 0) => "Do Not Disturb".into(),
            (true, count) => format!("Do Not Disturb — {count} notification(s)"),
            (false, 0) => "No notifications".into(),
            (false, count) => format!("{count} notification(s)"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Group {
    pub key: String,
    pub desktop_entry: Option<String>,
    pub name: String,
    pub icon: Option<String>,
    pub notifications: Vec<Notification>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Notification {
    pub id: u32,
    pub revision: u64,
    pub received_at: Option<i64>,
    pub active: bool,
    app_name: Option<String>,
    pub app_icon: Option<String>,
    pub thumbnail: Option<state::Thumbnail>,
    pub progress: Option<u8>,
    desktop_entry: Option<String>,
    pub summary: String,
    pub body: String,
    pub actions: Vec<state::Action>,
    pub urgency: Option<String>,
}

pub(super) fn from_state(store: &state::Store) -> Snapshot {
    let active = store
        .active()
        .map(|notification| from_state_notification(notification, true));
    let history = store
        .history()
        .map(|notification| from_state_notification(notification, false));
    group(active.chain(history), store.dnd())
}

fn from_state_notification(notification: &state::Notification, active: bool) -> Notification {
    Notification {
        id: notification.id,
        revision: notification.revision,
        received_at: Some(notification.received_at),
        active,
        app_name: Some(notification.app_name.clone()),
        app_icon: (!notification.app_icon.is_empty()).then(|| notification.app_icon.clone()),
        thumbnail: notification.thumbnail.clone(),
        progress: notification.progress,
        desktop_entry: (!notification.desktop_entry.is_empty())
            .then(|| notification.desktop_entry.clone()),
        summary: notification.summary.clone(),
        body: notification.body.clone(),
        actions: notification.actions.clone(),
        urgency: Some(
            match notification.urgency {
                state::Urgency::Low => "low",
                state::Urgency::Normal => "normal",
                state::Urgency::Critical => "critical",
            }
            .into(),
        ),
    }
}

fn group(notifications: impl Iterator<Item = Notification>, dnd: bool) -> Snapshot {
    let mut groups = Vec::<Group>::new();
    let mut indexes = HashMap::<String, usize>::new();
    let mut ids = HashSet::new();

    for notification in notifications {
        if !ids.insert(notification.id) {
            continue;
        }
        let key = application_key(&notification);
        let index = *indexes.entry(key.clone()).or_insert_with(|| {
            let index = groups.len();
            groups.push(Group {
                key,
                desktop_entry: nonempty(notification.desktop_entry.as_deref()).map(str::to_string),
                name: application_name(&notification),
                icon: notification
                    .app_icon
                    .clone()
                    .filter(|icon| !icon.is_empty()),
                notifications: Vec::new(),
            });
            index
        });
        groups[index].notifications.push(notification);
    }

    let count = ids.len();
    Snapshot {
        groups,
        count,
        dnd,
        available: true,
    }
}

fn application_key(notification: &Notification) -> String {
    if let Some(desktop) = nonempty(notification.desktop_entry.as_deref()) {
        format!("desktop:{}", desktop.to_lowercase())
    } else if let Some(name) = nonempty(notification.app_name.as_deref()) {
        format!("name:{}", name.to_lowercase())
    } else {
        "other".into()
    }
}

fn application_name(notification: &Notification) -> String {
    nonempty(notification.app_name.as_deref())
        .or_else(|| nonempty(notification.desktop_entry.as_deref()))
        .unwrap_or("Other")
        .to_string()
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(super) fn test_snapshot(notifications: &[(u32, u64)]) -> Snapshot {
    group(
        notifications
            .iter()
            .map(|&(id, revision)| notification(id, revision, None, None, true)),
        false,
    )
}

#[cfg(test)]
fn notification(
    id: u32,
    revision: u64,
    app_name: Option<&str>,
    desktop_entry: Option<&str>,
    active: bool,
) -> Notification {
    Notification {
        id,
        revision,
        received_at: None,
        active,
        app_name: app_name.map(str::to_string),
        app_icon: None,
        thumbnail: None,
        progress: None,
        desktop_entry: desktop_entry.map(str::to_string),
        summary: String::new(),
        body: String::new(),
        actions: Vec::new(),
        urgency: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_active_before_history_and_deduplicates_ids() {
        let mut active_chat = notification(2, 1, Some("Chat"), None, true);
        active_chat.summary = "Active".into();
        let snapshot = group(
            [
                active_chat,
                notification(1, 1, Some("Mail"), None, true),
                notification(2, 1, Some("Chat"), None, false),
                notification(3, 1, Some("Chat"), None, false),
            ]
            .into_iter(),
            false,
        );
        assert_eq!(snapshot.count, 3);
        assert_eq!(snapshot.groups[0].name, "Chat");
        assert_eq!(snapshot.groups[0].notifications.len(), 2);
        assert_eq!(snapshot.groups[0].notifications[0].summary, "Active");
        assert_eq!(snapshot.groups[1].name, "Mail");
    }

    #[test]
    fn uses_desktop_entry_then_name_then_other_for_identity() {
        let snapshot = group(
            [
                notification(1, 1, Some("Chat"), Some("chat"), true),
                notification(2, 1, Some("Different label"), Some("chat"), true),
                notification(3, 1, Some("Mail"), None, true),
                notification(4, 1, None, None, true),
            ]
            .into_iter(),
            false,
        );

        assert_eq!(snapshot.groups.len(), 3);
        assert_eq!(snapshot.groups[0].notifications.len(), 2);
        assert_eq!(snapshot.groups[1].name, "Mail");
        assert_eq!(snapshot.groups[2].name, "Other");
    }

    #[test]
    fn derives_bell_state() {
        assert_eq!(Snapshot::empty(false).alt(), "none");
        assert_eq!(Snapshot::empty(true).alt(), "dnd-none");

        let populated = group([notification(1, 1, None, None, true)].into_iter(), true);
        assert_eq!(populated.alt(), "dnd-notification");
        assert_eq!(populated.tooltip(), "Do Not Disturb — 1 notification(s)");
        let history_only = group([notification(1, 1, None, None, false)].into_iter(), false);
        assert_eq!(history_only.alt(), "notification");
        assert_eq!(history_only.tooltip(), "1 notification(s)");
        assert_eq!(
            Snapshot::unavailable().tooltip(),
            "Notifications unavailable"
        );
    }

    #[test]
    fn carries_actions_into_the_snapshot() {
        let mut store = state::Store::default();
        store.notify(state::Incoming {
            actions: vec![state::Action {
                key: "reply".into(),
                label: "Reply".into(),
            }],
            ..state::Incoming::default()
        });

        assert_eq!(
            from_state(&store).groups[0].notifications[0].actions,
            vec![state::Action {
                key: "reply".into(),
                label: "Reply".into(),
            }]
        );
    }
}
