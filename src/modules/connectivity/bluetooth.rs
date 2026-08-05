use std::time::Duration;

use gtk::prelude::*;

use super::command::{command, module, on_click, set_state, spawn_shell, strip_ansi, watch};

const INTERVAL: Duration = Duration::from_secs(5);

pub fn bluetooth() -> gtk::Button {
    let (button, label) = module("bluetooth");
    on_click(&button, |mouse_button| {
        if mouse_button == 1 {
            spawn_shell(
                "hyprctl dispatch 'hl.dsp.exec_cmd(\"$TERMINAL -e bluetui\", { tag = \"+floating-window\" })'",
            );
        }
    });

    let widget = button.clone();
    watch(INTERVAL, state, move |state| {
        set_state(&widget, &state.class());
        label.set_text(state.icon());
        widget.set_tooltip_text(Some(&state.tooltip()));
        widget.set_visible(!matches!(state, State::NoController));
    });

    button
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    NoController,
    Disabled {
        alias: String,
        address: String,
    },
    Ready {
        alias: String,
        address: String,
    },
    Connected {
        alias: String,
        address: String,
        devices: Vec<Device>,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct Device {
    address: String,
    alias: String,
    battery: Option<u8>,
}

impl State {
    fn icon(&self) -> &'static str {
        match self {
            Self::NoController => "",
            Self::Disabled { .. } => "󰂲",
            Self::Ready { .. } => "󰂯",
            Self::Connected { .. } => "󰂱",
        }
    }

    fn class(&self) -> String {
        match self {
            Self::Disabled { .. } => "disabled".into(),
            _ => String::new(),
        }
    }

    fn tooltip(&self) -> String {
        match self {
            Self::NoController => String::new(),
            Self::Disabled { alias, address } | Self::Ready { alias, address } => {
                format!("{alias}\t{address}\n0 connected")
            }
            Self::Connected {
                alias,
                address,
                devices,
            } => {
                let devices = devices
                    .iter()
                    .map(|device| match device.battery {
                        Some(battery) => {
                            format!("{}\t{}\t{battery}%", device.alias, device.address)
                        }
                        None => format!("{}\t{}", device.alias, device.address),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "{alias}\t{address}\n{} connected\n\n{devices}",
                    devices.lines().count()
                )
            }
        }
    }
}

fn state() -> State {
    let Some(show) = command("bluetoothctl", &["show"]) else {
        return State::NoController;
    };
    let Some((address, alias, powered)) = parse_controller(&show) else {
        return State::NoController;
    };

    if !powered {
        return State::Disabled { alias, address };
    }

    let devices = command("bluetoothctl", &["devices", "Connected"])
        .map(|output| parse_devices(&output))
        .unwrap_or_default()
        .into_iter()
        .map(|(address, alias)| Device {
            battery: command("bluetoothctl", &["info", &address])
                .and_then(|info| parse_battery(&info)),
            address,
            alias,
        })
        .collect::<Vec<_>>();

    if devices.is_empty() {
        State::Ready { alias, address }
    } else {
        State::Connected {
            alias,
            address,
            devices,
        }
    }
}

fn parse_controller(text: &str) -> Option<(String, String, bool)> {
    let text = strip_ansi(text);
    let first = text.lines().next()?.split_whitespace().collect::<Vec<_>>();
    let address = first.get(1)?.to_string();
    let alias = property(&text, "Alias:")?;
    let powered = property(&text, "Powered:").is_some_and(|value| value == "yes");
    Some((address, alias, powered))
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

fn property(text: &str, name: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(name).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_controller_devices_and_battery() {
        let controller =
            "Controller 00:11:22:33:44:55 (public)\n\tAlias: desktop\n\tPowered: yes\n";
        assert_eq!(
            parse_controller(controller),
            Some(("00:11:22:33:44:55".into(), "desktop".into(), true))
        );
        assert_eq!(
            parse_devices("Device AA:BB:CC:DD:EE:FF Headphones\n"),
            vec![("AA:BB:CC:DD:EE:FF".into(), "Headphones".into())]
        );
        assert_eq!(parse_battery("Battery Percentage: 0x64 (100)"), Some(100));
    }
}
