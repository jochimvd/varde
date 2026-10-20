mod query;
mod state;

use std::time::Duration;

use super::command::{Button, Refresh, Shown, on_click, spawn_shell_then_refresh, watch};
use crate::bar::{Msg, Updates, modules::open_floating};

const UPDATE_INTERVAL: Duration = Duration::from_secs(5);

pub struct Network {
    refresh: Refresh,
}

impl Network {
    pub fn new(updates: &Updates) -> Self {
        let mut previous = None;
        let refresh = watch(UPDATE_INTERVAL, query::state, {
            let updates = updates.clone();
            move |state| {
                updates.send(Msg::Network(Shown {
                    class: state.class(),
                    text: state.icon(),
                    tooltip: state.tooltip(previous.as_ref()),
                    visible: true,
                }));
                previous = state.traffic_sample();
            }
        });
        Self { refresh }
    }

    pub fn widget(&self) -> Button {
        let button = Button::new("network");
        let refresh = self.refresh.clone();
        on_click(&button.button, move |mouse_button| match mouse_button {
            1 => open_floating("$TERMINAL -e impala"),
            3 => spawn_shell_then_refresh("rfkill toggle wlan", refresh.clone()),
            _ => {}
        });
        button
    }
}
