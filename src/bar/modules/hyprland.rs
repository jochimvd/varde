use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    env,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{glib, prelude::*};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{
    background,
    bar::{Msg, Updates},
    title::{self, Item, Overlay},
};

const IPC_TIMEOUT: Duration = Duration::from_secs(2);
const TITLE_UPDATE_INTERVAL: Duration = Duration::from_millis(50);

/// Reports Hyprland's workspaces and focused window, and the title overlay.
pub fn start(title: &title::Manager, updates: &Updates) {
    let generation = Rc::new(Cell::new(0_u64));
    title.connect({
        let updates = updates.clone();
        move |current| show_overlay(&updates, &generation, current.cloned())
    });

    let (sender, receiver) = async_channel::unbounded();
    background::spawn("hyprland-events", move || run_worker(sender));
    updates.forward(receiver, Msg::Hyprland);
}

/// Reports `overlay`, adding its items once their delay passed.
fn show_overlay(updates: &Updates, generation: &Rc<Cell<u64>>, overlay: Option<Overlay>) {
    let current = generation.get().wrapping_add(1);
    generation.set(current);
    let Some(overlay) = overlay else {
        updates.send(Msg::Title(None));
        return;
    };
    let delayed = !overlay.items.is_empty() && overlay.items_delay_ms > 0;
    if delayed {
        let updates = updates.clone();
        let generation = generation.clone();
        let overlay = overlay.clone();
        let delay = Duration::from_millis(overlay.items_delay_ms);
        glib::timeout_add_local_once(delay, move || {
            if generation.get() == current {
                updates.send(Msg::Title(Some(TitleOverlay {
                    overlay,
                    items: true,
                })));
            }
        });
    }
    let items = !overlay.items.is_empty() && !delayed;
    updates.send(Msg::Title(Some(TitleOverlay { overlay, items })));
}

/// A title overlay, and whether its items are shown yet.
#[derive(Clone)]
pub struct TitleOverlay {
    overlay: Overlay,
    items: bool,
}

/// One bar's workspaces, and the focused window's title if that window is on
/// the bar's monitor.
pub struct Widgets {
    pub root: gtk::Box,
    workspaces: gtk::Box,
    window: Rc<WindowTitle>,
    shown: Shown,
}

impl Widgets {
    pub fn new() -> Self {
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

        let label = gtk::Label::new(None);
        label.add_css_class("window");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);
        label.set_margin_end(crate::bar::MODULE_GAP);
        label.set_valign(gtk::Align::Center);
        label.set_max_width_chars(1);
        label.set_single_line_mode(true);
        label.set_xalign(0.0);

        root.append(&workspaces);
        root.append(&label);
        let tooltip_grid: Rc<RefCell<Option<gtk::Grid>>> = Rc::default();
        let tooltip_content = tooltip_grid.clone();
        label.connect_query_tooltip(move |_, _, _, _, tooltip| {
            let content = tooltip_content.borrow();
            if let Some(grid) = content.as_ref() {
                tooltip.set_custom(Some(grid));
                true
            } else {
                false
            }
        });
        let window = Rc::new(WindowTitle {
            label,
            title: RefCell::default(),
            overlay: RefCell::default(),
            fitted: Cell::new(-1),
            tooltip_grid,
        });
        window.fit_after_paint();

        Self {
            root,
            workspaces,
            window,
            shown: Shown::default(),
        }
    }

    pub fn show(&mut self, state: &State, monitor: &str) {
        let next = state.shown(monitor);
        if workspace_structure_changed(&self.shown, &next) {
            rebuild_workspaces(&self.workspaces, &next);
        } else if self.shown != next {
            update_workspace_classes(&self.workspaces, &next);
        }
        self.shown = next;

        self.show_title(&state.title);
        self.window
            .label
            .set_visible(state.window_monitor == monitor);
    }

    pub fn show_title(&self, title: &str) {
        self.window.set_title(title);
    }

    pub fn show_overlay(&self, overlay: Option<TitleOverlay>) {
        self.window.set_overlay(overlay);
    }
}

