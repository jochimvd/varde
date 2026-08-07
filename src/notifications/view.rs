use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    time::Duration,
};

use gio::prelude::*;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

use super::{Manager, model::Snapshot};

const POPUP_NAME: &str = "varde-notification-popups";
const PANEL_WIDTH: i32 = 460;
const POPUP_WIDTH: i32 = 400;
const PANEL_GAP: i32 = 10;
const POPUP_TOP: i32 = 42;
const POPUP_PICTURE_SIZE: i32 = 54;
const MAX_POPUPS: usize = 5;
const MAX_CONTENT_HEIGHT: i32 = 520;
const DOT_SIZE: i32 = 5;
const DOT_RIGHT_OFFSET: i32 = 0;
const DOT_TOP: i32 = 4;

pub(super) struct Bell {
    pub button: gtk::Button,
    label: gtk::Label,
    dot: gtk::Box,
    class: RefCell<String>,
}

impl Bell {
    pub fn new(manager: &Rc<Manager>, app: &gtk::Application) -> Self {
        let button = gtk::Button::builder().focusable(false).build();
        button.add_css_class("module");
        button.add_css_class("notification");

        let label = gtk::Label::new(None);
        let dot = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .can_target(false)
            .build();
        dot.add_css_class("notification-dot");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&label));
        overlay.add_overlay(&dot);
        overlay.set_clip_overlay(&dot, false);
        overlay.connect_get_child_position(|overlay, _| {
            Some(gdk::Rectangle::new(
                overlay.width() - DOT_SIZE + DOT_RIGHT_OFFSET,
                DOT_TOP,
                DOT_SIZE,
                DOT_SIZE,
            ))
        });
        button.set_overflow(gtk::Overflow::Visible);
        button.set_child(Some(&overlay));

        button.connect_clicked({
            let app = app.clone();
            move |_| app.activate_action("notifications", None)
        });
        for mouse_button in [2, 3] {
            let click = gtk::GestureClick::new();
            click.set_button(mouse_button);
            click.connect_released({
                let manager = Rc::downgrade(manager);
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        match mouse_button {
                            2 => manager.toggle_dnd(),
                            3 => manager.clear(),
                            _ => unreachable!(),
                        }
                    }
                }
            });
            button.add_controller(click);
        }

        Self {
            button,
            label,
            dot,
            class: RefCell::new(String::new()),
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        let alt = snapshot.alt();
        self.label.set_text(if snapshot.dnd { "󰂛" } else { "󰂚" });
        self.dot.set_visible(snapshot.count > 0);
        self.button.set_tooltip_text(Some(&snapshot.tooltip()));

        let mut current = self.class.borrow_mut();
        if *current != alt {
            if !current.is_empty() {
                self.button.remove_css_class(&current);
            }
            self.button.add_css_class(alt);
            *current = alt.into();
        }
    }
}

pub(super) struct Popups {
    window: gtk::ApplicationWindow,
    list: gtk::Box,
    manager: std::rc::Weak<Manager>,
    state: RefCell<PopupState>,
}

#[derive(Default)]
struct PopupState {
    displayed: HashMap<u32, u64>,
    visible: HashSet<u32>,
}

impl PopupState {
    fn update(&mut self, snapshot: &Snapshot, blocked: bool) -> Vec<(u32, u64)> {
        let active_ids = snapshot
            .groups
            .iter()
            .flat_map(|group| &group.notifications)
            .filter(|notification| notification.active)
            .map(|notification| notification.id)
            .collect::<HashSet<_>>();
        self.displayed.retain(|id, _| active_ids.contains(id));
        self.visible.retain(|id| active_ids.contains(id));
        if blocked {
            self.visible.clear();
            return Vec::new();
        }
        let active = snapshot
            .groups
            .iter()
            .flat_map(|group| &group.notifications)
            .filter(|notification| notification.active)
            .collect::<Vec<_>>();
        let mut displayed = Vec::new();
        for notification in &active {
            if self.visible.contains(&notification.id)
                && self.displayed.get(&notification.id) != Some(&notification.revision)
            {
                self.displayed
                    .insert(notification.id, notification.revision);
                displayed.push((notification.id, notification.revision));
            }
        }
        let mut available = MAX_POPUPS.saturating_sub(self.visible.len());
        for notification in active {
            if available == 0 || self.displayed.contains_key(&notification.id) {
                continue;
            }
            available -= 1;
            self.visible.insert(notification.id);
            self.displayed
                .insert(notification.id, notification.revision);
            displayed.push((notification.id, notification.revision));
        }
        displayed
    }
}

