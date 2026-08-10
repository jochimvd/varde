use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) use super::image::Thumbnail;

const MAX_NOTIFICATIONS: usize = 100;
const LOW_TIMEOUT: Duration = Duration::from_secs(5);
const NORMAL_TIMEOUT: Duration = Duration::from_secs(10);

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Notification {
    pub id: u32,
    pub revision: u64,
    pub received_at: i64,
    pub app_name: String,
    pub app_icon: String,
    pub thumbnail: Option<Thumbnail>,
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
    pub thumbnail: Option<Thumbnail>,
    pub progress: Option<u8>,
    pub summary: String,
    pub body: String,
    pub actions: Vec<Action>,
    pub urgency: Urgency,
    pub desktop_entry: String,
    pub tag: String,
    pub transient: bool,
    pub resident: bool,
    pub timeout_ms: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Requested = 3,
}

#[derive(Clone, Debug)]
struct Active {
    notification: Notification,
    tag: String,
    transient: bool,
    timeout: Option<Duration>,
    expires_at: Option<Instant>,
}

#[derive(Default)]
pub(super) struct Store {
    next_id: u32,
    next_revision: u64,
    active: Vec<Active>,
    history: Vec<Notification>,
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
            .map(|index| self.active[index].notification.id)
            .unwrap_or_else(|| self.allocate_id());
        let revision = self.allocate_revision();
        let timeout = timeout(incoming.timeout_ms, incoming.urgency);
        let active = Active {
            notification: Notification {
                id,
                revision,
                received_at: unix_now(),
                app_name: incoming.app_name,
                app_icon: incoming.app_icon,
                thumbnail: incoming.thumbnail,
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
            timeout,
            expires_at: timeout
                .and_then(|_| replacement.and_then(|index| self.active[index].expires_at)),
        };

        if let Some(index) = replacement {
            self.active[index] = active;
        } else {
            self.active.insert(0, active);
        }
        (id, evicted)
    }

    pub fn close(&mut self, id: u32, keep_history: bool) -> bool {
        let Some(index) = self
            .active
            .iter()
            .position(|active| active.notification.id == id)
        else {
            return false;
        };
        let active = self.active.remove(index);
        if keep_history && !active.transient {
            self.history.insert(0, active.notification);
        }
        true
    }

    pub fn remove_history(&mut self, id: u32) -> bool {
        let Some(index) = self
            .history
            .iter()
            .position(|notification| notification.id == id)
        else {
            return false;
        };
        self.history.remove(index);
        true
    }

    pub fn expire(&mut self, now: Instant) -> Vec<(u32, CloseReason)> {
        let expired: Vec<_> = self
            .active
            .iter()
            .filter(|active| active.expires_at.is_some_and(|at| at <= now))
            .map(|active| active.notification.id)
            .collect();
        for id in &expired {
            self.close(*id, true);
        }
        expired
            .into_iter()
            .map(|id| (id, CloseReason::Expired))
            .collect()
    }

    pub fn displayed(&mut self, id: u32, revision: u64, now: Instant) -> bool {
        let Some(active) = self.active.iter_mut().find(|active| {
            active.notification.id == id && active.notification.revision == revision
        }) else {
            return false;
        };
        active.expires_at = active.timeout.and_then(|timeout| now.checked_add(timeout));
        true
    }

    pub fn next_expiration(&self) -> Option<Instant> {
        self.active
            .iter()
            .filter_map(|active| active.expires_at)
            .min()
    }

    pub fn clear(&mut self) -> Vec<(u32, CloseReason)> {
        let closed = self
            .active
            .iter()
            .map(|active| (active.notification.id, CloseReason::Dismissed))
            .collect();
        self.active.clear();
        self.history.clear();
        closed
    }

