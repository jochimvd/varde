use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use gtk::{glib, prelude::*};

use super::set_critical;

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
    label.set_tooltip_text(Some(&battery.tooltip()));
    label.remove_css_class("charging");
    if battery.status == "Charging" {
        label.add_css_class("charging");
    }
    set_critical(label, battery.capacity < 20 && battery.status != "Charging");
}

struct Battery {
    capacity: u8,
    status: String,
    power_watts: Option<f64>,
    estimate: Option<Estimate>,
}

#[derive(Clone, Copy)]
enum Estimate {
    Empty(Duration),
    Full(Duration),
}

impl Battery {
    fn tooltip(&self) -> String {
        let mut lines = Vec::new();
        if let Some(estimate) = self.estimate {
            let (label, duration) = match estimate {
                Estimate::Empty(duration) => ("Remaining", duration),
                Estimate::Full(duration) => ("Full in", duration),
            };
            lines.push(format!("{label:<10} {}", format_duration(duration)));
        }
        if let Some(power) = self.power_watts {
            lines.push(format!("Power      {power:.1} W"));
        }
        lines.join("\n")
    }
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
    let estimate = estimate(path, &status);
    Some(Battery {
        capacity,
        status,
        power_watts,
        estimate,
    })
}

fn estimate(path: &Path, status: &str) -> Option<Estimate> {
    let time_field = match status {
        "Charging" => "time_to_full_now",
        "Discharging" => "time_to_empty_now",
        _ => return None,
    };
    if let Some(seconds) = read_u64(path, time_field).filter(|seconds| *seconds > 0) {
        let duration = Duration::from_secs(seconds);
        return Some(if status == "Charging" {
            Estimate::Full(duration)
        } else {
            Estimate::Empty(duration)
        });
    }

    let (now, full, rate) = [
        ("energy_now", "energy_full", "power_now"),
        ("charge_now", "charge_full", "current_now"),
    ]
    .into_iter()
    .find_map(|(now, full, rate)| {
        let rate = read_u64(path, rate)?;
        if rate == 0 {
            return None;
        }
        Some((read_u64(path, now)?, read_u64(path, full)?, rate))
    })?;
    let remaining = if status == "Charging" {
        full.saturating_sub(now)
    } else {
        now
    };
    let seconds = remaining.checked_mul(3600)? / rate;
    let duration = Duration::from_secs(seconds);
    (seconds > 0).then_some(if status == "Charging" {
        Estimate::Full(duration)
    } else {
        Estimate::Empty(duration)
    })
}

fn read_u64(path: &Path, field: &str) -> Option<u64> {
    fs::read_to_string(path.join(field))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn format_duration(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
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
    fn formats_battery_tooltip() {
        let battery = Battery {
            capacity: 55,
            status: "Discharging".into(),
            power_watts: Some(8.25),
            estimate: Some(Estimate::Empty(Duration::from_secs(3 * 3600 + 12 * 60))),
        };
        assert_eq!(battery.tooltip(), "Remaining  3h 12m\nPower      8.2 W");
    }
}
