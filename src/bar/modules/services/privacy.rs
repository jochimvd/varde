use std::{
    cell::Cell,
    collections::HashMap,
    ffi::CString,
    fs,
    io::BufReader,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::Path,
    rc::Rc,
    thread,
    time::Duration,
};

use gtk::prelude::*;
use serde_json::Value;

use crate::background;

const ICON_SIZE: i32 = 14;
const ICON_GAP: i32 = 7;
const COLLAPSED_HOVER_SIZE: f32 = 12.0;
const NODE_TYPE: &str = "PipeWire:Interface:Node";
const SCREEN_SHARE_PREFIX: &str = "xdph-streaming-";
const DEVICE_DIRECTORY: &str = "/dev";
const DIRECTORY_EVENTS: u32 = libc::IN_CREATE | libc::IN_DELETE | libc::IN_ATTRIB;
const DEVICE_EVENTS: u32 = libc::IN_OPEN | libc::IN_CLOSE_WRITE | libc::IN_CLOSE_NOWRITE;
const SETTLE_DELAY: Duration = Duration::from_millis(100);

pub fn widget() -> gtk::Box {
    let privacy = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    privacy.set_widget_name("privacy");
    privacy.add_css_class("module");
    privacy.add_css_class("privacy");
    privacy.add_css_class("collapsed");

    let pill = gtk::Box::builder().valign(gtk::Align::Center).build();
    pill.add_css_class("privacy-pill");

    let icons = gtk::Box::new(gtk::Orientation::Horizontal, ICON_GAP);
    icons.add_css_class("privacy-icons");
    let revealer = gtk::Revealer::builder()
        .transition_duration(300)
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .child(&icons)
        .visible(false)
        .build();
    revealer.connect_child_revealed_notify(|revealer| {
        if !revealer.is_child_revealed() {
            revealer.set_visible(false);
        }
    });
    pill.append(&revealer);
    privacy.append(&pill);

    let hover_intent = crate::bar::HoverIntent::default();
    let pointer_inside = Rc::new(Cell::new(false));
    let hover = gtk::EventControllerMotion::new();
    let update_hover = Rc::new({
        let privacy = privacy.clone();
        let pill = pill.clone();
        let revealer = revealer.clone();
        let hover_intent = hover_intent.clone();
        let pointer_inside = pointer_inside.clone();
        move |x: f64, y: f64| {
            let inside = !privacy.has_css_class("collapsed")
                || pill.compute_bounds(&privacy).is_some_and(|bounds| {
                    let half = COLLAPSED_HOVER_SIZE / 2.0;
                    let center_x = bounds.x() + bounds.width() / 2.0;
                    let center_y = bounds.y() + bounds.height() / 2.0;
                    (x as f32 - center_x).abs() <= half && (y as f32 - center_y).abs() <= half
                });
            if inside == pointer_inside.replace(inside) {
                return;
            }
            if inside {
                let privacy = privacy.clone();
                let revealer = revealer.clone();
                hover_intent.enter(move || set_expanded(&privacy, &revealer, true));
            } else {
                hover_intent.leave();
                let privacy = privacy.clone();
                let revealer = revealer.clone();
                hover_intent.retract(move || set_expanded(&privacy, &revealer, false));
            }
        }
    });
    hover.connect_enter({
        let update_hover = update_hover.clone();
        move |_, x, y| update_hover(x, y)
    });
    hover.connect_motion(move |_, x, y| update_hover(x, y));
    hover.connect_leave({
        let privacy = privacy.clone();
        let revealer = revealer.clone();
        let hover_intent = hover_intent.clone();
        let pointer_inside = pointer_inside.clone();
        move |_| {
            pointer_inside.set(false);
            hover_intent.leave();
            let privacy = privacy.clone();
            let revealer = revealer.clone();
            hover_intent.retract(move || {
                set_expanded(&privacy, &revealer, false);
            });
        }
    });
    privacy.add_controller(hover);

    let (sender, receiver) = async_channel::unbounded();
    background::listen(receiver, {
        let privacy = privacy.clone();
        let icons = icons.clone();
        let revealer = revealer.clone();
        let mut pipewire = State::default();
        let mut direct_cameras = Vec::new();
        let mut current = State::default();
        move |change| {
            match change {
                Change::PipeWire(state) => pipewire = state,
                Change::DirectCameras(cameras) => direct_cameras = cameras,
            }
            let state = merged_state(&pipewire, &direct_cameras);
            if state != current {
                update(&privacy, &icons, &revealer, &state);
                current = state;
            }
        }
    });

    let camera_sender = sender.clone();
    background::spawn("privacy-monitor", move || {
        while monitor(&sender) {
            thread::sleep(background::RETRY_DELAY);
        }
    });
    background::spawn("camera-monitor", move || {
        monitor_direct_cameras(&camera_sender)
    });

    privacy
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    cameras: Vec<String>,
    screen_shares: Vec<String>,
    microphones: Vec<String>,
}