fn set_picture(image: &gtk::Image, notification: &super::model::Notification) -> bool {
    if let Some(data) = &notification.image_data {
        let format = if data.has_alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        let bytes = glib::Bytes::from_owned(data.bytes.clone());
        let texture =
            gdk::MemoryTexture::new(data.width, data.height, format, &bytes, data.rowstride);
        image.set_from_gicon(&texture);
        return true;
    }
    if let Some(icon) = notification.image.as_deref().map(notification_icon) {
        image.set_from_gicon(&icon);
        return true;
    }
    false
}

fn progress_bar(value: u8) -> gtk::ProgressBar {
    let bar = gtk::ProgressBar::builder()
        .fraction(f64::from(value) / 100.0)
        .hexpand(true)
        .build();
    bar.add_css_class("notification-progress");
    bar
}

impl Popups {
    pub fn new(app: &gtk::Application, manager: &Rc<Manager>) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .name(POPUP_NAME)
            .default_width(POPUP_WIDTH)
            .build();
        window.add_css_class("notification-popups");
        window.init_layer_shell();
        window.set_namespace(Some(POPUP_NAME));
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        window.set_anchor(gtk4_layer_shell::Edge::Top, true);
        window.set_anchor(gtk4_layer_shell::Edge::Right, true);
        window.set_margin(gtk4_layer_shell::Edge::Top, POPUP_TOP);
        window.set_margin(gtk4_layer_shell::Edge::Right, PANEL_GAP);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::None);

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .width_request(POPUP_WIDTH)
            .build();
        window.set_child(Some(&list));

        Self {
            window,
            list,
            manager: Rc::downgrade(manager),
            state: RefCell::new(PopupState::default()),
        }
    }

    pub fn update(&self, snapshot: &Snapshot, center_open: bool) {
        let mut state = self.state.borrow_mut();
        let displayed = state.update(snapshot, snapshot.dnd || center_open);
        if !displayed.is_empty()
            && let Some(manager) = self.manager.upgrade()
        {
            manager.displayed(displayed);
        }

        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let mut shown = 0;
        for group in &snapshot.groups {
            let (app_name, app_icon) = application(group);
            for notification in group
                .notifications
                .iter()
                .filter(|notification| state.visible.contains(&notification.id))
            {
                self.list.append(&popup_widget(
                    &app_name,
                    app_icon.as_ref(),
                    notification,
                    &self.manager,
                ));
                shown += 1;
                if shown == MAX_POPUPS {
                    break;
                }
            }
            if shown == MAX_POPUPS {
                break;
            }
        }
        self.window
            .set_visible(shown > 0 && !snapshot.dnd && !center_open);
    }

    pub fn hide(&self) {
        self.state.borrow_mut().visible.clear();
        self.window.set_visible(false);
    }
}

