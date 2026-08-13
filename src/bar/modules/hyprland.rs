use std::{
    env,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use gtk::prelude::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::background;

const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const TITLE_UPDATE_INTERVAL: Duration = Duration::from_millis(50);
const SUBMAP_PREFIX: &str = "󰌌 ";

pub fn widget() -> gtk::Box {
    let root = gtk::Box::builder()
        .spacing(crate::bar::MODULE_GAP)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();

    let workspaces = gtk::Box::builder()
        .spacing(0)
        .valign(gtk::Align::Center)
        .build();
    workspaces.add_css_class("workspaces");

    let window = gtk::Label::new(None);
    window.add_css_class("window");
    window.set_ellipsize(gtk::pango::EllipsizeMode::End);
    window.set_hexpand(true);
    window.set_margin_end(crate::bar::MODULE_GAP);
    window.set_valign(gtk::Align::Center);
    window.set_max_width_chars(1);
    window.set_single_line_mode(true);
    window.set_xalign(0.0);

    root.append(&workspaces);
    root.append(&window);

    let (updates_tx, updates_rx) = async_channel::unbounded();
    let (commands_tx, commands_rx) = mpsc::channel();
    background::spawn("hyprland-events", move || {
        run_worker(updates_tx, commands_rx)
    });
    let mut state = State::default();
    background::listen(updates_rx, move |update| match update {
        Update::State(next) => {
            render(&workspaces, &window, &state, &next, &commands_tx);
            state = next;
        }
        Update::ActiveWorkspace(id) => {
            state.active_id = Some(id);
            update_workspace_classes(&workspaces, &state);
        }
        Update::Title(title) => {
            state.title = title;
            if state.submap.is_none() {
                window.set_label(&state.title);
            }
        }
        Update::Submap(submap) => {
            state.submap = submap;
            update_window_title(&window, &state);
        }
    });

    root
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    workspaces: Vec<Workspace>,
    active_id: Option<i64>,
    title: String,
    submap: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Workspace {
    id: i64,
    name: String,
    urgent: bool,
}

enum Update {
    State(State),
    ActiveWorkspace(i64),
    Title(String),
    Submap(Option<String>),
}

#[derive(Default)]
struct TitleUpdates {
    pending: Option<String>,
    last_sent: Option<Instant>,
}

impl TitleUpdates {
    fn queue(&mut self, title: String) {
        self.pending = Some(title);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn take_ready(&mut self, now: Instant) -> Option<String> {
        if self
            .last_sent
            .is_some_and(|last_sent| now.duration_since(last_sent) < TITLE_UPDATE_INTERVAL)
        {
            return None;
        }

        let title = self.pending.take()?;
        self.last_sent = Some(now);
        Some(title)
    }
}

#[derive(Deserialize)]
struct WorkspaceInfo {
    id: i64,
    name: String,
    monitor: String,
    #[serde(default)]
    urgent: bool,
}

#[derive(Deserialize)]
struct ActiveWorkspace {
    id: i64,
    monitor: String,
}

#[derive(Default, Deserialize)]
struct ActiveWindow {
    #[serde(default)]
    address: String,
    #[serde(default)]
    title: String,
}

enum Command {
    Activate(WorkspaceSelector),
}

#[derive(Debug, Eq, PartialEq)]
enum Event {
    Refresh,
    Workspace(i64),
    ActiveWindow(Option<String>),
    Title { address: String, title: String },
    Submap(Option<String>),
    Ignore,
}

#[derive(Clone)]
enum WorkspaceSelector {
    Id(i64),
    Name(String),
}

fn render(
    workspaces: &gtk::Box,
    window: &gtk::Label,
    current: &State,
    next: &State,
    commands: &Sender<Command>,
) {
    if workspace_structure_changed(current, next) {
        rebuild_workspaces(workspaces, next, commands);
    } else if current.active_id != next.active_id || current.workspaces != next.workspaces {
        update_workspace_classes(workspaces, next);
    }

    if current.submap != next.submap || next.submap.is_none() && current.title != next.title {
        update_window_title(window, next);
    }
}

fn update_window_title(window: &gtk::Label, state: &State) {
    if let Some(submap) = &state.submap {
        window.set_label(&format!("{SUBMAP_PREFIX}{submap}"));
        window.add_css_class("submap");
    } else {
        window.set_label(&state.title);
        window.remove_css_class("submap");
    }
}

fn rebuild_workspaces(workspaces: &gtk::Box, state: &State, commands: &Sender<Command>) {
    while let Some(child) = workspaces.first_child() {
        workspaces.remove(&child);
    }

    for workspace in &state.workspaces {
        let button = gtk::Button::with_label(&workspace.name);
        button.set_cursor_from_name(Some("pointer"));
        if state.active_id == Some(workspace.id) {
            button.add_css_class("active");
        }
        if workspace.urgent {
            button.add_css_class("urgent");
        }

        let commands = commands.clone();
        let selector = if workspace.id > 0 {
            WorkspaceSelector::Id(workspace.id)
        } else {
            WorkspaceSelector::Name(workspace.name.clone())
        };
        button.connect_clicked(move |_| {
            let _ = commands.send(Command::Activate(selector.clone()));
        });
        workspaces.append(&button);
    }
}

fn update_workspace_classes(workspaces: &gtk::Box, state: &State) {
    let mut child = workspaces.first_child();
    for workspace in &state.workspaces {
        let Some(widget) = child else {
            return;
        };
        child = widget.next_sibling();
        let Ok(button) = widget.downcast::<gtk::Button>() else {
            return;
        };

        if state.active_id == Some(workspace.id) {
            button.add_css_class("active");
        } else {
            button.remove_css_class("active");
        }
        if workspace.urgent {
            button.add_css_class("urgent");
        } else {
            button.remove_css_class("urgent");
        }
    }
}

fn workspace_structure_changed(current: &State, next: &State) -> bool {
    current.workspaces.len() != next.workspaces.len()
        || current
            .workspaces
            .iter()
            .zip(&next.workspaces)
            .any(|(current, next)| current.id != next.id || current.name != next.name)
}

fn run_worker(updates: async_channel::Sender<Update>, commands: Receiver<Command>) {
    loop {
        let mut title_updates = TitleUpdates::default();
        let mut active_address = refresh(&updates).unwrap_or_default();

        let Ok((request_socket, event_socket)) = socket_paths() else {
            std::thread::sleep(background::RETRY_DELAY);
            continue;
        };

        let Ok(stream) = UnixStream::connect(event_socket) else {
            std::thread::sleep(background::RETRY_DELAY);
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        let mut events = BufReader::new(stream);

        // The read timeout can cut a line in half, so the partial event is kept
        // across reads instead of being handled as an event of its own.
        let mut line = Vec::new();
        loop {
            send_ready_title(&updates, &mut title_updates);

            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::Activate(selector) => {
                        let _ = request(&request_socket, &activation_command(&selector));
                    }
                }
            }

            match events.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line.ends_with(b"\n") {
                        let event = parse_event(&String::from_utf8_lossy(&line));
                        if let Event::Workspace(id) = &event {
                            let _ = updates.send_blocking(Update::ActiveWorkspace(*id));
                        } else if event_needs_refresh(&event, active_address.as_deref()) {
                            title_updates.clear();
                            if let Ok(address) = refresh(&updates) {
                                active_address = address;
                            }
                        } else if let Event::Title {
                            ref address,
                            ref title,
                        } = event
                            && is_active_address(address, active_address.as_deref())
                        {
                            title_updates.queue(title.clone());
                            send_ready_title(&updates, &mut title_updates);
                        } else if let Event::Submap(submap) = event {
                            let _ = updates.send_blocking(Update::Submap(submap));
                        }
                        line.clear();
                    }
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }

        std::thread::sleep(background::RETRY_DELAY);
    }
}

fn send_ready_title(updates: &async_channel::Sender<Update>, titles: &mut TitleUpdates) {
    if let Some(title) = titles.take_ready(Instant::now()) {
        let _ = updates.send_blocking(Update::Title(title));
    }
}

fn refresh(updates: &async_channel::Sender<Update>) -> io::Result<Option<String>> {
    let (request_socket, _) = socket_paths()?;
    let mut workspaces: Vec<WorkspaceInfo> = request_json(&request_socket, "j/workspaces")?;
    let active_workspace: ActiveWorkspace = request_json(&request_socket, "j/activeworkspace")?;
    let active_window: ActiveWindow = request_json(&request_socket, "j/activewindow")?;
    let submap = request(&request_socket, "repl ':' .. hl.get_current_submap()")?;
    let submap = parse_submap_query(&submap)?;
    for workspace in workspaces.iter_mut().filter(|workspace| {
        workspace.monitor == active_workspace.monitor && !workspace.name.starts_with("special:")
    }) {
        workspace.urgent = request(
            &request_socket,
            &format!("repl hl.get_workspace({}).has_urgent", workspace.id),
        )?
        .trim()
            == "true";
    }
    let active_address = normalize_address(&active_window.address);
    let state = state_from_parts(workspaces, active_workspace, active_window, submap);

    let _ = updates.send_blocking(Update::State(state));
    Ok(active_address)
}

fn socket_paths() -> io::Result<(PathBuf, PathBuf)> {
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    let instance = env::var_os("HYPRLAND_INSTANCE_SIGNATURE").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HYPRLAND_INSTANCE_SIGNATURE is not set",
        )
    })?;
    let directory = PathBuf::from(runtime_dir).join("hypr").join(instance);

    Ok((
        directory.join(".socket.sock"),
        directory.join(".socket2.sock"),
    ))
}

