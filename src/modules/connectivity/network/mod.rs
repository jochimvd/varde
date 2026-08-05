mod query;
mod state;

use std::{cell::Cell, rc::Rc, time::Duration};

use gtk::prelude::*;

use super::command::{module, on_click, set_state, spawn_shell, watch};
use state::TrafficSample;

const UPDATE_INTERVAL: Duration = Duration::from_secs(5);

pub fn network() -> gtk::Button {
    let (button, label) = module("network");
    on_click(&button, |mouse_button| {
        if mouse_button == 1 {
            spawn_shell(
                "hyprctl dispatch 'hl.dsp.exec_cmd(\"$TERMINAL -e impala\", { tag = \"+floating-window\" })'",
            );
        }
    });

    let previous = Rc::new(Cell::new(None::<TrafficSample>));
    let widget = button.clone();
    watch(UPDATE_INTERVAL, query::state, move |state| {
        set_state(&widget, state.class());
        label.set_text(state.icon());
        widget.set_tooltip_text(Some(&state.tooltip(previous.get())));
        previous.set(state.traffic_sample());
    });

    button
}