fn popup_widget(
    app_name: &str,
    app_icon: Option<&gio::Icon>,
    notification: &super::model::Notification,
    manager: &std::rc::Weak<Manager>,
) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    card.add_css_class("notification-popup");
    if notification.urgency.as_deref() == Some("critical") {
        card.add_css_class("critical");
    }

    let app_image = gtk::Image::builder().pixel_size(18).build();
    if let Some(icon) = app_icon {
        app_image.set_from_gicon(icon);
    }
    app_image.set_visible(app_icon.is_some());
    let app = gtk::Label::builder()
        .label(app_name)
        .hexpand(true)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    app.add_css_class("notification-popup-app");
    let header = gtk::Box::builder()
        .spacing(7)
        .valign(gtk::Align::Center)
        .build();
    header.append(&app_image);
    header.append(&app);
    card.append(&header);

    let content = gtk::Box::builder()
        .spacing(10)
        .valign(gtk::Align::Start)
        .build();
    let picture_widget = gtk::Image::builder()
        .pixel_size(POPUP_PICTURE_SIZE)
        .width_request(POPUP_PICTURE_SIZE)
        .height_request(POPUP_PICTURE_SIZE)
        .valign(gtk::Align::Start)
        .build();
    let has_picture = set_picture(&picture_widget, notification);
    picture_widget.set_visible(has_picture);
    content.append(&picture_widget);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();

    let summary = gtk::Label::builder()
        .label(&notification.summary)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    summary.add_css_class("notification-popup-summary");
    text.append(&summary);
    if !notification.body.trim().is_empty() {
        let body = gtk::Label::builder()
            .label(&notification.body)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        body.add_css_class("notification-popup-body");
        text.append(&body);
    }
    if let Some(value) = notification.progress {
        text.append(&progress_bar(value));
    }
    content.append(&text);
    card.append(&content);

    let activate = gtk::GestureClick::new();
    activate.set_button(1);
    activate.connect_released({
        let manager = manager.clone();
        let id = notification.id;
        move |_, _, _, _| {
            if let Some(manager) = manager.upgrade() {
                manager.invoke_default(id);
            }
        }
    });
    card.add_controller(activate);

    let dismiss = gtk::GestureClick::new();
    dismiss.set_button(3);
    dismiss.connect_released({
        let manager = manager.clone();
        let id = notification.id;
        move |_, _, _, _| {
            if let Some(manager) = manager.upgrade() {
                manager.dismiss(id, true);
            }
        }
    });
    card.add_controller(dismiss);
    card
}

pub(super) struct Center {
    popover: gtk::Popover,
    groups: gtk::Box,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    collapsed: Rc<RefCell<HashSet<String>>>,
    manager: std::rc::Weak<Manager>,
    interactive: bool,
}

impl Center {
    pub fn new(anchor: &gtk::Button, manager: &Rc<Manager>, interactive: bool) -> Self {
        let popover = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.add_css_class("notifications");
        popover.set_halign(gtk::Align::End);
        popover.set_offset(0, PANEL_GAP);
        popover.set_parent(anchor);
        popover.connect_closed({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.center_closed();
                }
            }
        });

        let title = gtk::Label::builder()
            .label("Notifications")
            .hexpand(true)
            .xalign(0.0)
            .build();
        title.add_css_class("notification-center-title");
        let dnd = gtk::Button::with_label("󰂛");
        dnd.add_css_class("notification-center-control");
        dnd.add_css_class("notification-center-icon");
        dnd.set_tooltip_text(Some("Enable Do Not Disturb"));
        dnd.connect_clicked({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.toggle_dnd();
                }
            }
        });
        let clear = gtk::Button::with_label("󰆴");
        clear.add_css_class("notification-center-control");
        clear.add_css_class("notification-center-icon");
        clear.set_tooltip_text(Some("Clear all notifications"));
        clear.connect_clicked({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.clear();
                }
            }
        });
        let header = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        header.add_css_class("notification-center-header");
        header.append(&title);
        header.append(&dnd);
        header.append(&clear);

        let groups = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        let collapsed = Rc::new(RefCell::new(HashSet::new()));
        let scroll = gtk::ScrolledWindow::builder()
            .child(&groups)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(MAX_CONTENT_HEIGHT)
            .propagate_natural_height(true)
            .build();
        scroll.add_css_class("notification-center-scroll");

        let empty = message("No notifications", "notification-center-empty");
        let unavailable = message(
            "Notifications are unavailable",
            "notification-center-unavailable",
        );
        let stack = gtk::Stack::new();
        stack.add_named(&scroll, Some("content"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&unavailable, Some("unavailable"));

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .width_request(PANEL_WIDTH)
            .build();
        panel.add_css_class("notification-center");
        panel.append(&header);
        panel.append(&stack);

        popover.set_child(Some(&panel));
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let manager = Rc::downgrade(manager);
            move |_, key, _, _| {
                if key != gdk::Key::Escape {
                    return glib::Propagation::Proceed;
                }
                if let Some(manager) = manager.upgrade() {
                    manager.close();
                }
                glib::Propagation::Stop
            }
        });
        popover.add_controller(keys);

        Self {
            popover,
            groups,
            stack,
            dnd,
            clear,
            collapsed,
            manager: Rc::downgrade(manager),
            interactive,
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        self.collapsed
            .borrow_mut()
            .retain(|key| snapshot.groups.iter().any(|group| group.key == *key));
        while let Some(child) = self.groups.first_child() {
            self.groups.remove(&child);
        }
        for group in &snapshot.groups {
            self.groups.append(&group_widget(
                group,
                &self.collapsed,
                &self.popover,
                &self.manager,
                self.interactive,
            ));
        }
        self.stack.set_visible_child_name(if !snapshot.available {
            "unavailable"
        } else if snapshot.count == 0 {
            "empty"
        } else {
            "content"
        });
        if snapshot.dnd {
            self.dnd.add_css_class("active");
            self.dnd.set_tooltip_text(Some("Disable Do Not Disturb"));
        } else {
            self.dnd.remove_css_class("active");
            self.dnd.set_tooltip_text(Some("Enable Do Not Disturb"));
        }
        self.dnd.set_sensitive(snapshot.available);
        self.clear
            .set_sensitive(snapshot.available && snapshot.count > 0);
    }

    pub fn show(&self) {
        self.popover.popup();
    }

    pub fn hide(&self) {
        self.popover.popdown();
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }
}