fn request_json<T: DeserializeOwned>(socket: &Path, command: &str) -> io::Result<T> {
    let response = request(socket, command)?;
    serde_json::from_str(&response).map_err(io::Error::other)
}

fn request(socket: &Path, command: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(IPC_TIMEOUT))?;
    stream.set_write_timeout(Some(IPC_TIMEOUT))?;
    stream.write_all(command.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn activation_command(selector: &WorkspaceSelector) -> String {
    match selector {
        WorkspaceSelector::Id(id) => {
            format!("dispatch hl.dsp.focus({{ workspace = {id} }})")
        }
        WorkspaceSelector::Name(name) => format!(
            "dispatch hl.dsp.focus({{ workspace = 'name:{}' }})",
            name.replace('\\', "\\\\").replace('\'', "\\'")
        ),
    }
}

fn state_from_parts(
    workspaces: Vec<WorkspaceInfo>,
    active_workspace: ActiveWorkspace,
    active_window: ActiveWindow,
    submap: Option<String>,
) -> State {
    let mut workspaces: Vec<_> = workspaces
        .into_iter()
        .filter(|workspace| {
            workspace.monitor == active_workspace.monitor && !workspace.name.starts_with("special:")
        })
        .map(|workspace| Workspace {
            id: workspace.id,
            name: workspace.name,
            urgent: workspace.urgent,
        })
        .collect();
    workspaces.sort_by(|left, right| match (left.id > 0, right.id > 0) {
        (true, true) => left.id.cmp(&right.id),
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => left.name.cmp(&right.name),
    });

    State {
        workspaces,
        active_id: Some(active_workspace.id),
        title: active_window.title,
        submap,
    }
}

fn parse_submap_query(response: &str) -> io::Result<Option<String>> {
    let response = response.trim_end_matches(['\r', '\n']);
    let submap = response.strip_prefix(':').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Hyprland returned an invalid submap response",
        )
    })?;
    Ok(normalize_submap(submap))
}

