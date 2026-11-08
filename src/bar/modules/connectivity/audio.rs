use std::{rc::Rc, time::Duration};

use gtk::glib;
use gtk::prelude::*;

use super::command::{Button, Refresh, Shown, command, on_click, spawn_shell_then_refresh, watch};
use crate::background;
use crate::bar::{Msg, Updates, modules::open_floating};
use crate::osd::{Osd, Shown as OsdShown};

const INTERVAL: Duration = Duration::from_secs(30);
const DEVICE_NAME_LIMIT: usize = 16;
const VOLUME_BAR_WIDTH: usize = 10;

pub struct Audio {
    refresh: Refresh,
}

impl Audio {
    pub fn new(updates: &Updates, osd: &Rc<Osd>) -> Self {
        let mut previous = None;
        let refresh = watch(INTERVAL, state, {
            let updates = updates.clone();
            let osd = osd.clone();
            move |current: Option<State>| {
                if let (Some(before), Some(after)) = (&previous, &current)
                    && let Some(shown) = change(before, after)
                {
                    osd.show(&shown);
                }
                let state = current.as_ref().unwrap_or(&State::UNAVAILABLE);
                updates.send(Msg::Audio(Shown {
                    class: if state.muted { "muted" } else { "" },
                    text: state.text(),
                    tooltip: state.tooltip(),
                    visible: true,
                }));
                if current.is_some() {
                    previous = current;
                }
            }
        });
        subscribe(refresh.clone());
        Self { refresh }
    }

    pub fn widget(&self) -> Button {
        let button = Button::new("audio");
        on_click(&button.button, {
            let refresh = self.refresh.clone();
            move |mouse_button| match mouse_button {
                1 => open_floating("pavucontrol -t 3"),
                2 => spawn_shell_then_refresh("dot-menu-audio-switcher --cycle", refresh.clone()),
                3 => spawn_shell_then_refresh(
                    "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
                    refresh.clone(),
                ),
                _ => {}
            }
        });
        on_scroll(&button.button, self.refresh.clone());
        button
    }
}

/// wpctl has no event stream, so PipeWire's own monitor reports when a sink, a
/// source, or the default devices change.
fn subscribe(refresh: Refresh) {
    background::watch_lines(
        "audio-events",
        "pw-dump",
        &["--monitor", "--no-colors"],
        move |line| {
            if line.contains("\"media.class\": \"Audio/Sink\"")
                || line.contains("\"media.class\": \"Audio/Source\"")
                || line.contains("\"type\": \"PipeWire:Interface:Metadata\"")
            {
                refresh.request();
            }
        },
    );
}

fn on_scroll(button: &gtk::Button, refresh: Refresh) {
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
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

#[derive(Clone)]
struct State {
    volume: f32,
    muted: bool,
    device: Option<String>,
    mic_muted: Option<bool>,
}

impl State {
    const UNAVAILABLE: Self = Self {
        volume: 0.0,
        muted: true,
        device: None,
        mic_muted: None,
    };

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

fn state() -> Option<State> {
    let output = command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    let (volume, muted) = parse_volume(&output)?;
    Some(State {
        volume,
        muted,
        device: command("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"])
            .and_then(|output| parse_device(&output)),
        mic_muted: command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .and_then(|output| parse_volume(&output))
            .map(|(_, muted)| muted),
    })
}

/// What the OSD confirms about the change from `before` to `after`, if anything.
fn change(before: &State, after: &State) -> Option<OsdShown> {
    let switched = after.device.is_some() && after.device != before.device;
    if switched || after.percent() != before.percent() || after.muted != before.muted {
        return Some(OsdShown {
            icon: after.text().into(),
            text: switched.then(|| after.device.clone()).flatten(),
            level: Some(after.percent()),
            muted: after.muted,
        });
    }
    let (Some(was), Some(muted)) = (before.mic_muted, after.mic_muted) else {
        return None;
    };
    (was != muted).then(|| OsdShown {
        icon: if muted { "󰍭" } else { "󰍬" }.into(),
        text: Some(
            if muted {
                "Microphone muted"
            } else {
                "Microphone on"
            }
            .into(),
        ),
        level: None,
        muted,
    })
}

fn parse_volume(text: &str) -> Option<(f32, bool)> {
    let volume = text.split_whitespace().nth(1)?.parse().ok()?;
    Some((volume, text.contains("[MUTED]")))
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

    fn output(volume: f32, muted: bool, device: &str) -> State {
        State {
            volume,
            muted,
            device: Some(device.into()),
            mic_muted: Some(false),
        }
    }

    #[test]
    fn parses_muted_volume() {
        assert_eq!(parse_volume("Volume: 0.50 [MUTED]"), Some((0.5, true)));
        assert_eq!(parse_volume("Volume: 0.40"), Some((0.4, false)));
    }

    #[test]
    fn confirms_volume_mute_and_device_changes() {
        let before = output(0.5, false, "Speakers");
        assert_eq!(change(&before, &before.clone()), None);
        let louder = change(&before, &output(0.55, false, "Speakers")).unwrap();
        assert_eq!((louder.level, louder.text), (Some(55), None));
        assert!(
            change(&before, &output(0.5, true, "Speakers"))
                .unwrap()
                .muted
        );
        let switched = change(&before, &output(0.5, false, "Headphones")).unwrap();
        assert_eq!(switched.text.as_deref(), Some("Headphones"));
    }

    #[test]
    fn confirms_microphone_mute_only_between_known_states() {
        let before = output(0.5, false, "Speakers");
        let muted = State {
            mic_muted: Some(true),
            ..before.clone()
        };
        assert_eq!(
            change(&before, &muted).unwrap().text.as_deref(),
            Some("Microphone muted")
        );
        let unknown = State {
            mic_muted: None,
            ..before.clone()
        };
        assert_eq!(change(&unknown, &muted), None);
        assert_eq!(change(&muted, &unknown), None);
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
