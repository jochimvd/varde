use std::{collections::HashMap, io::BufReader, thread};

use gtk::prelude::*;
use serde_json::Value;

use crate::background;

const ICON_SIZE: i32 = 14;
const ICON_GAP: i32 = 4;
const NODE_TYPE: &str = "PipeWire:Interface:Node";

pub fn widget() -> gtk::Box {
    let privacy = gtk::Box::builder()
        .spacing(ICON_GAP)
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    privacy.set_widget_name("privacy");
    privacy.add_css_class("module");
    privacy.add_css_class("privacy");

    let (sender, receiver) = async_channel::unbounded();
    background::listen(receiver, {
        let privacy = privacy.clone();
        let mut current = State::default();
        move |state: State| {
            if state != current {
                update(&privacy, &state);
                current = state;
            }
        }
    });

    background::spawn("privacy-monitor", move || {
        while monitor(&sender) {
            thread::sleep(background::RETRY_DELAY);
        }
    });

    privacy
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    screenshares: Vec<String>,
    audio_inputs: Vec<String>,
}

#[derive(Clone)]
enum Stream {
    Screen(String),
    AudioInput(String),
}

fn monitor(sender: &async_channel::Sender<State>) -> bool {
    let Some(mut child) = background::spawn_child(
        "pw-dump",
        &["--monitor", "--no-colors", "--indent", "0", NODE_TYPE],
    ) else {
        return true;
    };
    let Some(output) = child.stdout.take() else {
        background::kill(&mut child);
        return true;
    };

    let batches =
        serde_json::Deserializer::from_reader(BufReader::new(output)).into_iter::<Vec<Value>>();
    let mut streams = HashMap::new();
    let mut current = None;
    for batch in batches {
        let Ok(objects) = batch else {
            break;
        };
        apply_objects(&mut streams, &objects);
        let state = stream_state(&streams);
        if current.as_ref() != Some(&state) {
            if sender.send_blocking(state.clone()).is_err() {
                background::kill(&mut child);
                return false;
            }
            current = Some(state);
        }
    }
    background::kill(&mut child);
    true
}

fn apply_objects(streams: &mut HashMap<u64, Stream>, objects: &[Value]) {
    for object in objects {
        let Some(id) = object.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(stream) = privacy_stream(object) {
            streams.insert(id, stream);
        } else {
            streams.remove(&id);
        }
    }
}

fn privacy_stream(object: &Value) -> Option<Stream> {
    if object.get("type").and_then(Value::as_str) != Some(NODE_TYPE) {
        return None;
    }
    let info = object.get("info")?;
    if info.get("state").and_then(Value::as_str) != Some("running") {
        return None;
    }
    let props = info.get("props")?;
    if props.get("stream.monitor").is_some() {
        return None;
    }
    let name = stream_name(props);
    match props.get("media.class").and_then(Value::as_str) {
        Some("Stream/Input/Video") => Some(Stream::Screen(name)),
        Some("Stream/Input/Audio") => Some(Stream::AudioInput(name)),
        _ => None,
    }
}

fn stream_state(streams: &HashMap<u64, Stream>) -> State {
    let mut state = State::default();
    for stream in streams.values() {
        match stream {
            Stream::Screen(name) => state.screenshares.push(name.clone()),
            Stream::AudioInput(name) => state.audio_inputs.push(name.clone()),
        }
    }
    state.screenshares.sort();
    state.screenshares.dedup();
    state.audio_inputs.sort();
    state.audio_inputs.dedup();
    state
}

#[cfg(test)]
fn parse_state(json: &str) -> Option<State> {
    let objects = serde_json::from_str::<Vec<Value>>(json).ok()?;
    let mut streams = HashMap::new();
    apply_objects(&mut streams, &objects);
    Some(stream_state(&streams))
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
                {"id":1,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Video","application.name":"Firefox"}}},
                {"id":2,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","node.description":"Discord"}}},
                {"id":3,"type":"PipeWire:Interface:Node","info":{"state":"suspended","props":{"media.class":"Stream/Input/Audio","application.name":"Ignored"}}},
                {"id":4,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","stream.monitor":"true","application.name":"Monitor"}}}
            ]"#,
        )
        .unwrap();

        assert_eq!(state.screenshares, ["Firefox"]);
        assert_eq!(state.audio_inputs, ["Discord"]);
    }

    #[test]
    fn applies_monitor_updates_and_removals() {
        let mut streams = HashMap::new();
        let active = serde_json::from_str::<Vec<Value>>(
            r#"[{"id":7,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","application.name":"Recorder"}}}]"#,
        )
        .unwrap();
        apply_objects(&mut streams, &active);
        assert_eq!(stream_state(&streams).audio_inputs, ["Recorder"]);

        let removed = serde_json::from_str::<Vec<Value>>(r#"[{"id":7,"info":null}]"#).unwrap();
        apply_objects(&mut streams, &removed);
        assert_eq!(stream_state(&streams), State::default());
    }

    #[test]
    fn uses_the_best_available_stream_name() {
        let props: Value =
            serde_json::from_str(r#"{"application.name":"Browser","node.description":"Ignored"}"#)
                .unwrap();

        assert_eq!(stream_name(&props), "Browser");
    }

    #[test]
    fn rejects_invalid_pipewire_state() {
        assert_eq!(parse_state("not json"), None);
    }
}