impl State {
    fn is_empty(&self) -> bool {
        self.cameras.is_empty() && self.screen_shares.is_empty() && self.microphones.is_empty()
    }

    fn normalize(&mut self) {
        self.cameras.sort();
        self.cameras.dedup();
        self.screen_shares.sort();
        self.screen_shares.dedup();
        self.microphones.sort();
        self.microphones.dedup();
    }
}

enum Change {
    PipeWire(State),
    DirectCameras(Vec<String>),
}

#[derive(Clone)]
enum Usage {
    Camera(String),
    ScreenShare(String),
    Microphone(String),
}

fn monitor(sender: &async_channel::Sender<Change>) -> bool {
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
    let mut usages = HashMap::new();
    let mut current = None;
    for batch in batches {
        let Ok(objects) = batch else {
            break;
        };
        apply_objects(&mut usages, &objects);
        let state = usage_state(&usages);
        if current.as_ref() != Some(&state) {
            if sender
                .send_blocking(Change::PipeWire(state.clone()))
                .is_err()
            {
                background::kill(&mut child);
                return false;
            }
            current = Some(state);
        }
    }
    background::kill(&mut child);
    true
}

fn monitor_direct_cameras(sender: &async_channel::Sender<Change>) {
    let mut devices = DeviceWatch::new();
    let mut current = Vec::new();
    loop {
        let cameras = direct_cameras();
        if cameras != current {
            if sender
                .send_blocking(Change::DirectCameras(cameras.clone()))
                .is_err()
            {
                break;
            }
            current = cameras;
        }
        if !devices.as_mut().is_some_and(DeviceWatch::wait) {
            devices = None;
            thread::sleep(background::RETRY_DELAY);
        }
    }
}

/// Wakes the camera scan only when a video device is opened, closed, added, or
/// removed, since scanning every process's descriptors is expensive.
struct DeviceWatch {
    fd: OwnedFd,
    directory: i32,
    complete: bool,
}

impl DeviceWatch {
    fn new() -> Option<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mut watch = Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            directory: -1,
            complete: false,
        };
        watch.directory = watch.add(Path::new(DEVICE_DIRECTORY), DIRECTORY_EVENTS);
        watch.complete = watch.watch_devices();
        Some(watch)
    }

    fn add(&self, path: &Path, mask: u32) -> i32 {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return -1;
        };
        unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), path.as_ptr(), mask) }
    }

    /// Watches every video device, reporting whether nothing was missed.
    fn watch_devices(&self) -> bool {
        let Ok(entries) = fs::read_dir(DEVICE_DIRECTORY) else {
            return false;
        };
        let mut complete = self.directory >= 0;
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(is_video_device) {
                complete &= self.add(&entry.path(), DEVICE_EVENTS) >= 0;
            }
        }
        complete
    }

    /// Blocks until a video device may have changed, polling instead whenever
    /// a device could not be watched. Returns false once the watch is broken.
    fn wait(&mut self) -> bool {
        let timeout = if self.complete {
            -1
        } else {
            background::RETRY_DELAY.as_millis() as i32
        };
        loop {
            match self.poll(timeout) {
                Some(true) => {}
                Some(false) => break,
                None => return false,
            }
            match self.read_events() {
                Some(true) => break,
                Some(false) => {}
                None => return false,
            }
        }
        // Opening a device tends to come with further opens and closes, so
        // they are left to settle into a single scan.
        thread::sleep(SETTLE_DELAY);
        while self.poll(0) == Some(true) {
            if self.read_events().is_none() {
                return false;
            }
        }
        self.complete = self.watch_devices();
        true
    }

    fn poll(&self, timeout: i32) -> Option<bool> {
        let mut fd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        match unsafe { libc::poll(&mut fd, 1, timeout) } {
            0 => Some(false),
            1.. => Some(true),
            _ if interrupted() => Some(false),
            _ => None,
        }
    }

    fn read_events(&self) -> Option<bool> {
        let mut buffer = [0; 4096];
        let length = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if length < 0 {
            return interrupted().then_some(false);
        }
        Some(devices_changed(&buffer[..length as usize], self.directory))
    }
}

fn interrupted() -> bool {
    std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
}

/// Reads raw inotify events, ignoring unrelated entries in the device directory.
fn devices_changed(mut events: &[u8], directory: i32) -> bool {
    const HEADER: usize = std::mem::size_of::<libc::inotify_event>();
    let mut changed = false;
    while events.len() >= HEADER {
        let field = |offset: usize| events[offset..offset + 4].try_into().unwrap();
        let watch = i32::from_ne_bytes(field(0));
        let length = u32::from_ne_bytes(field(12)) as usize;
        let Some(name) = events.get(HEADER..HEADER + length) else {
            break;
        };
        let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
        changed |= watch != directory || std::str::from_utf8(name).is_ok_and(is_video_device);
        events = &events[HEADER + length..];
    }
    changed
}

