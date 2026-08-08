use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};

use super::{
    super::{
        Manager,
        model::{Group, Notification, Snapshot},
    },
    common::{application, message, notification_time, progress_bar, set_picture},
};

const PANEL_WIDTH: i32 = 460;
const PANEL_RIGHT: i32 = 20;
const PANEL_TOP: i32 = 18;
const MAX_CONTENT_HEIGHT: i32 = 520;
const GROUP_TRANSITION_DURATION: u32 = 150;

pub(in crate::notifications) struct Center {
    popover: gtk::Popover,
    groups: gtk::Box,
    group_views: RefCell<Vec<GroupView>>,
    group_order: RefCell<Vec<String>>,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    collapsed: Rc<RefCell<HashSet<String>>>,
    manager: std::rc::Weak<Manager>,
    interactive: bool,
}

impl Center {
    pub fn new(anchor: &gtk::ApplicationWindow, manager: &Rc<Manager>, interactive: bool) -> Self {
        let popover = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.add_css_class("notifications");
        popover.set_halign(gtk::Align::End);
        popover.set_offset(-PANEL_RIGHT, PANEL_TOP);
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

        let empty = gtk::Label::new(Some("All caught up"));
        empty.add_css_class("notification-center-empty");
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
            group_views: RefCell::new(Vec::new()),
            group_order: RefCell::new(Vec::new()),
            stack,
            dnd,
            clear,
            collapsed,
            manager: Rc::downgrade(manager),
            interactive,
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        self.render(snapshot, !self.popover.is_visible());
    }

    fn render(&self, snapshot: &Snapshot, reset_order: bool) {
        self.collapsed
            .borrow_mut()
            .retain(|key| snapshot.groups.iter().any(|group| group.key == *key));

        let keys = snapshot
            .groups
            .iter()
            .map(|group| group.key.clone())
            .collect::<Vec<_>>();
        let groups = {
            let mut order = self.group_order.borrow_mut();
            update_group_order(&mut order, &keys, reset_order);
            order
                .iter()
                .filter_map(|key| snapshot.groups.iter().find(|group| group.key == *key))
                .collect::<Vec<_>>()
        };

        let mut views = self.group_views.borrow_mut();
        while views.len() < groups.len() {
            let view = GroupView::new(
                &self.collapsed,
                &self.popover,
                &self.manager,
                self.interactive,
            );
            self.groups.append(&view.container);
            views.push(view);
        }
        for (view, group) in views.iter_mut().zip(groups.iter().copied()) {
            view.update(group);
        }
        while views.len() > groups.len() {
            let view = views.pop().expect("group view count exceeds snapshot");
            self.groups.remove(&view.container);
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
        if self.popover.is_visible() {
            self.popover.queue_resize();
            self.popover.present();
        }
    }

    pub fn show(&self, snapshot: &Snapshot) {
        self.render(snapshot, true);
        self.popover.popup();
    }

    pub fn hide(&self) {
        self.popover.popdown();
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }
}

pub(super) fn update_group_order(order: &mut Vec<String>, keys: &[String], reset: bool) {
    if reset {
        order.clear();
        order.extend_from_slice(keys);
        return;
    }

    order.retain(|key| keys.contains(key));
    for key in keys {
        if !order.contains(key) {
            order.push(key.clone());
        }
    }
}

struct GroupView {
    container: gtk::Box,
    icon: gtk::Stack,
    image: gtk::Image,
    name: gtk::Label,
    count: gtk::Label,
    disclosure: gtk::Label,
    rows: gtk::Box,
    revealer: gtk::Revealer,
    row_views: Vec<RowView>,
    key: Rc<RefCell<String>>,
    notifications: Rc<RefCell<Vec<(u32, bool)>>>,
    collapsed: Rc<RefCell<HashSet<String>>>,
    manager: std::rc::Weak<Manager>,
    interactive: bool,
}

impl GroupView {
    fn new(
        collapsed: &Rc<RefCell<HashSet<String>>>,
        popover: &gtk::Popover,
        manager: &std::rc::Weak<Manager>,
        interactive: bool,
    ) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-group");

