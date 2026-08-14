use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use super::command::{
    Refresh, StateClass, command, module, on_click, spawn_shell, spawn_shell_then_refresh, watch,
};
use crate::background;

const INTERVAL: Duration = Duration::from_secs(30);
const DEVICE_NAME_LIMIT: usize = 16;
const VOLUME_BAR_WIDTH: usize = 10;

pub fn audio() -> gtk::Button {
    let (button, label) = module("audio");
    let widget = button.clone();
    let mut class = StateClass::new(&button);
    let refresh = watch(INTERVAL, state, move |state| {
        class.set(if state.muted { "muted" } else { "" });
        label.set_text(state.text());
        widget.set_tooltip_text(Some(&state.tooltip()));
    });
    subscribe(refresh.clone());
    on_click(&button, {
        let refresh = refresh.clone();
        move |mouse_button| match mouse_button {
            1 => spawn_shell(
                "hyprctl dispatch 'hl.dsp.exec_cmd(\"pavucontrol -t 3\", { tag = \"+floating-window\" })'",
            ),
            2 => spawn_shell_then_refresh("dot-menu-audio-switcher --cycle", refresh.clone()),
            3 => spawn_shell_then_refresh(
                "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
                refresh.clone(),
            ),
            _ => {}
        }
    });
    on_scroll(&button, refresh);

    button
}

fn subscribe(refresh: Refresh) {
    background::watch_lines("audio-events", "pactl", &["subscribe"], move |event| {
        if event.contains(" on sink ") || event.contains(" on server ") {
            refresh.request();
        }
    });
}

fn on_scroll(button: &gtk::Button, refresh: Refresh) {
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll(move |_, _, dy| {
        if dy < 0.0 {
            spawn_shell_then_refresh(
                "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 5%+",
                refresh.clone(),
            );
        } else if dy > 0.0 {
            spawn_shell_then_refresh("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-", refresh.clone());
        }
        glib::Propagation::Stop
    });
    button.add_controller(scroll);
}

struct State {
    volume: f32,
    muted: bool,
    device: Option<String>,
}

impl State {
    fn percent(&self) -> u8 {
        (self.volume * 100.0).round().clamp(0.0, 100.0) as u8
    }

    fn text(&self) -> &'static str {
        if self.muted {
            "󰖁"
        } else {
            match self.percent() {
                0..=33 => "󰕿",
                34..=66 => "󰖀",
                _ => "󰕾",
            }
        }
    }

    fn tooltip(&self) -> String {
        let percent = self.percent();
        let volume = if self.muted {
            format!("{}% muted", percent)
        } else {
            format!("{percent}%")
        };
        let volume = format!("Volume  {} {volume}", volume_bar(percent));
        match &self.device {
            Some(device) => format!("Output  {}\n{volume}", truncate(device, DEVICE_NAME_LIMIT)),
            None => volume,
        }
    }
}

fn state() -> State {
    let mut state = command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).map_or(
        State {
            volume: 0.0,
            muted: true,
            device: None,
        },
        |output| {
            parse_volume(&output).unwrap_or(State {
                volume: 0.0,
                muted: true,
                device: None,
            })
        },
    );
    state.device = command("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"])
        .and_then(|output| parse_device(&output));
    state
}

fn parse_volume(text: &str) -> Option<State> {
    let volume = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(State {
        volume,
        muted: text.contains("[MUTED]"),
        device: None,
    })
}

fn parse_device(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .trim_start_matches("* ")
            .strip_prefix("node.description = ")
            .map(|value| value.trim_matches('"').to_owned())
    })
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.into();
    }
    text.chars().take(limit - 1).chain(['…']).collect()
}

fn volume_bar(percent: u8) -> String {
    let filled = ((usize::from(percent) + 5) / 10).min(VOLUME_BAR_WIDTH);
    format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(VOLUME_BAR_WIDTH - filled)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_muted_volume() {
        let state = parse_volume("Volume: 0.50 [MUTED]").unwrap();
        assert_eq!(state.percent(), 50);
        assert!(state.muted);
    }

    #[test]
    fn parses_output_device() {
        assert_eq!(
            parse_device("  * node.description = \"Scarlett 2i2 Headphones\"\n"),
            Some("Scarlett 2i2 Headphones".into())
        );
    }

    #[test]
    fn caps_output_device_name() {
        assert_eq!(
            truncate("Scarlett 2i2 4th Gen Headphones / Line 1-2", 16),
            "Scarlett 2i2 4t…"
        );
    }

    #[test]
    fn formats_volume_bar() {
        assert_eq!(volume_bar(0), "[----------]");
        assert_eq!(volume_bar(50), "[#####-----]");
        assert_eq!(volume_bar(100), "[##########]");
    }
}