fn direct_cameras() -> Vec<String> {
    let mut cameras = Vec::new();
    let Ok(processes) = fs::read_dir("/proc") else {
        return cameras;
    };
    for process in processes.flatten() {
        let Ok(descriptors) = fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            let Ok(target) = fs::read_link(descriptor.path()) else {
                continue;
            };
            let Some(device) = video_device(&target) else {
                continue;
            };
            let name = fs::read_to_string(
                Path::new("/sys/class/video4linux")
                    .join(device)
                    .join("name"),
            )
            .map(|name| name.trim().to_string())
            .unwrap_or_else(|_| device.to_string());
            cameras.push(name);
        }
    }
    cameras.sort();
    cameras.dedup();
    cameras
}

fn video_device(path: &Path) -> Option<&str> {
    let device = path.strip_prefix(DEVICE_DIRECTORY).ok()?.to_str()?;
    is_video_device(device).then_some(device)
}

fn is_video_device(name: &str) -> bool {
    name.strip_prefix("video").is_some_and(|number| {
        !number.is_empty() && number.chars().all(|character| character.is_ascii_digit())
    })
}

fn apply_objects(usages: &mut HashMap<u64, Usage>, objects: &[Value]) {
    for object in objects {
        let Some(id) = object.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if let Some(usage) = privacy_usage(object) {
            usages.insert(id, usage);
        } else {
            usages.remove(&id);
        }
    }
}

