mod battery;
mod clock;
mod resources;

use gtk::prelude::*;

const MODULE_GAP: i32 = 10;

pub struct SystemWidgets {
    pub center: gtk::Box,
    pub right: gtk::Box,
}

pub fn widgets() -> SystemWidgets {
    let center = module_box();
    center.append(&clock::date());
    center.append(&clock::time());

    let right = module_box();
    right.set_spacing(MODULE_GAP);
    right.append(&resources::cpu());
    right.append(&resources::memory());
    if let Some(battery) = battery::widget() {
        right.append(&battery);
    }
    SystemWidgets { center, right }
}

/// Marks a reading that has crossed its warning threshold.
fn set_critical(label: &gtk::Label, critical: bool) {
    if critical {
        label.add_css_class("critical");
    } else {
        label.remove_css_class("critical");
    }
}

fn module_box() -> gtk::Box {
    gtk::Box::builder()
        .spacing(0)
        .valign(gtk::Align::Center)
        .build()
}