        let image = gtk::Image::builder().pixel_size(20).build();
        let fallback = gtk::Box::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        fallback.add_css_class("notification-group-icon-fallback");
        let icon = gtk::Stack::builder()
            .width_request(20)
            .height_request(20)
            .build();
        icon.add_named(&image, Some("image"));
        icon.add_named(&fallback, Some("fallback"));
        icon.add_css_class("notification-group-icon");
        let name = gtk::Label::builder()
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let count = gtk::Label::builder().valign(gtk::Align::Center).build();
        count.add_css_class("notification-group-count");
        let disclosure = gtk::Label::new(None);
        disclosure.add_css_class("notification-group-disclosure");
        let header = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        header.add_css_class("notification-group-header");
        header.append(&icon);
        header.append(&name);
        header.append(&count);
        header.append(&disclosure);

        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_duration(GROUP_TRANSITION_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&rows)
            .build();

        let key = Rc::new(RefCell::new(String::new()));
        let notifications = Rc::new(RefCell::new(Vec::new()));
        let pressed_key = Rc::new(RefCell::new(None));
        let toggle = gtk::Button::builder()
            .focusable(false)
            .hexpand(true)
            .child(&header)
            .build();
        toggle.add_css_class("notification-group-toggle");
        toggle.connect_state_flags_changed({
            let key = Rc::clone(&key);
            let pressed_key = Rc::clone(&pressed_key);
            move |_, flags| {
                if flags.contains(gtk::StateFlags::ACTIVE) {
                    pressed_key.replace(Some(key.borrow().clone()));
                }
            }
        });
        toggle.connect_clicked({
            let collapsed = Rc::clone(collapsed);
            let key = Rc::clone(&key);
            let pressed_key = Rc::clone(&pressed_key);
            let revealer = revealer.clone();
            let disclosure = disclosure.clone();
            let popover = popover.clone();
            move |_| {
                let pressed = pressed_key
                    .borrow_mut()
                    .take()
                    .unwrap_or_else(|| key.borrow().clone());
                if pressed != *key.borrow() {
                    return;
                }
                let reveal = !revealer.reveals_child();
                revealer.set_reveal_child(reveal);
                disclosure.set_text(if reveal { "▾" } else { "▸" });
                if reveal {
                    collapsed.borrow_mut().remove(&pressed);
                } else {
                    collapsed.borrow_mut().insert(pressed);
                }
                revealer.add_tick_callback({
                    let popover = popover.clone();
                    move |revealer, _| {
                        popover.queue_resize();
                        popover.present();
                        if revealer.reveals_child() == revealer.is_child_revealed() {
                            glib::ControlFlow::Break
                        } else {
                            glib::ControlFlow::Continue
                        }
                    }
                });
            }
        });
        if interactive {
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            let pressed_notifications = Rc::new(RefCell::new(None));
            dismiss.connect_pressed({
                let notifications = Rc::clone(&notifications);
                let pressed_notifications = Rc::clone(&pressed_notifications);
                move |_, _, _, _| {
                    pressed_notifications.replace(Some(notifications.borrow().clone()));
                }
            });
            dismiss.connect_released({
                let toggle = toggle.clone();
                let manager = manager.clone();
                let notifications = Rc::clone(&notifications);
                let pressed_notifications = Rc::clone(&pressed_notifications);
                move |_, _, x, y| {
                    let target = pressed_notifications
                        .borrow_mut()
                        .take()
                        .unwrap_or_else(|| notifications.borrow().clone());
                    if toggle.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.dismiss_group(target);
                    }
                }
            });
            toggle.add_controller(dismiss);
        }

        container.append(&toggle);
        container.append(&revealer);
        Self {
            container,
            icon,
            image,
            name,
            count,
            disclosure,
            rows,
            revealer,
            row_views: Vec::new(),
            key,
            notifications,
            collapsed: Rc::clone(collapsed),
            manager: manager.clone(),
            interactive,
        }
    }

    fn update(&mut self, group: &Group) {
        let (name, icon) = application(group);
        self.image.clear();
        if let Some(icon) = icon {
            self.image.set_from_gicon(&icon);
            self.icon.set_visible_child_name("image");
        } else {
            self.icon.set_visible_child_name("fallback");
        }
        self.name.set_label(&name);
        self.count.set_label(&group.notifications.len().to_string());
        self.key.replace(group.key.clone());
        self.notifications.replace(
            group
                .notifications
                .iter()
                .map(|notification| (notification.id, notification.active))
                .collect(),
        );

        let is_collapsed = self.collapsed.borrow().contains(&group.key);

        while self.row_views.len() < group.notifications.len() {
            let view = RowView::new(&self.manager, self.interactive);
            self.rows.append(&view.container);
            self.row_views.push(view);
        }
        for (view, notification) in self.row_views.iter().zip(&group.notifications) {
            view.update(notification);
        }
        while self.row_views.len() > group.notifications.len() {
            let view = self.row_views.pop().expect("row view count exceeds group");
            self.rows.remove(&view.container);
        }

        // Recycled views adopt snapshot state immediately; only direct toggles animate.
        self.revealer.set_transition_duration(0);
        self.revealer.set_reveal_child(!is_collapsed);
        self.revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);
        self.disclosure
            .set_text(if is_collapsed { "▸" } else { "▾" });
    }
}

