mod battery;
mod clock;
mod resources;

use gtk::prelude::*;

use crate::bar::Updates;

pub use battery::Battery;
pub use resources::{CpuUsage, Memory};

pub struct System {
    battery: bool,
}

impl System {
    pub fn new(updates: &Updates) -> Self {
        resources::watch(updates);
        Self {
            battery: battery::watch(updates),
        }
    }

    pub fn widgets(&self) -> Widgets {
        let date = clock::date();
        let time = clock::time();
        let center = module_box();
        center.set_spacing(crate::bar::CENTER_GAP);
        center.append(&date.label);
        center.append(&time.label);

        let cpu = resources::cpu();
        let memory = resources::memory();
        let battery = self.battery.then(battery::widget);
        let right = module_box();
        right.set_spacing(crate::bar::MODULE_GAP);
        right.append(&cpu);
        right.append(&memory);
        if let Some(battery) = &battery {
            right.append(battery);
        }
        Widgets {
            center,
            right,
            date,
            time,
            cpu,
            memory,
            battery,
        }
    }
}

/// One bar's clock, resource, and battery readings.
pub struct Widgets {
    pub center: gtk::Box,
    pub right: gtk::Box,
    date: clock::Clock,
    time: clock::Clock,
    cpu: gtk::Label,
    memory: gtk::Label,
    battery: Option<gtk::Label>,
}

impl Widgets {
    pub fn tick(&self) {
        self.date.show();
        self.time.show();
    }

    pub fn show_cpu(&self, usage: &CpuUsage) {
        resources::show_cpu(&self.cpu, usage);
    }

    pub fn show_memory(&self, memory: &Memory) {
        resources::show_memory(&self.memory, memory);
    }

    pub fn show_battery(&self, battery: &Battery) {
        if let Some(label) = &self.battery {
            battery::show(label, battery);
        }
    }
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
