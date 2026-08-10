use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::image::Thumbnail;

const MAX_NOTIFICATIONS: usize = 100;
const POPUP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Action {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Picture {
    Pixels(Thumbnail),
    Themed(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Notification {
    pub id: u32,
    pub revision: u64,
    pub received_at: i64,
    pub app_name: String,
    pub app_icon: String,
    pub picture: Option<Picture>,
    pub progress: Option<u8>,
    pub summary: String,
    pub body: String,
    pub actions: Vec<Action>,
    pub urgency: Urgency,
    pub desktop_entry: String,
    pub resident: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Incoming {
    pub replaces_id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub picture: Option<Picture>,
    pub progress: Option<u8>,
    pub summary: String,
    pub body: String,
    pub actions: Vec<Action>,
    pub urgency: Urgency,
    pub desktop_entry: String,
    pub tag: String,
    pub transient: bool,
    pub resident: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Requested = 3,
    Undefined = 4,
}

#[derive(Clone, Debug)]
struct Stored {
    notification: Notification,
    tag: String,
    transient: bool,
    popup_timeout: Option<Duration>,
    popup_deadline: Option<Instant>,
    show_popup: bool,
}

#[derive(Default)]
pub(super) struct Store {
    next_id: u32,
    next_revision: u64,
    notifications: Vec<Stored>,
    dnd: bool,
}

impl Store {
    #[cfg(test)]
    pub fn notify(&mut self, incoming: Incoming) -> u32 {
        self.notify_with_eviction(incoming).0
    }

    pub fn notify_with_eviction(
        &mut self,
        incoming: Incoming,
    ) -> (u32, Option<(u32, CloseReason)>) {
        let replacement = self.replacement_index(&incoming);
        let evicted = replacement
            .is_none()
            .then(|| self.evict_at_capacity())
            .flatten();
        let id = replacement
            .map(|index| self.notifications[index].notification.id)
            .unwrap_or_else(|| self.allocate_id());
        let revision = self.allocate_revision();
        let popup_timeout = popup_timeout(incoming.urgency);
        let show_popup = replacement
            .map(|index| self.notifications[index].show_popup)
            .unwrap_or(true);
        let stored = Stored {
            notification: Notification {
                id,
                revision,
                received_at: unix_now(),
                app_name: incoming.app_name,
                app_icon: incoming.app_icon,
                picture: incoming.picture,
                progress: incoming.progress,
                summary: incoming.summary,
                body: incoming.body,
                actions: incoming.actions,
                urgency: incoming.urgency,
                desktop_entry: incoming.desktop_entry,
                resident: incoming.resident,
            },
            tag: incoming.tag,
            transient: incoming.transient,
            popup_timeout,
            popup_deadline: popup_timeout.and_then(|_| {
                replacement.and_then(|index| self.notifications[index].popup_deadline)
            }),
            show_popup,
        };

        if let Some(index) = replacement {
            self.notifications[index] = stored;
        } else {
            self.notifications.insert(0, stored);
        }
        (id, evicted)
    }

    pub fn close(&mut self, id: u32) -> bool {
        let Some(index) = self
            .notifications
            .iter()
            .position(|stored| stored.notification.id == id)
        else {
            return false;
        };
        self.notifications.remove(index);
        true
    }

    // Popup timeouts end presentation, not the notification's lifetime.
    pub fn hide_due_popups(&mut self, now: Instant) -> Vec<(u32, CloseReason)> {
        let due = self
            .notifications
            .iter()
            .enumerate()
            .filter(|(_, stored)| stored.popup_deadline.is_some_and(|at| at <= now))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let mut closed = Vec::new();
        for index in due.into_iter().rev() {
            if self.notifications[index].transient {
                let id = self.notifications.remove(index).notification.id;
                closed.push((id, CloseReason::Expired));
            } else {
                self.notifications[index].show_popup = false;
                self.notifications[index].popup_deadline = None;
            }
        }
        closed
    }

    pub fn displayed(&mut self, id: u32, revision: u64, now: Instant) -> bool {
        let Some(stored) = self.notifications.iter_mut().find(|stored| {
            stored.notification.id == id && stored.notification.revision == revision
        }) else {
            return false;
        };
        stored.popup_deadline = stored
            .popup_timeout
            .and_then(|timeout| now.checked_add(timeout));
        true
    }

    pub fn next_popup_deadline(&self) -> Option<Instant> {
        self.notifications
            .iter()
            .filter_map(|stored| stored.popup_deadline)
            .min()
    }

    pub fn clear(&mut self) -> Vec<(u32, CloseReason)> {
        let closed = self
            .notifications
            .iter()
            .map(|stored| (stored.notification.id, CloseReason::Dismissed))
            .collect();
        self.notifications.clear();
        closed
    }

    pub fn set_dnd(&mut self, dnd: bool) {
        self.dnd = dnd;
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    pub fn notifications(&self) -> impl Iterator<Item = (&Notification, bool)> {
        self.notifications
            .iter()
            .map(|stored| (&stored.notification, stored.show_popup))
    }

    fn replacement_index(&self, incoming: &Incoming) -> Option<usize> {
        if incoming.replaces_id != 0
            && let Some(index) = self
                .notifications
                .iter()
                .position(|stored| stored.notification.id == incoming.replaces_id)
        {
            return Some(index);
        }
        if incoming.tag.is_empty() {
            return None;
        }
        self.notifications.iter().position(|stored| {
            stored.tag == incoming.tag && stored.notification.app_name == incoming.app_name
        })
    }

    fn evict_at_capacity(&mut self) -> Option<(u32, CloseReason)> {
        if self.notifications.len() < MAX_NOTIFICATIONS {
            return None;
        }
        let index = self
            .notifications
            .iter()
            .enumerate()
            .min_by_key(|(_, stored)| stored.notification.revision)
            .map(|(index, _)| index)?;
        let id = self.notifications.remove(index).notification.id;
        Some((id, CloseReason::Undefined))
    }

    fn allocate_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id != 0
                && !self
                    .notifications
                    .iter()
                    .any(|stored| stored.notification.id == self.next_id)
            {
                return self.next_id;
            }
        }
    }

    fn allocate_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1);
        if self.next_revision == 0 {
            self.next_revision = 1;
        }
        self.next_revision
    }
}

fn popup_timeout(urgency: Urgency) -> Option<Duration> {
    match urgency {
        Urgency::Low | Urgency::Normal => Some(POPUP_TIMEOUT),
        Urgency::Critical => None,
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn incoming(summary: &str) -> Incoming {
        Incoming {
            summary: summary.into(),
            ..Incoming::default()
        }
    }

    fn notification(store: &Store, id: u32) -> Option<(&Notification, bool)> {
        store
            .notifications()
            .find(|(notification, _)| notification.id == id)
    }

    #[test]
    fn allocates_ids_and_replaces_notifications_in_place() {
        let mut store = Store::default();
        let first = store.notify(incoming("first"));
        let second = store.notify(incoming("second"));

        let replacement = Incoming {
            replaces_id: first,
            summary: "updated".into(),
            ..Incoming::default()
        };
        assert_eq!(store.notify(replacement), first);
        assert_eq!(store.notifications().count(), 2);
        assert_eq!(notification(&store, first).unwrap().0.summary, "updated");
        assert_ne!(first, second);
    }

    #[test]
    fn replacements_always_receive_a_new_revision() {
        let mut store = Store::default();
        let id = store.notify(incoming("same"));
        let revision = notification(&store, id).unwrap().0.revision;

        assert_eq!(
            store.notify(Incoming {
                replaces_id: id,
                ..incoming("same")
            }),
            id
        );
        assert!(notification(&store, id).unwrap().0.revision > revision);
    }

    #[test]
    fn invalid_replacement_ids_create_new_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            replaces_id: 42,
            ..Incoming::default()
        });
        assert_ne!(id, 42);
        assert_ne!(id, 0);
    }

    #[test]
    fn stack_tags_are_scoped_to_the_application() {
        let mut store = Store::default();
        let tagged = |app: &str, summary: &str| Incoming {
            app_name: app.into(),
            summary: summary.into(),
            tag: "progress".into(),
            ..Incoming::default()
        };
        let first = store.notify(tagged("one", "old"));
        let other = store.notify(tagged("two", "other"));
        assert_eq!(store.notify(tagged("one", "new")), first);
        assert_eq!(store.notifications().count(), 2);
        assert_ne!(first, other);
    }

    #[test]
    fn non_critical_popups_hide_after_five_seconds() {
        let now = Instant::now();
        let mut store = Store::default();
        let low = store.notify(Incoming {
            urgency: Urgency::Low,
            ..Incoming::default()
        });
        let normal = store.notify(incoming("normal"));
        let critical = store.notify(Incoming {
            urgency: Urgency::Critical,
            ..Incoming::default()
        });

        assert!(store.hide_due_popups(now + POPUP_TIMEOUT).is_empty());
        assert!(store.displayed(low, 1, now));
        assert!(store.displayed(normal, 2, now));
        assert!(store.displayed(critical, 3, now));

        assert!(
            store
                .hide_due_popups(now + POPUP_TIMEOUT - Duration::from_millis(1))
                .is_empty()
        );
        assert!(notification(&store, low).unwrap().1);
        assert!(notification(&store, normal).unwrap().1);
        assert!(store.hide_due_popups(now + POPUP_TIMEOUT).is_empty());
        assert!(!notification(&store, low).unwrap().1);
        assert!(!notification(&store, normal).unwrap().1);
        assert!(notification(&store, critical).unwrap().1);
    }

    #[test]
    fn queued_popup_timeouts_start_only_after_display() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming::default());
        let revision = notification(&store, id).unwrap().0.revision;

        assert!(
            store
                .hide_due_popups(now + Duration::from_secs(1))
                .is_empty()
        );
        assert!(notification(&store, id).unwrap().1);
        assert!(store.displayed(id, revision, now + Duration::from_secs(1)));
        assert!(
            store
                .hide_due_popups(
                    now + Duration::from_secs(1) + POPUP_TIMEOUT - Duration::from_millis(1)
                )
                .is_empty()
        );
        assert!(notification(&store, id).unwrap().1);
        assert!(
            store
                .hide_due_popups(now + Duration::from_secs(1) + POPUP_TIMEOUT)
                .is_empty()
        );
        assert!(!notification(&store, id).unwrap().1);
    }

    #[test]
    fn critical_replacement_cancels_the_previous_popup_timeout() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming::default());
        assert!(store.displayed(id, 1, now));
        store.notify(Incoming {
            replaces_id: id,
            urgency: Urgency::Critical,
            ..Incoming::default()
        });

        assert!(
            store
                .hide_due_popups(now + Duration::from_secs(1))
                .is_empty()
        );
        assert!(notification(&store, id).unwrap().1);
    }

    #[test]
    fn replacing_a_hidden_notification_keeps_it_in_the_center_without_a_popup() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming {
            actions: vec![Action {
                key: "reply".into(),
                label: "Reply".into(),
            }],
            ..Incoming::default()
        });
        assert!(store.displayed(id, 1, now));
        assert!(store.hide_due_popups(now + POPUP_TIMEOUT).is_empty());

        assert_eq!(
            store.notify(Incoming {
                replaces_id: id,
                summary: "updated".into(),
                ..Incoming::default()
            }),
            id
        );
        let (updated, show_popup) = notification(&store, id).unwrap();
        assert_eq!(updated.summary, "updated");
        assert!(updated.actions.is_empty());
        assert!(!show_popup);
    }

    #[test]
    fn popup_timeout_preserves_notifications_except_for_transient_items() {
        let now = Instant::now();
        let mut store = Store::default();
        let pixels: Arc<[u8]> = Arc::from([0, 0, 0, 0]);
        let regular = store.notify(Incoming {
            picture: Some(Picture::Pixels(Thumbnail {
                width: 1,
                height: 1,
                rowstride: 4,
                bytes: Arc::clone(&pixels),
            })),
            ..incoming("regular")
        });
        let transient = store.notify(Incoming {
            transient: true,
            ..Incoming::default()
        });
        assert_eq!(
            store
                .notifications()
                .map(|(notification, _)| notification.id)
                .collect::<Vec<_>>(),
            [transient, regular]
        );
        assert!(store.displayed(regular, 1, now));
        assert!(store.displayed(transient, 2, now));
        assert_eq!(
            store.hide_due_popups(now + POPUP_TIMEOUT),
            vec![(transient, CloseReason::Expired)]
        );
        let (retained, show_popup) = notification(&store, regular).unwrap();
        assert!(!show_popup);
        let Some(Picture::Pixels(thumbnail)) = &retained.picture else {
            panic!("expected pixel picture");
        };
        assert!(Arc::ptr_eq(&thumbnail.bytes, &pixels));
        assert!(notification(&store, transient).is_none());
    }

    #[test]
    fn sender_recall_removes_the_notification() {
        let mut store = Store::default();
        let id = store.notify(incoming("recalled"));

        assert!(store.close(id));
        assert_eq!(store.notifications().count(), 0);
    }

    #[test]
    fn notification_count_is_bounded() {
        let mut store = Store::default();
        let mut evicted = Vec::new();
        for index in 0..MAX_NOTIFICATIONS + 5 {
            if let Some((id, _)) = store.notify_with_eviction(incoming(&index.to_string())).1 {
                evicted.push(id);
            }
        }
        assert_eq!(store.notifications().count(), MAX_NOTIFICATIONS);
        assert_eq!(evicted.len(), 5);
        assert_eq!(store.notifications().next().unwrap().0.summary, "104");
    }

    #[test]
    fn replacement_at_capacity_does_not_evict() {
        let mut store = Store::default();
        let first = store.notify(incoming("first"));
        for index in 1..MAX_NOTIFICATIONS {
            store.notify(incoming(&index.to_string()));
        }
        let (id, evicted) = store.notify_with_eviction(Incoming {
            replaces_id: first,
            ..incoming("replacement")
        });
        assert_eq!(id, first);
        assert!(evicted.is_none());
        assert_eq!(store.notifications().count(), MAX_NOTIFICATIONS);
    }

    #[test]
    fn capacity_eviction_closes_the_oldest_notification() {
        let mut store = Store::default();
        let oldest = store.notify(incoming("oldest"));
        for index in 1..MAX_NOTIFICATIONS {
            store.notify(incoming(&index.to_string()));
        }
        let (_, evicted) = store.notify_with_eviction(incoming("new"));
        assert_eq!(evicted, Some((oldest, CloseReason::Undefined)));
        assert!(notification(&store, oldest).is_none());
        assert_eq!(store.notifications().count(), MAX_NOTIFICATIONS);
    }

    #[test]
    fn dnd_does_not_start_the_popup_timeout_before_display() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(incoming("hidden"));
        store.set_dnd(true);
        assert!(store.dnd());
        assert!(store.hide_due_popups(now + POPUP_TIMEOUT).is_empty());
        assert!(notification(&store, id).unwrap().1);
        assert!(store.displayed(id, 1, now + POPUP_TIMEOUT));
        assert!(store.hide_due_popups(now + POPUP_TIMEOUT * 2).is_empty());
        assert!(!notification(&store, id).unwrap().1);
    }
}