fn normalize_submap(submap: &str) -> Option<String> {
    (!submap.is_empty() && submap != "reset").then(|| submap.to_owned())
}

fn parse_event(line: &str) -> Event {
    let Some((event, data)) = line.trim_end_matches(['\r', '\n']).split_once(">>") else {
        return Event::Ignore;
    };

    match event {
        "workspacev2" => data
            .split_once(',')
            .and_then(|(id, _)| id.parse().ok())
            .map(Event::Workspace)
            .unwrap_or(Event::Ignore),
        "activewindowv2" => Event::ActiveWindow(normalize_address(data)),
        "windowtitlev2" => {
            let Some((address, title)) = data.split_once(',') else {
                return Event::Ignore;
            };
            let Some(address) = normalize_address(address) else {
                return Event::Ignore;
            };
            Event::Title {
                address,
                title: title.into(),
            }
        }
        "submap" => Event::Submap(normalize_submap(data)),
        "focusedmonv2" | "createworkspacev2" | "destroyworkspacev2" | "moveworkspacev2"
        | "renameworkspace" | "closewindow" | "movewindowv2" | "urgent" => Event::Refresh,
        _ => Event::Ignore,
    }
}

fn normalize_address(address: &str) -> Option<String> {
    let address = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
        .unwrap_or(address)
        .trim();
    (!address.is_empty()).then(|| address.to_ascii_lowercase())
}