fn group_widget(
    group: &super::model::Group,
    collapsed: &Rc<RefCell<HashSet<String>>>,
    popover: &gtk::Popover,
    manager: &std::rc::Weak<Manager>,
    interactive: bool,
) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    container.add_css_class("notification-group");

    let (name, icon) = application(group);
    let image = gtk::Image::builder().pixel_size(20).build();
    if let Some(icon) = icon {
        image.set_from_gicon(&icon);
    }
    image.add_css_class("notification-group-icon");
    let name = gtk::Label::builder()
        .label(name)
        .hexpand(true)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let count = gtk::Label::builder()
        .label(group.notifications.len().to_string())
        .valign(gtk::Align::Center)
        .build();
    count.add_css_class("notification-group-count");
    let disclosure = gtk::Label::new(None);
    disclosure.add_css_class("notification-group-disclosure");
    let header = gtk::Box::builder()
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    header.add_css_class("notification-group-header");
    header.append(&image);
    header.append(&name);
    header.append(&count);
    header.append(&disclosure);

    let rows = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();

    for notification in &group.notifications {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        row.add_css_class("notification-row");
        if notification.urgency.as_deref() == Some("critical") {
            row.add_css_class("critical");
        }
        let summary = gtk::Label::builder()
            .label(&notification.summary)
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("notification-summary");
        let header = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        header.append(&summary);
        if let Some(time) = notification_time(notification.received_at) {
            let time = gtk::Label::new(Some(&time));
            time.add_css_class("notification-time");
            header.append(&time);
        }
        row.append(&header);
        if !notification.body.trim().is_empty() {
            let body = gtk::Label::builder()
                .label(&notification.body)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(3)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            body.add_css_class("notification-body");
            row.append(&body);
        }
        if let Some(value) = notification.progress {
            row.append(&progress_bar(value));
        }
        if interactive {
            let activate = gtk::GestureClick::new();
            activate.set_button(1);
            activate.connect_released({
                let manager = manager.clone();
                let id = notification.id;
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        manager.invoke_default(id);
                    }
                }
            });
            row.add_controller(activate);
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_released({
                let manager = manager.clone();
                let id = notification.id;
                let active = notification.active;
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        manager.dismiss(id, active);
                    }
                }
            });
            row.add_controller(dismiss);
        }
        rows.append(&row);
    }

    let is_collapsed = collapsed.borrow().contains(&group.key);
    disclosure.set_text(if is_collapsed { "▸" } else { "▾" });
    let revealer = gtk::Revealer::builder()
        .transition_duration(150)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .reveal_child(!is_collapsed)
        .child(&rows)
        .build();

    let toggle = gtk::Button::builder()
        .focusable(false)
        .hexpand(true)
        .child(&header)
        .build();
    toggle.add_css_class("notification-group-toggle");
    toggle.connect_clicked({
        let collapsed = Rc::clone(collapsed);
        let key = group.key.clone();
        let revealer = revealer.clone();
        let popover = popover.clone();
        move |_| {
            let reveal = !revealer.reveals_child();
            revealer.set_reveal_child(reveal);
            disclosure.set_text(if reveal { "▾" } else { "▸" });
            if reveal {
                collapsed.borrow_mut().remove(&key);
            } else {
                collapsed.borrow_mut().insert(key.clone());
            }
            glib::timeout_add_local_once(Duration::from_millis(160), {
                let popover = popover.clone();
                move || {
                    popover.queue_resize();
                }
            });
        }
    });
    if interactive {
        let dismiss = gtk::GestureClick::new();
        dismiss.set_button(3);
        dismiss.connect_released({
            let manager = manager.clone();
            let notifications = group
                .notifications
                .iter()
                .map(|notification| (notification.id, notification.active))
                .collect::<Vec<_>>();
            move |_, _, _, _| {
                if let Some(manager) = manager.upgrade() {
                    manager.dismiss_group(notifications.clone());
                }
            }
        });
        toggle.add_controller(dismiss);
    }
    container.append(&toggle);
    container.append(&revealer);
    container
}