/// Hyprland's regular workspaces on every monitor and where the focus is.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    workspaces: Vec<Workspace>,
    active: HashMap<String, i64>,
    /// The monitor of the focused window, or the focused monitor without
    /// one. Hyprland keeps a window focused when the pointer focuses an
    /// empty monitor.
    window_monitor: String,
    title: String,
}

impl State {
    pub fn apply(&mut self, update: &Update) {
        match update {
            Update::State(next) => *self = next.clone(),
            Update::ActiveWorkspace(id) => self.activate(*id),
            Update::Title(title) => self.title = title.clone(),
        }
    }

    fn activate(&mut self, id: i64) {
        if let Some(workspace) = self.workspaces.iter().find(|workspace| workspace.id == id) {
            self.active.insert(workspace.monitor.clone(), id);
        }
    }

    fn shown(&self, monitor: &str) -> Shown {
        Shown {
            workspaces: self
                .workspaces
                .iter()
                .filter(|workspace| workspace.monitor == monitor)
                .cloned()
                .collect(),
            active_id: self.active.get(monitor).copied(),
        }
    }
}

/// What one monitor's workspace buttons show.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Shown {
    workspaces: Vec<Workspace>,
    active_id: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Workspace {
    id: i64,
    name: String,
    monitor: String,
    urgent: bool,
}

pub enum Update {
    State(State),
    ActiveWorkspace(i64),
    Title(String),
}

struct WindowTitle {
    label: gtk::Label,
    title: RefCell<String>,
    overlay: RefCell<Option<TitleOverlay>>,
    /// The label width the overlay items were last fitted to.
    fitted: Cell<i32>,
    tooltip_grid: Rc<RefCell<Option<gtk::Grid>>>,
}

impl WindowTitle {
    fn set_title(&self, title: &str) {
        if *self.title.borrow() == title {
            return;
        }
        self.title.replace(title.to_owned());
        if self.overlay.borrow().is_none() {
            self.label.set_text(title);
        }
    }

    fn set_overlay(&self, overlay: Option<TitleOverlay>) {
        self.overlay.replace(overlay);
        self.fitted.set(-1);
        self.label.set_has_tooltip(false);
        self.tooltip_grid.borrow_mut().take();
        let overlay = self.overlay.borrow();
        let Some(TitleOverlay { overlay, items }) = overlay.as_ref() else {
            self.label.set_text(&self.title.borrow());
            self.label.remove_css_class("overlay");
            return;
        };
        self.label.add_css_class("overlay");
        if !overlay.items.is_empty() {
            *self.tooltip_grid.borrow_mut() = Some(overlay_tooltip(overlay));
            self.label.set_has_tooltip(true);
        }
        if *items {
            self.label
                .set_markup(&overlay_markup(overlay, &overlay.items));
        } else {
            self.label.set_text(&heading(overlay));
        }
    }

    /// Fits shown overlay items to the label whenever its bar is painted at a
    /// new width, so each monitor fits as many as its bar has room for.
    fn fit_after_paint(self: &Rc<Self>) {
        let handler = Rc::new(RefCell::new(None));
        self.label.connect_map({
            let window = Rc::downgrade(self);
            let handler = handler.clone();
            move |label| {
                let Some(clock) = label.frame_clock() else {
                    return;
                };
                let window = window.clone();
                let id = clock.connect_after_paint(move |_| {
                    if let Some(window) = window.upgrade() {
                        window.fit();
                    }
                });
                handler.replace(Some((clock, id)));
            }
        });
        self.label.connect_unmap(move |_| {
            if let Some((clock, id)) = handler.take() {
                clock.disconnect(id);
            }
        });
    }

