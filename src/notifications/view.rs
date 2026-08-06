use std::{cell::RefCell, collections::HashSet, path::Path, rc::Rc, time::Duration};

use gio::prelude::*;
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

use super::{Manager, model::Snapshot};

const CENTER_NAME: &str = "shell-notifications";
const PANEL_WIDTH: i32 = 460;
const PANEL_GAP: i32 = 10;
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
                            2 => manager.clear(),
                            3 => manager.toggle_dnd(),
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

pub(super) struct Center {
    window: gtk::ApplicationWindow,
    groups: gtk::Box,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    collapsed: Rc<RefCell<HashSet<String>>>,
}

impl Center {
    pub fn new(app: &gtk::Application, manager: &Rc<Manager>) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .name(CENTER_NAME)
            .default_width(PANEL_WIDTH)
            .build();
        window.add_css_class("notifications");
        window.init_layer_shell();
        window.set_namespace(Some(CENTER_NAME));
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        window.set_anchor(gtk4_layer_shell::Edge::Top, true);
        window.set_anchor(gtk4_layer_shell::Edge::Right, true);
        window.set_margin(gtk4_layer_shell::Edge::Top, PANEL_GAP);
        window.set_margin(gtk4_layer_shell::Edge::Right, PANEL_GAP);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

        let title = gtk::Label::builder()
            .label("Notifications")
            .hexpand(true)
            .xalign(0.0)
            .build();
        title.add_css_class("notification-center-title");
        let dnd = gtk::Button::with_label("Do Not Disturb");
        dnd.add_css_class("notification-center-control");
        dnd.connect_clicked({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.toggle_dnd();
                }
            }
        });
        let clear = gtk::Button::with_label("Clear All");
        clear.add_css_class("notification-center-control");
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
        window.set_child(Some(&panel));

        window.connect_close_request({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.close();
                }
                glib::Propagation::Stop
            }
        });
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
        window.add_controller(keys);

        Self {
            window,
            groups,
            stack,
            dnd,
            clear,
            collapsed,
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
            self.groups
                .append(&group_widget(group, &self.collapsed, &self.window));
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
        } else {
            self.dnd.remove_css_class("active");
        }
        self.dnd.set_sensitive(snapshot.available);
        self.clear
            .set_sensitive(snapshot.available && snapshot.count > 0);
    }

    pub fn show(&self) {
        self.window.present();
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }
}

fn group_widget(
    group: &super::model::Group,
    collapsed: &Rc<RefCell<HashSet<String>>>,
    window: &gtk::ApplicationWindow,
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
    let count = gtk::Label::new(Some(&group.notifications.len().to_string()));
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
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("notification-summary");
        row.append(&summary);
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
        let window = window.clone();
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
                let window = window.clone();
                move || {
                    window.set_default_size(PANEL_WIDTH, -1);
                    window.queue_resize();
                }
            });
        }
    });
    container.append(&toggle);
    container.append(&revealer);
    container
}

fn application(group: &super::model::Group) -> (String, Option<gio::Icon>) {
    if let Some(entry) = group.desktop_entry.as_deref().and_then(desktop_info) {
        return (entry.display_name().to_string(), entry.icon());
    }
    let icon = group.icon.as_deref().map(|icon| {
        if Path::new(icon).is_absolute() {
            gio::FileIcon::new(&gio::File::for_path(icon)).upcast()
        } else {
            gio::ThemedIcon::new(icon).upcast()
        }
    });
    (group.name.clone(), icon)
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
