use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    time::Duration,
};

use gtk::glib;
use gtk::prelude::*;

use super::command::{
    Refresh, command, module, on_click, set_state, spawn_shell, spawn_shell_then_refresh, watch,
};
use crate::background;

const INTERVAL: Duration = Duration::from_secs(30);

pub fn audio() -> gtk::Button {
    let (button, label) = module("audio");
    let widget = button.clone();
    let refresh = watch(INTERVAL, state, move |state| {
        set_state(&widget, if state.muted { "muted" } else { "" });
        label.set_text(state.text());
        widget.set_tooltip_text(Some(&format!("Volume: {}%", state.percent())));
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
    background::spawn("audio-events", move || {
        loop {
            let mut command = Command::new("pactl");
            command.arg("subscribe").stdout(Stdio::piped());
            // The subscriber must not outlive the shell when the compositor stops it.
            unsafe {
                command.pre_exec(|| {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            let Some(mut child) = command.spawn().ok() else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            let Some(output) = child.stdout.take() else {
                let _ = child.kill();
                continue;
            };
            for event in BufReader::new(output).lines().map_while(Result::ok) {
                if event.contains(" on sink ") || event.contains(" on server ") {
                    refresh.request();
                }
            }
            let _ = child.wait();
            std::thread::sleep(Duration::from_secs(1));
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
}

fn state() -> State {
    command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).map_or(
        State {
            volume: 0.0,
            muted: true,
        },
        |output| {
            parse_volume(&output).unwrap_or(State {
                volume: 0.0,
                muted: true,
            })
        },
    )
}

fn parse_volume(text: &str) -> Option<State> {
    let volume = text.split_whitespace().nth(1)?.parse().ok()?;
    Some(State {
        volume,
        muted: text.contains("[MUTED]"),
    })
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
}
