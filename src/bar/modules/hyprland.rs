use std::{
    collections::HashSet,
    env,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use gtk::prelude::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::background;

const IPC_TIMEOUT: Duration = Duration::from_secs(2);

pub fn widget() -> gtk::Box {
    let root = gtk::Box::builder()
        .spacing(0)
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
    background::listen(updates_rx, move |state| {
        render(&workspaces, &window, &state, &commands_tx);
    });

    root
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    workspaces: Vec<Workspace>,
    active_id: Option<i64>,
    title: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Workspace {
    id: i64,
    name: String,
    urgent: bool,
}

#[derive(Deserialize)]
struct WorkspaceInfo {
    id: i64,
    name: String,
    monitor: String,
}

#[derive(Deserialize)]
struct ActiveWorkspace {
    id: i64,
    monitor: String,
}

#[derive(Default, Deserialize)]
struct ActiveWindow {
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct Client {
    #[serde(default)]
    urgent: bool,
    workspace: ClientWorkspace,
}

#[derive(Deserialize)]
struct ClientWorkspace {
    id: i64,
}

enum Command {
    Activate(WorkspaceSelector),
}

#[derive(Clone)]
enum WorkspaceSelector {
    Id(i64),
    Name(String),
}

fn render(workspaces: &gtk::Box, window: &gtk::Label, state: &State, commands: &Sender<Command>) {
    while let Some(child) = workspaces.first_child() {
        workspaces.remove(&child);
    }

    for workspace in &state.workspaces {
        let button = gtk::Button::with_label(&workspace.name);
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

    window.set_label(&state.title);
}

fn run_worker(updates: async_channel::Sender<State>, commands: Receiver<Command>) {
    loop {
        refresh(&updates);

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
                        if event_needs_refresh(&String::from_utf8_lossy(&line)) {
                            refresh(&updates);
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

fn refresh(updates: &async_channel::Sender<State>) {
    let Ok((request_socket, _)) = socket_paths() else {
        return;
    };

    let state: io::Result<State> = (|| {
        let workspaces = request_json(&request_socket, "j/workspaces")?;
        let active_workspace = request_json(&request_socket, "j/activeworkspace")?;
        let active_window = request_json(&request_socket, "j/activewindow")?;
        let clients = request_json(&request_socket, "j/clients")?;
        Ok(state_from_parts(
            workspaces,
            active_workspace,
            active_window,
            clients,
        ))
    })();

    if let Ok(state) = state {
        let _ = updates.send_blocking(state);
    }
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
    clients: Vec<Client>,
) -> State {
    let urgent_ids: HashSet<_> = clients
        .into_iter()
        .filter(|client| client.urgent)
        .map(|client| client.workspace.id)
        .collect();

    let mut workspaces: Vec<_> = workspaces
        .into_iter()
        .filter(|workspace| {
            workspace.monitor == active_workspace.monitor && !workspace.name.starts_with("special:")
        })
        .map(|workspace| Workspace {
            id: workspace.id,
            name: workspace.name,
            urgent: urgent_ids.contains(&workspace.id),
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
    }
}

fn event_needs_refresh(line: &str) -> bool {
    matches!(
        line.split_once(">>").map(|(event, _)| event),
        Some(
            "workspace"
                | "workspacev2"
                | "focusedmon"
                | "focusedmonv2"
                | "createworkspace"
                | "createworkspacev2"
                | "destroyworkspace"
                | "destroyworkspacev2"
                | "moveworkspace"
                | "moveworkspacev2"
                | "renameworkspace"
                | "activewindow"
                | "activewindowv2"
                | "openwindow"
                | "closewindow"
                | "movewindow"
                | "movewindowv2"
                | "urgent"
                | "windowtitle"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_regular_workspaces_on_the_active_output() {
        let state = state_from_parts(
            serde_json::from_str(
                r#"[
                    {"id": 3, "name": "3", "monitor": "DP-1"},
                    {"id": -1337, "name": "development", "monitor": "DP-1"},
                    {"id": -99, "name": "special:scratchpad", "monitor": "DP-1"},
                    {"id": 1, "name": "1", "monitor": "DP-1"},
                    {"id": 2, "name": "2", "monitor": "HDMI-A-1"}
                ]"#,
            )
            .unwrap(),
            serde_json::from_str(r#"{"id": 1, "monitor": "DP-1"}"#).unwrap(),
            serde_json::from_str(r#"{"title": "Terminal"}"#).unwrap(),
            serde_json::from_str(r#"[{"urgent": true, "workspace": {"id": 3}}]"#).unwrap(),
        );

        assert_eq!(state.active_id, Some(1));
        assert_eq!(state.title, "Terminal");
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
    fn recognizes_events_that_change_the_module() {
        assert!(event_needs_refresh("activewindowv2>>0x123"));
        assert!(event_needs_refresh("workspacev2>>2,2"));
        assert!(event_needs_refresh("urgent>>0x123"));
        assert!(!event_needs_refresh("configreloaded>>"));
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
