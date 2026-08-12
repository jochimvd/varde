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
    fn new_groups_go_first_while_existing_groups_stay_stable() {
        let strings = |keys: &[&str]| {
            keys.iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<_>>()
        };
        let mut order = strings(&["chat", "mail"]);

        update_group_order(&mut order, &strings(&["mail", "chat"]), false);
        assert_eq!(order, strings(&["chat", "mail"]));

        update_group_order(&mut order, &strings(&["news", "mail", "chat"]), false);
        assert_eq!(order, strings(&["news", "chat", "mail"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), false);
        assert_eq!(order, strings(&["news", "mail"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), true);
        assert_eq!(order, strings(&["news", "mail"]));
    }

    #[test]
    fn replacements_update_visible_popups_and_blocked_popups_do_not_resurface() {
        let first = super::super::model::test_snapshot(&[(1, 1)]);
        let replaced = super::super::model::test_snapshot(&[(1, 2)]);
        let empty = super::super::model::test_snapshot(&[]);
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
    fn hidden_notifications_and_their_replacements_do_not_resurface() {
        let visible = super::super::model::test_snapshot_with_popup(&[(1, 1, true)]);
        let hidden = super::super::model::test_snapshot_with_popup(&[(1, 1, false)]);
        let replaced = super::super::model::test_snapshot_with_popup(&[(1, 2, false)]);
        let mut state = PopupState::default();

        assert_eq!(state.update(&visible, false), vec![(1, 1)]);
        assert!(state.visible.contains(&1));

        assert!(state.update(&hidden, false).is_empty());
        assert!(!state.visible.contains(&1));
        assert!(state.update(&replaced, false).is_empty());
        assert!(!state.visible.contains(&1));
    }

    #[test]
    fn blocked_and_queued_notifications_wait_until_they_can_be_displayed() {
        let notifications = (1..=MAX_POPUPS as u32 + 1)
            .map(|id| (id, 1))
            .collect::<Vec<_>>();
        let snapshot = super::super::model::test_snapshot(&notifications);
        let mut state = PopupState::default();

        assert!(state.update(&snapshot, true).is_empty());
        assert_eq!(state.update(&snapshot, false).len(), MAX_POPUPS);
        let first = *state.visible.iter().next().unwrap();
        let reduced = super::super::model::test_snapshot(
            &notifications
                .into_iter()
                .filter(|(id, _)| *id != first)
                .collect::<Vec<_>>(),
        );
        assert_eq!(state.update(&reduced, false).len(), 1);
    }

    #[test]
    fn opening_the_center_consumes_visible_popups_then_shows_the_next_batch() {
        let notifications = (1..=MAX_POPUPS as u32 * 2)
            .rev()
            .map(|id| (id, u64::from(id)))
            .collect::<Vec<_>>();
        let snapshot = super::super::model::test_snapshot(&notifications);
        let mut state = PopupState::default();

        assert_eq!(
            state.update(&snapshot, false),
            vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]
        );
        assert!(state.update(&snapshot, true).is_empty());
        assert!(state.visible.is_empty());
        assert_eq!(
            state.update(&snapshot, false),
            vec![(6, 6), (7, 7), (8, 8), (9, 9), (10, 10)]
        );
    }

    #[test]
    fn popup_queue_reveals_oldest_undisplayed_notifications_first() {
        let notifications = (1..=10)
            .rev()
            .map(|id| (id, u64::from(id)))
            .collect::<Vec<_>>();
        let mut state = PopupState::default();

        assert_eq!(
            state.update(&super::super::model::test_snapshot(&notifications), false),
            vec![(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]
        );

        let remaining = notifications
            .into_iter()
            .filter(|(id, _)| *id != 1)
            .collect::<Vec<_>>();
        assert_eq!(
            state.update(&super::super::model::test_snapshot(&remaining), false),
            vec![(6, 6)]
        );
    }
}