fn event_needs_refresh(event: &Event, active_address: Option<&str>) -> bool {
    match event {
        Event::Refresh => true,
        Event::ActiveWindow(address) => address.as_deref() != active_address,
        Event::Workspace(_) | Event::Title { .. } | Event::Submap(_) | Event::Ignore => false,
    }
}

fn is_active_address(address: &str, active_address: Option<&str>) -> bool {
    active_address == Some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_regular_workspaces_on_the_active_output() {
        let state = state_from_parts(
            serde_json::from_str(
                r#"[
                    {"id": 3, "name": "3", "monitor": "DP-1", "urgent": true},
                    {"id": -1337, "name": "development", "monitor": "DP-1"},
                    {"id": -99, "name": "special:scratchpad", "monitor": "DP-1"},
                    {"id": 1, "name": "1", "monitor": "DP-1"},
                    {"id": 2, "name": "2", "monitor": "HDMI-A-1"}
                ]"#,
            )
            .unwrap(),
            serde_json::from_str(r#"{"id": 1, "monitor": "DP-1"}"#).unwrap(),
            serde_json::from_str(r#"{"title": "Terminal"}"#).unwrap(),
            None,
        );

        assert_eq!(state.active_id, Some(1));
        assert_eq!(state.title, "Terminal");
        assert_eq!(state.submap, None);
        assert_eq!(
            state.workspaces,
            vec![
                Workspace {
                    id: 1,
                    name: "1".into(),
                    urgent: false,
                },
                Workspace {
                    id: 3,
                    name: "3".into(),
                    urgent: true,
                },
                Workspace {
                    id: -1337,
                    name: "development".into(),
                    urgent: false,
                },
            ]
        );
    }

    #[test]
    fn parses_events_that_change_the_module() {
        assert_eq!(
            parse_event("activewindowv2>>0x123"),
            Event::ActiveWindow(Some("123".into()))
        );
        assert_eq!(parse_event("activewindowv2>>"), Event::ActiveWindow(None));
        assert_eq!(parse_event("activewindow>>class,title"), Event::Ignore);
        assert_eq!(parse_event("workspacev2>>2,2"), Event::Workspace(2));
        assert_eq!(parse_event("workspacev2>>invalid,2"), Event::Ignore);
        assert_eq!(parse_event("workspace>>2"), Event::Ignore);
        assert_eq!(parse_event("focusedmonv2>>DP-1,2"), Event::Refresh);
        assert_eq!(parse_event("focusedmon>>DP-1,2"), Event::Ignore);
        assert_eq!(parse_event("urgent>>0x123"), Event::Refresh);
        assert_eq!(
            parse_event("submap>>Resize windows"),
            Event::Submap(Some("Resize windows".into()))
        );
        assert_eq!(parse_event("submap>>"), Event::Submap(None));
        assert_eq!(parse_event("submap>>reset"), Event::Submap(None));
        assert_eq!(parse_event("configreloaded>>"), Event::Ignore);
        assert_eq!(parse_event("windowtitle>>0x123"), Event::Ignore);
    }

    #[test]
    fn parses_title_events_without_losing_commas() {
        assert_eq!(
            parse_event("windowtitlev2>>0xAbC,tmux:agent:codex (work, active)\n"),
            Event::Title {
                address: "abc".into(),
                title: "tmux:agent:codex (work, active)".into(),
            }
        );
        assert_eq!(parse_event("windowtitlev2>>0x123"), Event::Ignore);
    }

    #[test]
    fn normalizes_hyprland_window_addresses() {
        assert_eq!(normalize_address("0xAbC"), Some("abc".into()));
        assert_eq!(normalize_address("ABC"), Some("abc".into()));
        assert_eq!(normalize_address("0x"), None);
    }

    #[test]
    fn parses_the_current_submap_query() {
        assert_eq!(parse_submap_query(":\n").unwrap(), None);
        assert_eq!(parse_submap_query(":reset").unwrap(), None);
        assert_eq!(
            parse_submap_query(":Resize windows\n").unwrap(),
            Some("Resize windows".into())
        );
        assert!(parse_submap_query("Resize windows").is_err());
    }

    #[test]
    fn title_events_only_match_the_active_window() {
        assert!(is_active_address("abc", Some("abc")));
        assert!(!is_active_address("abc", Some("def")));
        assert!(!is_active_address("abc", None));
    }

    #[test]
    fn repeated_active_window_events_do_not_refresh() {
        assert!(!event_needs_refresh(
            &Event::ActiveWindow(Some("abc".into())),
            Some("abc")
        ));
        assert!(event_needs_refresh(
            &Event::ActiveWindow(Some("def".into())),
            Some("abc")
        ));
        assert!(event_needs_refresh(&Event::ActiveWindow(None), Some("abc")));
    }

    #[test]
    fn coalesces_title_updates_until_the_rate_limit_expires() {
        let start = Instant::now();
        let mut titles = TitleUpdates::default();

        titles.queue("first".into());
        assert_eq!(titles.take_ready(start), Some("first".into()));

        titles.queue("second".into());
        assert_eq!(titles.take_ready(start + TITLE_UPDATE_INTERVAL / 2), None);
        titles.queue("latest".into());
        assert_eq!(
            titles.take_ready(start + TITLE_UPDATE_INTERVAL),
            Some("latest".into())
        );
    }

    #[test]
    fn clearing_title_updates_resets_the_rate_limit() {
        let start = Instant::now();
        let mut titles = TitleUpdates::default();

        titles.queue("old window".into());
        assert_eq!(titles.take_ready(start), Some("old window".into()));
        titles.queue("stale".into());
        titles.clear();
        titles.queue("new window".into());

        assert_eq!(titles.take_ready(start), Some("new window".into()));
    }

    #[test]
    fn workspace_state_only_rebuilds_for_identity_or_name_changes() {
        let state = State {
            workspaces: vec![Workspace {
                id: 1,
                name: "1".into(),
                urgent: false,
            }],
            active_id: Some(1),
            title: "one".into(),
            submap: None,
        };
        let mut changed = state.clone();
        changed.active_id = Some(2);
        changed.workspaces[0].urgent = true;
        changed.title = "two".into();
        assert!(!workspace_structure_changed(&state, &changed));

        changed.workspaces[0].name = "renamed".into();
        assert!(workspace_structure_changed(&state, &changed));
    }

    #[test]
    fn builds_current_hyprland_workspace_dispatch() {
        assert_eq!(
            activation_command(&WorkspaceSelector::Id(2)),
            "dispatch hl.dsp.focus({ workspace = 2 })"
        );
        assert_eq!(
            activation_command(&WorkspaceSelector::Name("developer's \\ workspace".into())),
            "dispatch hl.dsp.focus({ workspace = 'name:developer\\'s \\\\ workspace' })"
        );
    }
}