fn privacy_usage(object: &Value) -> Option<Usage> {
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
    match props.get("media.class").and_then(Value::as_str) {
        Some("Video/Source")
            if props.get("media.role").and_then(Value::as_str) == Some("Camera") =>
        {
            Some(Usage::Camera(display_name(props)))
        }
        Some("Video/Source")
            if props
                .get("media.name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with(SCREEN_SHARE_PREFIX)) =>
        {
            Some(Usage::ScreenShare("Hyprland".into()))
        }
        Some("Stream/Input/Audio") => Some(Usage::Microphone(display_name(props))),
        _ => None,
    }
}

fn usage_state(usages: &HashMap<u64, Usage>) -> State {
    let mut state = State::default();
    for usage in usages.values() {
        match usage {
            Usage::Camera(name) => state.cameras.push(name.clone()),
            Usage::ScreenShare(name) => state.screen_shares.push(name.clone()),
            Usage::Microphone(name) => state.microphones.push(name.clone()),
        }
    }
    state.normalize();
    state
}

fn merged_state(pipewire: &State, direct_cameras: &[String]) -> State {
    let mut state = pipewire.clone();
    state.cameras.extend_from_slice(direct_cameras);
    state.normalize();
    state
}

#[cfg(test)]
fn parse_state(json: &str) -> Option<State> {
    let objects = serde_json::from_str::<Vec<Value>>(json).ok()?;
    let mut usages = HashMap::new();
    apply_objects(&mut usages, &objects);
    Some(usage_state(&usages))
}

fn display_name(props: &Value) -> String {
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

fn update(privacy: &gtk::Box, icons: &gtk::Box, revealer: &gtk::Revealer, state: &State) {
    while let Some(child) = icons.first_child() {
        icons.remove(&child);
    }
    privacy.set_visible(!state.is_empty());
    if state.is_empty() {
        set_expanded(privacy, revealer, false);
        return;
    }

    if !state.microphones.is_empty() {
        icons.append(&item(
            "audio-input-microphone-symbolic",
            &format_tooltip("Microphone in use", &state.microphones),
        ));
    }
    if !state.cameras.is_empty() {
        icons.append(&item(
            "camera-photo-symbolic",
            &format_tooltip("Camera in use", &state.cameras),
        ));
    }
    if !state.screen_shares.is_empty() {
        icons.append(&item(
            "screen-shared-symbolic",
            &format_tooltip("Screen sharing", &state.screen_shares),
        ));
    }
}

fn set_expanded(privacy: &gtk::Box, revealer: &gtk::Revealer, expanded: bool) {
    if expanded {
        revealer.set_visible(true);
        privacy.remove_css_class("collapsed");
    } else {
        privacy.add_css_class("collapsed");
    }
    revealer.set_reveal_child(expanded);
}

fn item(icon: &str, tooltip: &str) -> gtk::Image {
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(ICON_SIZE);
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
    fn selects_running_privacy_nodes() {
        let state = parse_state(
            r#"[
                {"id":1,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","media.role":"Camera","node.description":"Webcam"}}},
                {"id":2,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","media.name":"xdph-streaming-123","node.name":"xdg-desktop-portal-hyprland"}}},
                {"id":3,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","node.description":"Discord"}}}
            ]"#,
        )
        .unwrap();

        assert_eq!(state.cameras, ["Webcam"]);
        assert_eq!(state.screen_shares, ["Hyprland"]);
        assert_eq!(state.microphones, ["Discord"]);
    }

    #[test]
    fn ignores_inactive_and_unrelated_nodes() {
        let state = parse_state(
            r#"[
                {"id":1,"type":"PipeWire:Interface:Node","info":{"state":"suspended","props":{"media.class":"Video/Source","media.role":"Camera","node.description":"Sleeping camera"}}},
                {"id":2,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","node.name":"virtual-camera"}}},
                {"id":3,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Video","application.name":"Browser"}}},
                {"id":4,"type":"PipeWire:Interface:Node","info":{"state":"idle","props":{"media.class":"Stream/Input/Audio","application.name":"Idle recorder"}}},
                {"id":5,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","stream.monitor":"true","application.name":"Monitor"}}}
            ]"#,
        )
        .unwrap();

        assert_eq!(state, State::default());
    }

    #[test]
    fn applies_monitor_updates_and_removals() {
        let mut usages = HashMap::new();
        let active = serde_json::from_str::<Vec<Value>>(
            r#"[{"id":7,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Stream/Input/Audio","application.name":"Recorder"}}}]"#,
        )
        .unwrap();
        apply_objects(&mut usages, &active);
        assert_eq!(usage_state(&usages).microphones, ["Recorder"]);

        let removed = serde_json::from_str::<Vec<Value>>(r#"[{"id":7,"info":null}]"#).unwrap();
        apply_objects(&mut usages, &removed);
        assert_eq!(usage_state(&usages), State::default());
    }

    #[test]
    fn sorts_and_deduplicates_source_names() {
        let state = parse_state(
            r#"[
                {"id":1,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","media.role":"Camera","node.description":"Webcam B"}}},
                {"id":2,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","media.role":"Camera","node.description":"Webcam A"}}},
                {"id":3,"type":"PipeWire:Interface:Node","info":{"state":"running","props":{"media.class":"Video/Source","media.role":"Camera","node.description":"Webcam A"}}}
            ]"#,
        )
        .unwrap();

        assert_eq!(state.cameras, ["Webcam A", "Webcam B"]);
    }

    #[test]
    fn merges_pipewire_and_direct_camera_sources() {
        let pipewire = State {
            cameras: vec!["Webcam B".into(), "Webcam A".into()],
            ..State::default()
        };

        let state = merged_state(&pipewire, &["Webcam A".into(), "Webcam C".into()]);

        assert_eq!(state.cameras, ["Webcam A", "Webcam B", "Webcam C"]);
    }

    #[test]
    fn recognizes_only_numbered_video_devices() {
        assert_eq!(video_device(Path::new("/dev/video0")), Some("video0"));
        assert_eq!(video_device(Path::new("/dev/video12")), Some("video12"));
        assert_eq!(video_device(Path::new("/dev/video")), None);
        assert_eq!(video_device(Path::new("/dev/video-camera")), None);
        assert_eq!(video_device(Path::new("/tmp/video0")), None);
    }

    #[test]
    fn device_events_ignore_unrelated_device_directory_entries() {
        fn event(watch: i32, name: &str) -> Vec<u8> {
            let mut name = name.as_bytes().to_vec();
            name.resize(name.len().next_multiple_of(16), 0);
            let mut event = Vec::new();
            event.extend_from_slice(&watch.to_ne_bytes());
            event.extend_from_slice(&libc::IN_CREATE.to_ne_bytes());
            event.extend_from_slice(&0u32.to_ne_bytes());
            event.extend_from_slice(&(name.len() as u32).to_ne_bytes());
            event.extend_from_slice(&name);
            event
        }

        let unrelated = [event(1, "ttyS0"), event(1, "video")].concat();
        assert!(!devices_changed(&unrelated, 1));
        assert!(devices_changed(
            &[unrelated, event(1, "video2")].concat(),
            1
        ));
        assert!(devices_changed(&event(2, ""), 1));
        assert!(devices_changed(&event(-1, ""), 1));
    }

    #[test]
    fn uses_the_best_available_display_name() {
        let props: Value =
            serde_json::from_str(r#"{"application.name":"Browser","node.description":"Ignored"}"#)
                .unwrap();

        assert_eq!(display_name(&props), "Browser");
    }

    #[test]
    fn rejects_invalid_pipewire_state() {
        assert_eq!(parse_state("not json"), None);
    }
}