fn application(group: &super::model::Group) -> (String, Option<gio::Icon>) {
    if let Some(entry) = group.desktop_entry.as_deref().and_then(desktop_info) {
        return (entry.display_name().to_string(), entry.icon());
    }
    let icon = group.icon.as_deref().map(notification_icon);
    (group.name.clone(), icon)
}

fn notification_icon(icon: &str) -> gio::Icon {
    if Path::new(icon).is_absolute() {
        gio::FileIcon::new(&gio::File::for_path(icon)).upcast()
    } else if icon.starts_with("file://") {
        gio::FileIcon::new(&gio::File::for_uri(icon)).upcast()
    } else {
        gio::ThemedIcon::new(icon).upcast()
    }
}

fn notification_time(timestamp: Option<i64>) -> Option<String> {
    glib::DateTime::from_unix_local(timestamp?)
        .ok()?
        .format("%H:%M")
        .ok()
        .map(|time| time.to_string())
}

fn desktop_info(id: &str) -> Option<gio::DesktopAppInfo> {
    gio::DesktopAppInfo::new(id).or_else(|| {
        (!id.ends_with(".desktop"))
            .then(|| gio::DesktopAppInfo::new(&format!("{id}.desktop")))
            .flatten()
    })
}

fn message(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(0.0).build();
    label.add_css_class(class);
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacements_update_visible_popups_without_resurfacing_hidden_ones() {
        let first = super::super::model::parse(
            br#"[{"id":1,"revision":1,"app_name":"Test","summary":"Same"}]"#,
            b"[]",
            false,
        )
        .unwrap();
        let replaced = super::super::model::parse(
            br#"[{"id":1,"revision":2,"app_name":"Test","summary":"Same"}]"#,
            b"[]",
            false,
        )
        .unwrap();
        let empty = super::super::model::parse(b"[]", b"[]", false).unwrap();
        let mut state = PopupState::default();

        assert_eq!(state.update(&first, false), vec![(1, 1)]);
        assert!(state.visible.contains(&1));

        assert_eq!(state.update(&replaced, false), vec![(1, 2)]);
        assert!(state.visible.contains(&1));

        state.update(&replaced, true);
        assert!(state.visible.is_empty());
        assert!(state.update(&replaced, false).is_empty());
        assert!(state.visible.is_empty());

        state.update(&empty, false);
        assert_eq!(state.update(&first, false), vec![(1, 1)]);
        assert!(state.visible.contains(&1));
    }

    #[test]
    fn blocked_and_queued_notifications_wait_until_they_can_be_displayed() {
        let json = (1..=MAX_POPUPS + 1)
            .map(|id| format!(r#"{{"id":{id},"revision":1,"summary":"{id}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let snapshot =
            super::super::model::parse(format!("[{json}]").as_bytes(), b"[]", false).unwrap();
        let mut state = PopupState::default();

        assert!(state.update(&snapshot, true).is_empty());
        assert_eq!(state.update(&snapshot, false).len(), MAX_POPUPS);
        let first = *state.visible.iter().next().unwrap();
        let reduced = super::super::model::parse(
            format!(
                "[{}]",
                (1..=MAX_POPUPS + 1)
                    .filter(|id| *id != first as usize)
                    .map(|id| format!(r#"{{"id":{id},"revision":1,"summary":"{id}"}}"#))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .as_bytes(),
            b"[]",
            false,
        )
        .unwrap();
        assert_eq!(state.update(&reduced, false).len(), 1);
    }
}
