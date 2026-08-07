use std::collections::{HashMap, HashSet};

use serde::Deserialize;

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct Notification {
    pub id: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub received_at: Option<i64>,
    #[serde(skip)]
    pub active: bool,
    #[serde(default)]
    app_name: Option<String>,
    #[serde(default)]
    pub app_icon: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(skip)]
    pub image_data: Option<state::ImageData>,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    desktop_entry: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub urgency: Option<String>,
}

pub(super) fn parse(active: &[u8], history: &[u8], dnd: bool) -> Option<Snapshot> {
    let mut active = serde_json::from_slice::<Vec<Notification>>(active).ok()?;
    active
        .iter_mut()
        .for_each(|notification| notification.active = true);
    let history = serde_json::from_slice::<Vec<Notification>>(history).ok()?;
    Some(group(active.into_iter().chain(history), dnd))
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
        image: (!notification.image.is_empty()).then(|| notification.image.clone()),
        image_data: notification.image_data.clone(),
        progress: notification.progress,
        desktop_entry: (!notification.desktop_entry.is_empty())
            .then(|| notification.desktop_entry.clone()),
        summary: notification.summary.clone(),
        body: notification.body.clone(),
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
mod tests {
    use super::*;

    #[test]
    fn groups_active_before_history_and_deduplicates_ids() {
        let active = br#"[
            {"id":2,"app_name":"Chat","summary":"Active"},
            {"id":1,"app_name":"Mail","summary":"Mail"}
        ]"#;
        let history = br#"[
            {"id":2,"app_name":"Chat","summary":"Duplicate"},
            {"id":3,"app_name":"Chat","summary":"Older"}
        ]"#;

        let snapshot = parse(active, history, false).unwrap();
        assert_eq!(snapshot.count, 3);
        assert_eq!(snapshot.groups[0].name, "Chat");
        assert_eq!(snapshot.groups[0].notifications.len(), 2);
        assert_eq!(snapshot.groups[0].notifications[0].summary, "Active");
        assert_eq!(snapshot.groups[1].name, "Mail");
    }

    #[test]
    fn uses_desktop_entry_then_name_then_other_for_identity() {
        let snapshot = parse(
            br#"[
                {"id":1,"desktop_entry":"chat","app_name":"Chat"},
                {"id":2,"desktop_entry":"chat","app_name":"Different label"},
                {"id":3,"app_name":"Mail"},
                {"id":4}
            ]"#,
            b"[]",
            false,
        )
        .unwrap();

        assert_eq!(snapshot.groups.len(), 3);
        assert_eq!(snapshot.groups[0].notifications.len(), 2);
        assert_eq!(snapshot.groups[1].name, "Mail");
        assert_eq!(snapshot.groups[2].name, "Other");
    }

    #[test]
    fn derives_bell_state() {
        assert_eq!(Snapshot::empty(false).alt(), "none");
        assert_eq!(Snapshot::empty(true).alt(), "dnd-none");

        let populated = parse(br#"[{"id":1}]"#, b"[]", true).unwrap();
        assert_eq!(populated.alt(), "dnd-notification");
        assert_eq!(populated.tooltip(), "Do Not Disturb — 1 notification(s)");
        let history_only = parse(b"[]", br#"[{"id":1}]"#, false).unwrap();
        assert_eq!(history_only.alt(), "notification");
        assert_eq!(history_only.tooltip(), "1 notification(s)");
        assert_eq!(
            Snapshot::unavailable().tooltip(),
            "Notifications unavailable"
        );
    }

    #[test]
    fn rejects_malformed_snapshots() {
        assert!(parse(b"not json", b"[]", false).is_none());
        assert!(parse(b"[]", b"not json", false).is_none());
    }
}
