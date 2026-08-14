use std::{cell::Cell, rc::Rc, time::Duration};

use gtk::prelude::*;

use super::command::{
    StateClass, command, module, on_click, property, spawn_shell, spawn_shell_then_refresh,
    strip_ansi, watch,
};

const INTERVAL: Duration = Duration::from_secs(5);

pub fn bluetooth() -> gtk::Button {
    let (button, label) = module("bluetooth");
    let powered = Rc::new(Cell::new(None));
    let widget = button.clone();
    let mut class = StateClass::new(&button);
    let refresh = watch(INTERVAL, state, {
        let powered = Rc::clone(&powered);
        move |state| {
            powered.set(state.powered());
            class.set(state.class());
            label.set_text(state.icon());
            widget.set_tooltip_text(Some(&state.tooltip()));
            widget.set_visible(!matches!(state, State::NoController));
        }
    });
    on_click(&button, move |mouse_button| match mouse_button {
        1 => spawn_shell(
            "hyprctl dispatch 'hl.dsp.exec_cmd(\"$TERMINAL -e bluetui\", { tag = \"+floating-window\" })'",
        ),
        3 => {
            if let Some(powered) = powered.get() {
                let command = if powered {
                    "bluetoothctl power off"
                } else {
                    "bluetoothctl power on"
                };
                spawn_shell_then_refresh(command, refresh.clone());
            }
        }
        _ => {}
    });

    button
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    NoController,
    Disabled,
    Ready(Activity),
    Connected(Vec<Device>, Activity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Idle,
    Discoverable,
    Discovering,
}

#[derive(Debug, PartialEq, Eq)]
struct Device {
    alias: String,
    battery: Option<u8>,
}

impl State {
    fn powered(&self) -> Option<bool> {
        match self {
            Self::NoController => None,
            Self::Disabled => Some(false),
            Self::Ready(_) | Self::Connected(..) => Some(true),
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::NoController => "",
            Self::Disabled => "󰂲",
            Self::Ready(_) => "󰂯",
            Self::Connected(..) => "󰂱",
        }
    }

    fn class(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ready(activity) | Self::Connected(_, activity) => activity.class(),
            Self::NoController => "",
        }
    }

    fn tooltip(&self) -> String {
        match self {
            Self::NoController => String::new(),
            Self::Disabled => "Bluetooth disabled".into(),
            Self::Ready(_) => "No devices connected".into(),
            Self::Connected(devices, _) => {
                let devices = devices
                    .iter()
                    .map(|device| match device.battery {
                        Some(battery) => format!("{}  {battery}%", device.alias),
                        None => device.alias.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("Connected\n{devices}")
            }
        }
    }
}

impl Activity {
    fn class(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Discoverable => "discoverable",
            Self::Discovering => "discovering",
        }
    }
}

fn state() -> State {
    let Some(show) = command("bluetoothctl", &["show"]) else {
        return State::NoController;
    };
    let Some((powered, activity)) = parse_controller(&show) else {
        return State::NoController;
    };

    if !powered {
        return State::Disabled;
    }

    let devices = command("bluetoothctl", &["devices", "Connected"])
        .map(|output| parse_devices(&output))
        .unwrap_or_default()
        .into_iter()
        .map(|(address, alias)| Device {
            battery: command("bluetoothctl", &["info", &address])
                .and_then(|info| parse_battery(&info)),
            alias,
        })
        .collect::<Vec<_>>();

    if devices.is_empty() {
        State::Ready(activity)
    } else {
        State::Connected(devices, activity)
    }
}

fn parse_controller(text: &str) -> Option<(bool, Activity)> {
    let text = strip_ansi(text);
    let powered = property(&text, "Powered:")? == "yes";
    let activity = if property(&text, "Discovering:").is_some_and(|value| value == "yes") {
        Activity::Discovering
    } else if property(&text, "Discoverable:").is_some_and(|value| value == "yes") {
        Activity::Discoverable
    } else {
        Activity::Idle
    };
    Some((powered, activity))
}

fn parse_devices(text: &str) -> Vec<(String, String)> {
    strip_ansi(text)
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Device "))
        .filter_map(|line| line.split_once(' '))
        .map(|(address, alias)| (address.to_string(), alias.to_string()))
        .collect()
}

fn parse_battery(text: &str) -> Option<u8> {
    let battery = property(&strip_ansi(text), "Battery Percentage:")?;
    battery
        .split(['(', ')'])
        .nth(1)
        .or_else(|| battery.split_whitespace().next())?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_controller_devices_and_battery() {
        let controller = "Controller 00:11:22:33:44:55 (public)\n\tPowered: yes\n\tDiscoverable: yes\n\tDiscovering: no\n";
        assert_eq!(
            parse_controller(controller),
            Some((true, Activity::Discoverable))
        );
        assert_eq!(
            parse_devices("Device AA:BB:CC:DD:EE:FF Headphones\n"),
            vec![("AA:BB:CC:DD:EE:FF".into(), "Headphones".into())]
        );
        assert_eq!(parse_battery("Battery Percentage: 0x64 (100)"), Some(100));
    }

    #[test]
    fn tooltip_lists_connected_device_names() {
        let state = State::Connected(
            vec![
                Device {
                    alias: "Headphones".into(),
                    battery: Some(80),
                },
                Device {
                    alias: "Mouse".into(),
                    battery: None,
                },
            ],
            Activity::Idle,
        );
        assert_eq!(state.tooltip(), "Connected\nHeadphones  80%\nMouse");
    }

    #[test]
    fn discovering_takes_precedence_over_discoverable() {
        let controller = "Powered: yes\nDiscoverable: yes\nDiscovering: yes\n";
        assert_eq!(
            parse_controller(controller),
            Some((true, Activity::Discovering))
        );
    }
}