    fn fit(&self) {
        let overlay = self.overlay.borrow();
        let Some(TitleOverlay {
            overlay,
            items: true,
        }) = overlay.as_ref()
        else {
            return;
        };
        let available = self.label.layout().width() / gtk::pango::SCALE;
        if available <= 0 || available == self.fitted.replace(available) {
            return;
        }
        let layout = self.label.create_pango_layout(None);
        let markup = fit_overlay_markup(overlay, |markup| {
            layout.set_markup(markup);
            layout.pixel_size().0 <= available
        });
        self.label.set_markup(&markup);
    }
}

fn heading(overlay: &Overlay) -> String {
    if overlay.icon.is_empty() {
        overlay.text.clone()
    } else {
        format!("{} {}", overlay.icon, overlay.text)
    }
}

fn overlay_tooltip(overlay: &Overlay) -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .column_spacing(18)
        .row_spacing(4)
        .build();
    let heading = gtk::Label::new(None);
    heading.set_xalign(0.0);
    heading.set_markup(&format!(
        "<b>{}</b>",
        glib::markup_escape_text(&overlay.text)
    ));
    grid.attach(&heading, 0, 0, 2, 1);
    for (row, item) in overlay.items.iter().enumerate() {
        let key = gtk::Label::new(Some(&item.key));
        key.set_xalign(0.0);
        let description = gtk::Label::new(Some(&item.label));
        description.set_xalign(0.0);
        grid.attach(&key, 0, row as i32 + 1, 1, 1);
        grid.attach(&description, 1, row as i32 + 1, 1, 1);
    }
    grid
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

    /// How long a pending title still has to wait, or `None` when none is.
    fn delay(&self, now: Instant) -> Option<Duration> {
        self.pending.as_ref()?;
        let ready = self.last_sent? + TITLE_UPDATE_INTERVAL;
        Some(
            ready
                .saturating_duration_since(now)
                .max(Duration::from_millis(1)),
        )
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
struct MonitorInfo {
    id: i64,
    name: String,
    #[serde(rename = "activeWorkspace")]
    active_workspace: WorkspaceRef,
    focused: bool,
}

#[derive(Deserialize)]
struct WorkspaceRef {
    id: i64,
}

#[derive(Default, Deserialize)]
struct ActiveWindow {
    #[serde(default)]
    address: String,
    #[serde(default)]
    title: String,
    monitor: Option<i64>,
}

#[derive(Debug, Eq, PartialEq)]
enum Event {
    Refresh,
    Workspace(i64),
    ActiveWindow(Option<String>),
    Title { address: String, title: String },
    Ignore,
}

enum WorkspaceSelector {
    Id(i64),
    Name(String),
}

fn overlay_markup(overlay: &Overlay, items: &[Item]) -> String {
    let mut parts = vec![glib::markup_escape_text(&heading(overlay)).to_string()];
    parts.extend(items.iter().map(|item| {
        let key = glib::markup_escape_text(&item.key);
        if item.label.is_empty() {
            format!("<b>{key}</b>")
        } else {
            format!("<b>{key}</b> {}", glib::markup_escape_text(&item.label))
        }
    }));
    parts.join(" · ")
}

fn fit_overlay_markup(overlay: &Overlay, fits: impl Fn(&str) -> bool) -> String {
    let (pinned, actions): (Vec<_>, Vec<_>) =
        overlay.items.iter().cloned().partition(|item| item.pinned);
    for count in (0..=actions.len()).rev() {
        let mut shown = actions[..count].to_vec();
        if count < actions.len() {
            shown.push(Item {
                key: format!("+{}", actions.len() - count),
                label: "more".into(),
                pinned: false,
            });
        }
        shown.extend(pinned.iter().cloned());
        let markup = overlay_markup(overlay, &shown);
        if fits(&markup) || count == 0 {
            return markup;
        }
    }
    unreachable!()
}

fn rebuild_workspaces(workspaces: &gtk::Box, state: &Shown) {
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

        let command = activation_command(&if workspace.id > 0 {
            WorkspaceSelector::Id(workspace.id)
        } else {
            WorkspaceSelector::Name(workspace.name.clone())
        });
        button.connect_clicked(move |_| {
            let command = command.clone();
            background::spawn("workspace-activate", move || {
                if let Ok((request_socket, _)) = socket_paths() {
                    let _ = request(&request_socket, &command);
                }
            });
        });
        workspaces.append(&button);
    }
}

