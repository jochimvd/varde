use std::{process::Command, time::Duration};

use gtk::prelude::*;
use serde_json::Value;

use crate::background;

const UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const ICON_SIZE: i32 = 14;
const ICON_GAP: i32 = 4;

pub fn widget() -> gtk::Box {
    let privacy = gtk::Box::builder()
        .spacing(ICON_GAP)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    privacy.set_widget_name("privacy");
    privacy.add_css_class("module");
    privacy.add_css_class("privacy");

    background::repeat("privacy-state", UPDATE_INTERVAL, state, {
        let privacy = privacy.clone();
        move |state| update(&privacy, &state)
    });

    privacy
}

#[derive(Debug, Default, Eq, PartialEq)]
struct State {
    screenshares: Vec<String>,
    audio_inputs: Vec<String>,
}

fn state() -> State {
    let Ok(output) = Command::new("pw-dump").output() else {
        return State::default();
    };
    if !output.status.success() {
        return State::default();
    }
    parse_state(&String::from_utf8_lossy(&output.stdout))
}

fn parse_state(json: &str) -> State {
    let Ok(objects) = serde_json::from_str::<Vec<Value>>(json) else {
        return State::default();
    };

    let mut state = State::default();
    for object in objects {
        if object.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let info = object.get("info").unwrap_or(&Value::Null);
        if info.get("state").and_then(Value::as_str) != Some("running") {
            continue;
        }
        let props = info.get("props").unwrap_or(&Value::Null);
        if props.get("stream.monitor").is_some() {
            continue;
        }
        let name = stream_name(props);
        match props.get("media.class").and_then(Value::as_str) {
            Some("Stream/Input/Video") => state.screenshares.push(name),
            Some("Stream/Input/Audio") => state.audio_inputs.push(name),
            _ => {}
        }
    }
    state.screenshares.sort();
    state.screenshares.dedup();
    state.audio_inputs.sort();
    state.audio_inputs.dedup();
    state
}

fn stream_name(props: &Value) -> String {
    [
        "application.name",
        "node.description",
        "media.name",
        "node.name",
    ]
    .into_iter()
    .find_map(|property| props.get(property).and_then(Value::as_str))
    .unwrap_or("Unknown application")
    .to_string()
}

fn update(privacy: &gtk::Box, state: &State) {
    while let Some(child) = privacy.first_child() {
        privacy.remove(&child);
    }
    privacy.set_visible(!state.screenshares.is_empty() || !state.audio_inputs.is_empty());

    if !state.screenshares.is_empty() {
        privacy.append(&item(
            "screenshare",
            "video-display-symbolic",
            &format_tooltip("Screen sharing", &state.screenshares),
        ));
    }
    if !state.audio_inputs.is_empty() {
        privacy.append(&item(
            "audio-in",
            "audio-input-microphone-symbolic",
            &format_tooltip("Microphone in use", &state.audio_inputs),
        ));
    }
}

fn item(kind: &str, icon: &str, tooltip: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon);
    image.set_widget_name("privacy-item");
    image.add_css_class("privacy-item");
    image.add_css_class(kind);
    image.set_pixel_size(ICON_SIZE);
    image.set_size_request(ICON_SIZE, ICON_SIZE);
    image.set_tooltip_text(Some(tooltip));
    image
}

fn format_tooltip(kind: &str, names: &[String]) -> String {
    format!("{kind}:\n{}", names.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_running_privacy_streams() {
        let state = parse_state(
            r#"[
                {"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Video","application.name":"Firefox"}}},
                {"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","node.description":"Discord"}}},
                {"type":"PipeWire:Interface:Node","info":{"state":"suspended","props":{"media.class":"Stream/Input/Audio","application.name":"Ignored"}}},
                {"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","stream.monitor":"true","application.name":"Monitor"}}}
            ]"#,
        );

        assert_eq!(state.screenshares, ["Firefox"]);
        assert_eq!(state.audio_inputs, ["Discord"]);
    }

    #[test]
    fn uses_the_best_available_stream_name() {
        let props: Value =
            serde_json::from_str(r#"{"application.name":"Browser","node.description":"Ignored"}"#)
                .unwrap();

        assert_eq!(stream_name(&props), "Browser");
    }
}
