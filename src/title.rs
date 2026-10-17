use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use gtk::glib;
use serde::Deserialize;

const MAX_TTL_MS: u64 = 3_600_000;
const MAX_ITEMS_DELAY_MS: u64 = 60_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    version: u8,
    pub id: String,
    #[serde(default)]
    pub icon: String,
    pub text: String,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub items_delay_ms: u64,
    #[serde(default)]
    ttl_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Item {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub pinned: bool,
}

pub fn parse(json: &str) -> Result<Overlay, String> {
    let overlay: Overlay =
        serde_json::from_str(json).map_err(|error| format!("invalid title JSON: {error}"))?;
    if overlay.version != 1 {
        return Err(format!("unsupported title version {}", overlay.version));
    }
    if overlay.id.is_empty() {
        return Err("title id must not be empty".into());
    }
    if overlay.ttl_ms > MAX_TTL_MS {
        return Err(format!("ttl_ms must be at most {MAX_TTL_MS}"));
    }
    if overlay.items_delay_ms > MAX_ITEMS_DELAY_MS {
        return Err(format!(
            "items_delay_ms must be at most {MAX_ITEMS_DELAY_MS}"
        ));
    }
    Ok(overlay)
}

#[derive(Default)]
struct Entry {
    seq: Option<u64>,
    generation: u64,
}

type Listener = Box<dyn Fn(Option<&Overlay>)>;

/// Text written to the bar's title by `varde title`. The most recently set
/// overlay is shown; without one the bar shows the active window title.
#[derive(Default)]
pub struct Manager {
    stack: RefCell<Vec<Overlay>>,
    entries: RefCell<HashMap<String, Entry>>,
    listener: RefCell<Option<Listener>>,
}

impl Manager {
    pub fn new() -> Rc<Self> {
        Rc::default()
    }

    pub fn connect(&self, listener: impl Fn(Option<&Overlay>) + 'static) {
        listener(self.stack.borrow().last());
        self.listener.replace(Some(Box::new(listener)));
    }

    pub fn set(self: &Rc<Self>, overlay: Overlay, seq: Option<u64>) {
        let Some(generation) = self.advance(&overlay.id, seq) else {
            return;
        };
        if overlay.ttl_ms > 0 {
            let manager = Rc::downgrade(self);
            let id = overlay.id.clone();
            glib::timeout_add_local_once(Duration::from_millis(overlay.ttl_ms), move || {
                if let Some(manager) = manager.upgrade() {
                    manager.expire(&id, generation);
                }
            });
        }
        let mut stack = self.stack.borrow_mut();
        stack.retain(|shown| shown.id != overlay.id);
        stack.push(overlay);
        drop(stack);
        self.notify();
    }

    pub fn clear(&self, id: &str, seq: Option<u64>) {
        if self.advance(id, seq).is_some() {
            self.remove(id);
        }
    }

    /// Records a change to `id`, returning its new generation, or `None` when
    /// `seq` is older than one already applied to that id.
    fn advance(&self, id: &str, seq: Option<u64>) -> Option<u64> {
        let mut entries = self.entries.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_default();
        if let (Some(seq), Some(last)) = (seq, entry.seq)
            && seq <= last
        {
            return None;
        }
        entry.seq = seq.or(entry.seq);
        entry.generation = entry.generation.wrapping_add(1);
        Some(entry.generation)
    }

    fn expire(&self, id: &str, generation: u64) {
        let current = self.entries.borrow().get(id).map(|entry| entry.generation);
        if current == Some(generation) {
            self.remove(id);
        }
    }

    fn remove(&self, id: &str) {
        let mut stack = self.stack.borrow_mut();
        let before = stack.len();
        stack.retain(|shown| shown.id != id);
        let changed = stack.len() != before;
        drop(stack);
        if changed {
            self.notify();
        }
    }

    fn notify(&self) {
        if let Some(listener) = self.listener.borrow().as_ref() {
            listener(self.stack.borrow().last());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(id: &str, text: &str) -> Overlay {
        parse(&format!(r#"{{"version":1,"id":"{id}","text":"{text}"}}"#)).unwrap()
    }

    fn shown(manager: &Manager) -> Option<String> {
        manager
            .stack
            .borrow()
            .last()
            .map(|shown| shown.text.clone())
    }

    #[test]
    fn parses_and_validates_overlays() {
        let parsed = parse(
            r#"{"version":1,"id":"submap","icon":"x","text":"Resize","ttl_ms":5,
                "items_delay_ms":300,"items":[{"key":"Esc","label":"Exit","pinned":true},{"key":"h"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.items.len(), 2);
        assert!(parsed.items[0].pinned);
        assert_eq!(parsed.items[1].label, "");
        for invalid in [
            r#"{"version":2,"id":"a","text":"t"}"#,
            r#"{"version":1,"id":"","text":"t"}"#,
            r#"{"version":1,"id":"a"}"#,
            r#"{"version":1,"id":"a","text":"t","color":"red"}"#,
            r#"{"version":1,"id":"a","text":"t","ttl_ms":3600001}"#,
            r#"{"version":1,"id":"a","text":"t","items_delay_ms":60001}"#,
            "not json",
        ] {
            assert!(parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn shows_the_latest_overlay_and_falls_back_on_clear() {
        let manager = Manager::new();
        manager.set(overlay("a", "first"), None);
        manager.set(overlay("b", "second"), None);
        assert_eq!(shown(&manager).as_deref(), Some("second"));
        manager.set(overlay("a", "again"), None);
        assert_eq!(shown(&manager).as_deref(), Some("again"));
        manager.clear("a", None);
        assert_eq!(shown(&manager).as_deref(), Some("second"));
        manager.clear("b", None);
        assert_eq!(shown(&manager), None);
    }

    #[test]
    fn ignores_out_of_order_sequence_numbers() {
        let manager = Manager::new();
        manager.set(overlay("submap", "Apps"), Some(5));
        manager.clear("submap", Some(4));
        assert_eq!(shown(&manager).as_deref(), Some("Apps"));
        manager.clear("submap", Some(6));
        manager.set(overlay("submap", "stale"), Some(6));
        assert_eq!(shown(&manager), None);
        manager.set(overlay("submap", "unordered"), None);
        assert_eq!(shown(&manager).as_deref(), Some("unordered"));
    }

    #[test]
    fn expiry_only_removes_the_overlay_it_was_scheduled_for() {
        let manager = Manager::new();
        manager.set(overlay("a", "first"), None);
        let first = manager.entries.borrow()["a"].generation;
        manager.set(overlay("a", "renewed"), None);
        manager.expire("a", first);
        assert_eq!(shown(&manager).as_deref(), Some("renewed"));
        let renewed = manager.entries.borrow()["a"].generation;
        manager.expire("a", renewed);
        assert_eq!(shown(&manager), None);
    }

    #[test]
    fn notifies_the_listener_with_the_shown_overlay() {
        let manager = Manager::new();
        let seen = Rc::new(RefCell::new(Vec::new()));
        manager.connect({
            let seen = seen.clone();
            move |shown| {
                seen.borrow_mut()
                    .push(shown.map(|shown| shown.text.clone()))
            }
        });
        manager.set(overlay("a", "one"), None);
        manager.clear("a", None);
        manager.clear("a", None);
        assert_eq!(*seen.borrow(), [None, Some("one".into()), None]);
    }
}
