use std::{
    fs,
    path::{Path, PathBuf},
};

use gtk::{glib, prelude::*};

const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub fn widget() -> Option<gtk::Label> {
    let path = battery_path()?;
    let label = gtk::Label::new(None);
    label.set_widget_name("battery");
    label.add_css_class("battery");
    update(&label, &path);
    glib::timeout_add_local(UPDATE_INTERVAL, {
        let label = label.clone();
        move || {
            update(&label, &path);
            glib::ControlFlow::Continue
        }
    });
    Some(label)
}

fn update(label: &gtk::Label, path: &Path) {
    let Some(battery) = read(path) else {
        return;
    };
    label.set_text(&format!("{} {:>2}%", icon(&battery), battery.capacity));
    let direction = if battery.status == "Charging" {
        '↑'
    } else {
        '↓'
    };
    let power = battery
        .power_watts
        .map(|power| format!("{power:.0}W{direction} "));
    label.set_tooltip_text(Some(&format!(
        "{}{}% ({})",
        power.unwrap_or_default(),
        battery.capacity,
        battery.status
    )));
    label.remove_css_class("charging");
    if battery.status == "Charging" {
        label.add_css_class("charging");
    }
    set_critical(label, battery.capacity < 20 && battery.status != "Charging");
}

fn set_critical(label: &gtk::Label, critical: bool) {
    if critical {
        label.add_css_class("critical");
    } else {
        label.remove_css_class("critical");
    }
}

struct Battery {
    capacity: u8,
    status: String,
    power_watts: Option<f64>,
}

fn battery_path() -> Option<PathBuf> {
    fs::read_dir("/sys/class/power_supply")
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            fs::read_to_string(path.join("type")).is_ok_and(|kind| kind.trim() == "Battery")
        })
}

fn read(path: &Path) -> Option<Battery> {
    let capacity = fs::read_to_string(path.join("capacity"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let status = fs::read_to_string(path.join("status"))
        .ok()?
        .trim()
        .to_owned();
    let power_watts = fs::read_to_string(path.join("power_now"))
        .ok()
        .and_then(|watts| watts.trim().parse::<f64>().ok())
        .map(|watts| watts / 1_000_000.0);
    Some(Battery {
        capacity,
        status,
        power_watts,
    })
}

fn icon(battery: &Battery) -> &'static str {
    if battery.status == "Charging" {
        return "󰂄";
    }
    if battery.status == "Full" {
        return "󰁹";
    }
    if battery.status != "Discharging" {
        return "󰚥";
    }
    if battery.capacity < 20 {
        return "󰂃";
    }
    const ICONS: [&str; 11] = ["󰂎", "󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"];
    ICONS[(battery.capacity as usize / 10).min(10)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_waybar_battery_icons() {
        assert_eq!(
            icon(&Battery {
                capacity: 10,
                status: "Discharging".into(),
                power_watts: None
            }),
            "󰂃"
        );
        assert_eq!(
            icon(&Battery {
                capacity: 55,
                status: "Discharging".into(),
                power_watts: None
            }),
            "󰁾"
        );
        assert_eq!(
            icon(&Battery {
                capacity: 55,
                status: "Charging".into(),
                power_watts: None
            }),
            "󰂄"
        );
    }
}