    pub fn set_dnd(&mut self, dnd: bool) {
        self.dnd = dnd;
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    pub fn active(&self) -> impl Iterator<Item = &Notification> {
        self.active.iter().map(|active| &active.notification)
    }

    pub fn has_action(&self, id: u32, key: &str) -> bool {
        self.active().chain(self.history()).any(|notification| {
            notification.id == id && notification.actions.iter().any(|action| action.key == key)
        })
    }

    pub fn is_active(&self, id: u32) -> bool {
        self.active().any(|notification| notification.id == id)
    }

    pub fn is_resident(&self, id: u32) -> bool {
        self.active()
            .chain(self.history())
            .any(|notification| notification.id == id && notification.resident)
    }

    pub fn history(&self) -> impl Iterator<Item = &Notification> {
        self.history.iter()
    }

    fn replacement_index(&self, incoming: &Incoming) -> Option<usize> {
        if incoming.replaces_id != 0
            && let Some(index) = self
                .active
                .iter()
                .position(|active| active.notification.id == incoming.replaces_id)
        {
            return Some(index);
        }
        if incoming.tag.is_empty() {
            return None;
        }
        self.active.iter().position(|active| {
            active.tag == incoming.tag && active.notification.app_name == incoming.app_name
        })
    }

    fn evict_at_capacity(&mut self) -> Option<(u32, CloseReason)> {
        if self.active.len() + self.history.len() < MAX_NOTIFICATIONS {
            return None;
        }
        let active = self
            .active
            .iter()
            .enumerate()
            .map(|(index, active)| (active.notification.revision, true, index));
        let history = self
            .history
            .iter()
            .enumerate()
            .map(|(index, notification)| (notification.revision, false, index));
        let (_, is_active, index) = active.chain(history).min_by_key(|item| item.0)?;
        if is_active {
            let id = self.active.remove(index).notification.id;
            Some((id, CloseReason::Expired))
        } else {
            self.history.remove(index);
            None
        }
    }

    fn allocate_id(&mut self) -> u32 {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            if self.next_id != 0
                && !self
                    .active
                    .iter()
                    .any(|active| active.notification.id == self.next_id)
                && !self
                    .history
                    .iter()
                    .any(|notification| notification.id == self.next_id)
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

fn timeout(requested_ms: i32, urgency: Urgency) -> Option<Duration> {
    match requested_ms {
        0 => None,
        timeout if timeout > 0 => Some(Duration::from_millis(timeout as u64)),
        _ => match urgency {
            Urgency::Low => Some(LOW_TIMEOUT),
            Urgency::Normal => Some(NORMAL_TIMEOUT),
            Urgency::Critical => None,
        },
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
            timeout_ms: -1,
            ..Incoming::default()
        }
    }

    #[test]
    fn allocates_ids_and_replaces_active_notifications_in_place() {
        let mut store = Store::default();
        let first = store.notify(incoming("first"));
        let second = store.notify(incoming("second"));

        let replacement = Incoming {
            replaces_id: first,
            summary: "updated".into(),
            timeout_ms: -1,
            ..Incoming::default()
        };
        assert_eq!(store.notify(replacement), first);
        assert_eq!(store.active().count(), 2);
        assert_eq!(store.active().nth(1).unwrap().summary, "updated");
        assert_ne!(first, second);
    }

    #[test]
    fn replacements_always_receive_a_new_revision() {
        let mut store = Store::default();
        let id = store.notify(incoming("same"));
        let revision = store.active().next().unwrap().revision;

        assert_eq!(
            store.notify(Incoming {
                replaces_id: id,
                ..incoming("same")
            }),
            id
        );
        assert!(store.active().next().unwrap().revision > revision);
    }

    #[test]
    fn invalid_replacement_ids_create_new_notifications() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            replaces_id: 42,
            timeout_ms: -1,
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
            timeout_ms: -1,
            ..Incoming::default()
        };
        let first = store.notify(tagged("one", "old"));
        let other = store.notify(tagged("two", "other"));
        assert_eq!(store.notify(tagged("one", "new")), first);
        assert_eq!(store.active().count(), 2);
        assert_ne!(first, other);
    }

    #[test]
    fn expiration_uses_current_system_defaults() {
        let now = Instant::now();
        let mut store = Store::default();
        let low = store.notify(Incoming {
            urgency: Urgency::Low,
            timeout_ms: -1,
            ..Incoming::default()
        });
        let normal = store.notify(incoming("normal"));
        let critical = store.notify(Incoming {
            urgency: Urgency::Critical,
            timeout_ms: -1,
            ..Incoming::default()
        });

        assert!(store.expire(now + NORMAL_TIMEOUT).is_empty());
        assert!(store.displayed(low, 1, now));
        assert!(store.displayed(normal, 2, now));
        assert!(store.displayed(critical, 3, now));

        assert_eq!(
            store.expire(now + LOW_TIMEOUT),
            vec![(low, CloseReason::Expired)]
        );
        assert_eq!(
            store.expire(now + NORMAL_TIMEOUT),
            vec![(normal, CloseReason::Expired)]
        );
        assert!(
            store
                .active()
                .any(|notification| notification.id == critical)
        );
    }

    #[test]
    fn zero_timeout_never_expires_and_positive_timeout_is_honored() {
        let now = Instant::now();
        let mut store = Store::default();
        let permanent = store.notify(Incoming::default());
        let short = store.notify(Incoming {
            timeout_ms: 25,
            ..Incoming::default()
        });
        assert!(store.displayed(permanent, 1, now));
        assert!(store.displayed(short, 2, now));
        assert_eq!(
            store.expire(now + Duration::from_millis(25)),
            vec![(short, CloseReason::Expired)]
        );
        assert!(
            store
                .active()
                .any(|notification| notification.id == permanent)
        );
    }

