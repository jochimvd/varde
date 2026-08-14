mod query;
mod state;

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::prelude::*;

use super::command::{StateClass, module, on_click, spawn_shell, spawn_shell_then_refresh, watch};
use state::TrafficSample;

const UPDATE_INTERVAL: Duration = Duration::from_secs(5);

pub fn network() -> gtk::Button {
    let (button, label) = module("network");
    let previous = Rc::new(RefCell::new(None::<TrafficSample>));
    let widget = button.clone();
    let mut class = StateClass::new(&button);
    let refresh = watch(UPDATE_INTERVAL, query::state, move |state| {
        class.set(state.class());
        label.set_text(state.icon());
        widget.set_tooltip_text(Some(&state.tooltip(previous.borrow().as_ref())));
        *previous.borrow_mut() = state.traffic_sample();
    });
    on_click(&button, move |mouse_button| match mouse_button {
        1 => spawn_shell(
            "hyprctl dispatch 'hl.dsp.exec_cmd(\"$TERMINAL -e impala\", { tag = \"+floating-window\" })'",
        ),
        3 => spawn_shell_then_refresh("rfkill toggle wlan", refresh.clone()),
        _ => {}
    });

    button
}