struct RowView {
    container: gtk::Button,
    picture: gtk::Image,
    summary: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    progress: gtk::ProgressBar,
    target: Rc<Cell<(u32, bool)>>,
}

impl RowView {
    fn new(manager: &std::rc::Weak<Manager>, interactive: bool) -> Self {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();

        let summary = gtk::Label::builder()
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("notification-summary");
        let time = gtk::Label::new(None);
        time.add_css_class("notification-time");
        let header = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        header.append(&summary);
        header.append(&time);
        text.append(&header);

        let body = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        body.add_css_class("notification-body");
        text.append(&body);
        let progress = progress_bar(0);
        text.append(&progress);

        let picture = gtk::Image::builder()
            .pixel_size(48)
            .width_request(48)
            .height_request(48)
            .valign(gtk::Align::Start)
            .build();
        picture.set_overflow(gtk::Overflow::Hidden);
        picture.add_css_class("notification-picture");
        let content = gtk::Box::builder()
            .spacing(10)
            .valign(gtk::Align::Start)
            .build();
        content.append(&picture);
        content.append(&text);

        let container = gtk::Button::builder().child(&content).build();
        container.add_css_class("notification-row");

        let target = Rc::new(Cell::new((0, false)));
        if interactive {
            let pressed_id = Rc::new(Cell::new(None));
            container.connect_state_flags_changed({
                let target = Rc::clone(&target);
                let pressed_id = Rc::clone(&pressed_id);
                move |_, flags| {
                    if flags.contains(gtk::StateFlags::ACTIVE) {
                        pressed_id.set(Some(target.get().0));
                    }
                }
            });
            container.connect_clicked({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                let pressed_id = Rc::clone(&pressed_id);
                move |_| {
                    if let Some(manager) = manager.upgrade() {
                        let id = pressed_id.take().unwrap_or_else(|| target.get().0);
                        manager.invoke_default(id);
                    }
                }
            });
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            let pressed_target = Rc::new(Cell::new(None));
            dismiss.connect_pressed({
                let target = Rc::clone(&target);
                let pressed_target = Rc::clone(&pressed_target);
                move |_, _, _, _| pressed_target.set(Some(target.get()))
            });
            dismiss.connect_released({
                let container = container.clone();
                let manager = manager.clone();
                let target = Rc::clone(&target);
                let pressed_target = Rc::clone(&pressed_target);
                move |_, _, x, y| {
                    let (id, active) = pressed_target.take().unwrap_or_else(|| target.get());
                    if container.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.dismiss(id, active);
                    }
                }
            });
            container.add_controller(dismiss);
        }

        Self {
            container,
            picture,
            summary,
            time,
            body,
            progress,
            target,
        }
    }

    fn update(&self, notification: &Notification) {
        self.target.set((notification.id, notification.active));
        if notification.urgency.as_deref() == Some("critical") {
            self.container.add_css_class("critical");
        } else {
            self.container.remove_css_class("critical");
        }
        self.summary.set_label(&notification.summary);

        self.picture.clear();
        self.picture
            .set_visible(set_picture(&self.picture, notification));

        let time = notification_time(notification.received_at);
        self.time.set_label(time.as_deref().unwrap_or_default());
        self.time.set_visible(time.is_some());

        let body = notification.body.trim();
        self.body.set_label(&notification.body);
        self.body.set_visible(!body.is_empty());

        self.progress
            .set_fraction(f64::from(notification.progress.unwrap_or_default()) / 100.0);
        self.progress.set_visible(notification.progress.is_some());
    }
}