fn update_workspace_classes(workspaces: &gtk::Box, state: &Shown) {
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

fn workspace_structure_changed(current: &Shown, next: &Shown) -> bool {
    current.workspaces.len() != next.workspaces.len()
        || current
            .workspaces
            .iter()
            .zip(&next.workspaces)
            .any(|(current, next)| current.id != next.id || current.name != next.name)
}

fn run_worker(updates: async_channel::Sender<Update>) {
    loop {
        let mut title_updates = TitleUpdates::default();
        let mut active_address = refresh(&updates).unwrap_or_default();

        let Ok((_, event_socket)) = socket_paths() else {
            std::thread::sleep(background::RETRY_DELAY);
            continue;
        };

        let Ok(stream) = UnixStream::connect(event_socket) else {
            std::thread::sleep(background::RETRY_DELAY);
            continue;
        };
        let mut events = BufReader::new(stream);

        // A pending title's read timeout can cut a line in half, so the partial
        // event is kept across reads instead of being handled as an event of its own.
        let mut line = Vec::new();
        loop {
            send_ready_title(&updates, &mut title_updates);
            let delay = title_updates.delay(Instant::now());
            if events.get_ref().set_read_timeout(delay).is_err() {
                break;
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
    let monitors: Vec<MonitorInfo> = request_json(&request_socket, "j/monitors")?;
    let active_window: ActiveWindow = request_json(&request_socket, "j/activewindow")?;
    let mut shown: Vec<_> = workspaces
        .iter_mut()
        .filter(|workspace| is_shown(workspace))
        .collect();
    if !shown.is_empty() {
        let ids = shown.iter().map(|workspace| workspace.id);
        let urgent = request(&request_socket, &urgency_query(ids))?;
        for (workspace, urgent) in shown.iter_mut().zip(parse_urgency(&urgent)) {
            workspace.urgent = urgent;
        }
    }
    let active_address = normalize_address(&active_window.address);
    let state = state_from_parts(workspaces, monitors, active_window);

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

fn is_shown(workspace: &WorkspaceInfo) -> bool {
    !workspace.name.starts_with("special:")
}

fn urgency_query(ids: impl Iterator<Item = i64>) -> String {
    let ids = ids.map(|id| id.to_string()).collect::<Vec<_>>().join(", ");
    format!(
        "repl (function() local urgent = {{}} for _, id in ipairs({{ {ids} }}) do \
         local workspace = hl.get_workspace(id) \
         urgent[#urgent + 1] = tostring(workspace ~= nil and workspace.has_urgent == true) \
         end return table.concat(urgent, ',') end)()"
    )
}

/// Reads the urgency of each queried workspace in order; anything Hyprland
/// could not answer counts as not urgent.
fn parse_urgency(response: &str) -> impl Iterator<Item = bool> + '_ {
    response
        .trim()
        .split(',')
        .map(|urgent| urgent == "true")
        .chain(std::iter::repeat(false))
}

fn state_from_parts(
    workspaces: Vec<WorkspaceInfo>,
    monitors: Vec<MonitorInfo>,
    active_window: ActiveWindow,
) -> State {
    let mut workspaces: Vec<_> = workspaces
        .into_iter()
        .filter(is_shown)
        .map(|workspace| Workspace {
            id: workspace.id,
            name: workspace.name,
            monitor: workspace.monitor,
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
        active: monitors
            .iter()
            .map(|monitor| (monitor.name.clone(), monitor.active_workspace.id))
            .collect(),
        window_monitor: monitors
            .iter()
            .find(|monitor| Some(monitor.id) == active_window.monitor)
            .or_else(|| monitors.iter().find(|monitor| monitor.focused))
            .map(|monitor| monitor.name.clone())
            .unwrap_or_default(),
        title: active_window.title,
    }
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
        "focusedmonv2" | "createworkspacev2" | "destroyworkspacev2" | "moveworkspacev2"
        | "renameworkspace" | "changeworkspaceid" | "closewindow" | "movewindowv2" | "urgent"
        | "configreloaded" | "monitoraddedv2" | "monitorremovedv2" => Event::Refresh,
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
        Event::Workspace(_) | Event::Title { .. } | Event::Ignore => false,
    }
}

fn is_active_address(address: &str, active_address: Option<&str>) -> bool {
    active_address == Some(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(id: i64, name: &str, monitor: &str, urgent: bool) -> Workspace {
        Workspace {
            id,
            name: name.into(),
            monitor: monitor.into(),
            urgent,
        }
    }

    fn two_monitors(active_window: &str) -> State {
        state_from_parts(
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
            serde_json::from_str(
                r#"[
                    {"id": 0, "name": "DP-1", "activeWorkspace": {"id": 1}, "focused": true},
                    {"id": 1, "name": "HDMI-A-1", "activeWorkspace": {"id": 2}, "focused": false}
                ]"#,
            )
            .unwrap(),
            serde_json::from_str(active_window).unwrap(),
        )
    }

    #[test]
    fn shows_each_monitor_its_own_regular_workspaces() {
        let state = two_monitors(r#"{"title": "Terminal", "monitor": 0}"#);

        assert_eq!(state.window_monitor, "DP-1");
        assert_eq!(state.title, "Terminal");
        assert_eq!(
            state.shown("DP-1"),
            Shown {
                workspaces: vec![
                    workspace(1, "1", "DP-1", false),
                    workspace(3, "3", "DP-1", true),
                    workspace(-1337, "development", "DP-1", false),
                ],
                active_id: Some(1),
            }
        );
        assert_eq!(
            state.shown("HDMI-A-1"),
            Shown {
                workspaces: vec![workspace(2, "2", "HDMI-A-1", false)],
                active_id: Some(2),
            }
        );
        assert_eq!(state.shown("eDP-1"), Shown::default());
    }

    #[test]
    fn titles_the_focused_window_on_its_own_monitor() {
        let state = two_monitors(r#"{"title": "Editor", "monitor": 1}"#);
        assert_eq!(state.window_monitor, "HDMI-A-1");

        let state = two_monitors("{}");
        assert_eq!(state.window_monitor, "DP-1");
    }

    #[test]
    fn switching_workspaces_activates_them_on_their_monitor() {
        let mut state = two_monitors("{}");

        state.activate(3);
        assert_eq!(state.shown("DP-1").active_id, Some(3));
        assert_eq!(state.shown("HDMI-A-1").active_id, Some(2));

        state.activate(7);
        assert_eq!(state.shown("DP-1").active_id, Some(3));
        assert_eq!(state.shown("HDMI-A-1").active_id, Some(2));
    }

    #[test]
    fn applies_worker_updates_to_the_last_full_state() {
        let mut state = State::default();
        state.apply(&Update::State(two_monitors("{}")));
        state.apply(&Update::ActiveWorkspace(3));
        state.apply(&Update::Title("Editor".into()));

        assert_eq!(state.shown("DP-1").active_id, Some(3));
        assert_eq!(state.title, "Editor");

        state.apply(&Update::State(two_monitors("{}")));
        assert_eq!(state.shown("DP-1").active_id, Some(1));
        assert_eq!(state.title, "");
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
        assert_eq!(parse_event("submap>>Resize windows"), Event::Ignore);
        assert_eq!(parse_event("configreloaded>>"), Event::Refresh);
        assert_eq!(parse_event("monitoraddedv2>>1,DP-2,desc"), Event::Refresh);
        assert_eq!(parse_event("monitorremovedv2>>1,DP-2,desc"), Event::Refresh);
        assert_eq!(parse_event("windowtitle>>0x123"), Event::Ignore);
    }

    #[test]
    fn workspace_id_changes_refresh_without_a_focus_change() {
        for line in [
            "changeworkspaceid>>1,4294967294",
            "changeworkspaceid>>2,1",
            "changeworkspaceid>>4294967294,2",
        ] {
            let event = parse_event(line);
            assert_eq!(event, Event::Refresh);
            assert!(event_needs_refresh(&event, Some("123")));
            assert!(event_needs_refresh(&event, None));
        }
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

    fn overlay(icon: &str, text: &str, items: serde_json::Value) -> Overlay {
        title::parse(
            &serde_json::json!({"version": 1, "id": "test", "icon": icon, "text": text, "items": items})
                .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn escapes_overlay_text_and_items() {
        let overlay = overlay(
            "",
            "<Resize>",
            serde_json::json!([{"key": "<&", "label": "<Resize & move>"}, {"key": "x"}]),
        );
        assert_eq!(
            overlay_markup(&overlay, &overlay.items),
            "&lt;Resize&gt; · <b>&lt;&amp;</b> &lt;Resize &amp; move&gt; · <b>x</b>"
        );
    }

    #[test]
    fn overflow_keeps_whole_items_heading_and_pinned_items() {
        let overlay = overlay(
            "󰌌",
            "Resize",
            serde_json::json!([
                {"key": "hjkl/←↓↑→", "label": "Resize"},
                {"key": "Esc/Enter", "label": "Exit", "pinned": true},
                {"key": "Shift+hjkl", "label": "Precise"},
            ]),
        );
        let full = fit_overlay_markup(&overlay, |_| true);
        assert_eq!(
            full,
            "󰌌 Resize · <b>hjkl/←↓↑→</b> Resize · <b>Shift+hjkl</b> Precise · <b>Esc/Enter</b> Exit"
        );
        let compact = fit_overlay_markup(&overlay, |markup| !markup.contains("Precise"));
        assert_eq!(
            compact,
            "󰌌 Resize · <b>hjkl/←↓↑→</b> Resize · <b>+1</b> more · <b>Esc/Enter</b> Exit"
        );
        let narrow = fit_overlay_markup(&overlay, |_| false);
        assert_eq!(narrow, "󰌌 Resize · <b>+2</b> more · <b>Esc/Enter</b> Exit");
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
    fn only_waits_for_pending_titles() {
        let start = Instant::now();
        let mut titles = TitleUpdates::default();
        assert_eq!(titles.delay(start), None);

        titles.queue("first".into());
        assert_eq!(titles.take_ready(start), Some("first".into()));
        assert_eq!(titles.delay(start), None);

        titles.queue("second".into());
        assert_eq!(
            titles.delay(start + TITLE_UPDATE_INTERVAL / 5),
            Some(TITLE_UPDATE_INTERVAL * 4 / 5)
        );
        assert_eq!(
            titles.delay(start + TITLE_UPDATE_INTERVAL),
            Some(Duration::from_millis(1))
        );
    }

    #[test]
    fn queries_and_parses_workspace_urgency_in_order() {
        assert!(urgency_query([1, -1337].into_iter()).contains("ipairs({ 1, -1337 })"));
        assert_eq!(
            parse_urgency("false,true\n").take(3).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(
            parse_urgency("error: invalid").take(2).collect::<Vec<_>>(),
            [false, false]
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
        let shown = Shown {
            workspaces: vec![workspace(1, "1", "DP-1", false)],
            active_id: Some(1),
        };
        let mut changed = shown.clone();
        changed.active_id = Some(2);
        changed.workspaces[0].urgent = true;
        assert!(!workspace_structure_changed(&shown, &changed));

        changed.workspaces[0].name = "renamed".into();
        assert!(workspace_structure_changed(&shown, &changed));
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
