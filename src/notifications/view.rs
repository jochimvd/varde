mod bell;
mod center;
mod common;
mod popups;

pub(super) use bell::Bell;
pub(super) use center::Center;
pub(super) use popups::Popups;

#[cfg(test)]
use center::update_group_order;
#[cfg(test)]
use popups::{MAX_POPUPS, PopupState};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_order_stays_stable_until_reset() {
        let strings = |keys: &[&str]| {
            keys.iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<_>>()
        };
        let mut order = strings(&["chat", "mail"]);

        update_group_order(&mut order, &strings(&["mail", "chat"]), false);
        assert_eq!(order, strings(&["chat", "mail"]));

        update_group_order(&mut order, &strings(&["news", "mail", "chat"]), false);
        assert_eq!(order, strings(&["chat", "mail", "news"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), false);
        assert_eq!(order, strings(&["mail", "news"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), true);
        assert_eq!(order, strings(&["news", "mail"]));
    }

    #[test]
    fn replacements_update_visible_popups_without_resurfacing_hidden_ones() {
        let first = super::super::model::parse(
            br#"[{"id":1,"revision":1,"app_name":"Test","summary":"Same"}]"#,
            b"[]",
            false,
        )
        .unwrap();
        let replaced = super::super::model::parse(
            br#"[{"id":1,"revision":2,"app_name":"Test","summary":"Same"}]"#,
            b"[]",
            false,
        )
        .unwrap();
        let empty = super::super::model::parse(b"[]", b"[]", false).unwrap();
        let mut state = PopupState::default();

        assert_eq!(state.update(&first, false), vec![(1, 1)]);
        assert!(state.visible.contains(&1));

        assert_eq!(state.update(&replaced, false), vec![(1, 2)]);
        assert!(state.visible.contains(&1));

        state.update(&replaced, true);
        assert!(state.visible.is_empty());
        assert!(state.update(&replaced, false).is_empty());
        assert!(state.visible.is_empty());

        state.update(&empty, false);
        assert_eq!(state.update(&first, false), vec![(1, 1)]);
        assert!(state.visible.contains(&1));
    }

    #[test]
    fn blocked_and_queued_notifications_wait_until_they_can_be_displayed() {
        let json = (1..=MAX_POPUPS + 1)
            .map(|id| format!(r#"{{"id":{id},"revision":1,"summary":"{id}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let snapshot =
            super::super::model::parse(format!("[{json}]").as_bytes(), b"[]", false).unwrap();
        let mut state = PopupState::default();

        assert!(state.update(&snapshot, true).is_empty());
        assert_eq!(state.update(&snapshot, false).len(), MAX_POPUPS);
        let first = *state.visible.iter().next().unwrap();
        let reduced = super::super::model::parse(
            format!(
                "[{}]",
                (1..=MAX_POPUPS + 1)
                    .filter(|id| *id != first as usize)
                    .map(|id| format!(r#"{{"id":{id},"revision":1,"summary":"{id}"}}"#))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .as_bytes(),
            b"[]",
            false,
        )
        .unwrap();
        assert_eq!(state.update(&reduced, false).len(), 1);
    }
}