    #[test]
    fn queued_notifications_start_expiring_only_after_display() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming {
            timeout_ms: 25,
            ..Incoming::default()
        });
        let revision = store.active().next().unwrap().revision;

        assert!(store.expire(now + Duration::from_secs(1)).is_empty());
        assert!(store.displayed(id, revision, now + Duration::from_secs(1)));
        assert!(
            store
                .expire(now + Duration::from_secs(1) + Duration::from_millis(24))
                .is_empty()
        );
        assert_eq!(
            store.expire(now + Duration::from_secs(1) + Duration::from_millis(25)),
            vec![(id, CloseReason::Expired)]
        );
    }

    #[test]
    fn persistent_replacement_cancels_the_previous_expiration() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(Incoming {
            timeout_ms: 25,
            ..Incoming::default()
        });
        assert!(store.displayed(id, 1, now));
        store.notify(Incoming {
            replaces_id: id,
            timeout_ms: 0,
            ..Incoming::default()
        });

        assert!(store.expire(now + Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn dismissal_and_expiration_preserve_history_except_for_transient_items() {
        let mut store = Store::default();
        let pixels: Arc<[u8]> = Arc::from([0, 0, 0, 0]);
        let regular = store.notify(Incoming {
            thumbnail: Some(Thumbnail {
                width: 1,
                height: 1,
                rowstride: 4,
                bytes: Arc::clone(&pixels),
            }),
            ..incoming("regular")
        });
        let transient = store.notify(Incoming {
            transient: true,
            timeout_ms: -1,
            ..Incoming::default()
        });
        assert!(store.close(regular, true));
        assert!(store.close(transient, true));
        assert_eq!(
            store.history().map(|item| item.id).collect::<Vec<_>>(),
            [regular]
        );
        let archived = store.history().next().unwrap();
        assert!(Arc::ptr_eq(
            &archived.thumbnail.as_ref().unwrap().bytes,
            &pixels
        ));
    }

    #[test]
    fn sender_recall_does_not_preserve_history() {
        let mut store = Store::default();
        let id = store.notify(incoming("recalled"));

        assert!(store.close(id, false));
        assert_eq!(store.active().count(), 0);
        assert_eq!(store.history().count(), 0);
    }

    #[test]
    fn active_and_history_share_one_bound() {
        let mut store = Store::default();
        for index in 0..MAX_NOTIFICATIONS + 5 {
            let id = store.notify(incoming(&index.to_string()));
            store.close(id, true);
        }
        assert_eq!(
            store.active().count() + store.history().count(),
            MAX_NOTIFICATIONS
        );
        assert_eq!(store.history().next().unwrap().summary, "104");
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
        assert_eq!(store.active().count(), MAX_NOTIFICATIONS);
    }

    #[test]
    fn evicts_the_oldest_notification_across_active_and_history() {
        let mut store = Store::default();
        let oldest = store.notify(incoming("history"));
        store.close(oldest, true);
        for index in 1..MAX_NOTIFICATIONS {
            store.notify(incoming(&index.to_string()));
        }
        let (_, evicted) = store.notify_with_eviction(incoming("new"));
        assert!(evicted.is_none());
        assert!(
            !store
                .history()
                .any(|notification| notification.id == oldest)
        );
        assert_eq!(
            store.active().count() + store.history().count(),
            MAX_NOTIFICATIONS
        );
    }

    #[test]
    fn active_capacity_eviction_is_reported_as_expired() {
        let mut store = Store::default();
        let oldest = store.notify(incoming("oldest active"));
        let archived = store.notify(incoming("newer history"));
        store.close(archived, true);
        for index in 2..MAX_NOTIFICATIONS {
            store.notify(incoming(&index.to_string()));
        }
        let (_, evicted) = store.notify_with_eviction(incoming("new"));
        assert_eq!(evicted, Some((oldest, CloseReason::Expired)));
        assert!(!store.active().any(|notification| notification.id == oldest));
        assert!(
            store
                .history()
                .any(|notification| notification.id == archived)
        );
    }

    #[test]
    fn dnd_does_not_start_expiration_before_display() {
        let now = Instant::now();
        let mut store = Store::default();
        let id = store.notify(incoming("hidden"));
        store.set_dnd(true);
        assert!(store.dnd());
        assert!(store.expire(now + NORMAL_TIMEOUT).is_empty());
        assert!(store.displayed(id, 1, now + NORMAL_TIMEOUT));
        assert_eq!(
            store.expire(now + NORMAL_TIMEOUT * 2),
            vec![(id, CloseReason::Expired)]
        );
    }

    #[test]
    fn finds_actions_only_on_the_target_notification() {
        let mut store = Store::default();
        let id = store.notify(Incoming {
            actions: vec![Action {
                key: "reply".into(),
                label: "Reply".into(),
            }],
            ..Incoming::default()
        });
        let other = store.notify(Incoming::default());

        assert!(store.has_action(id, "reply"));
        assert!(!store.has_action(id, "default"));
        assert!(!store.has_action(other, "reply"));

        store.close(id, true);
        assert!(store.has_action(id, "reply"));
        assert!(!store.is_active(id));
    }
}
