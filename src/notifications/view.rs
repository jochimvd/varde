use std::{
    cell::{Cell, RefCell},
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
) -> gtk::Button {
    let card_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();

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
    card_content.append(&header);

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
    card_content.append(&content);

    let card = gtk::Button::builder().child(&card_content).build();
    card.add_css_class("notification-popup");
    if notification.urgency.as_deref() == Some("critical") {
        card.add_css_class("critical");
    }
    card.connect_clicked({
        let manager = manager.clone();
        let id = notification.id;
        move |_| {
            if let Some(manager) = manager.upgrade() {
                manager.invoke_default(id);
            }
        }
    });

    let dismiss = gtk::GestureClick::new();
    dismiss.set_button(3);
    dismiss.connect_released({
        let card = card.clone();
        let manager = manager.clone();
        let id = notification.id;
        move |_, _, x, y| {
            if card.contains(x, y)
                && let Some(manager) = manager.upgrade()
            {
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

fn update_group_order(order: &mut Vec<String>, keys: &[String], reset: bool) {
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
        image.add_css_class("notification-group-icon");
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
        header.append(&image);
        header.append(&name);
        header.append(&count);
        header.append(&disclosure);

        let rows = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_duration(150)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&rows)
            .build();

        let key = Rc::new(RefCell::new(String::new()));
        let notifications = Rc::new(RefCell::new(Vec::new()));
        let toggle = gtk::Button::builder()
            .focusable(false)
            .hexpand(true)
            .child(&header)
            .build();
        toggle.add_css_class("notification-group-toggle");
        toggle.connect_clicked({
            let collapsed = Rc::clone(collapsed);
            let key = Rc::clone(&key);
            let revealer = revealer.clone();
            let disclosure = disclosure.clone();
            let popover = popover.clone();
            move |_| {
                let reveal = !revealer.reveals_child();
                revealer.set_reveal_child(reveal);
                disclosure.set_text(if reveal { "▾" } else { "▸" });
                let key = key.borrow().clone();
                if reveal {
                    collapsed.borrow_mut().remove(&key);
                } else {
                    collapsed.borrow_mut().insert(key);
                }
                glib::timeout_add_local_once(Duration::from_millis(160), {
                    let popover = popover.clone();
                    move || popover.queue_resize()
                });
            }
        });
        if interactive {
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_released({
                let toggle = toggle.clone();
                let manager = manager.clone();
                let notifications = Rc::clone(&notifications);
                move |_, _, x, y| {
                    if toggle.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.dismiss_group(notifications.borrow().clone());
                    }
                }
            });
            toggle.add_controller(dismiss);
        }

        container.append(&toggle);
        container.append(&revealer);
        Self {
            container,
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

    fn update(&mut self, group: &super::model::Group) {
        let (name, icon) = application(group);
        self.image.clear();
        if let Some(icon) = icon {
            self.image.set_from_gicon(&icon);
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

        self.revealer.set_reveal_child(!is_collapsed);
        self.disclosure
            .set_text(if is_collapsed { "▸" } else { "▾" });
    }
}

struct RowView {
    container: gtk::Button,
    summary: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    progress: gtk::ProgressBar,
    target: Rc<Cell<(u32, bool)>>,
}

impl RowView {
    fn new(manager: &std::rc::Weak<Manager>, interactive: bool) -> Self {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
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
        content.append(&header);

        let body = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        body.add_css_class("notification-body");
        content.append(&body);
        let progress = progress_bar(0);
        content.append(&progress);

        let container = gtk::Button::builder().child(&content).build();
        container.add_css_class("notification-row");

        let target = Rc::new(Cell::new((0, false)));
        if interactive {
            container.connect_clicked({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                move |_| {
                    if let Some(manager) = manager.upgrade() {
                        manager.invoke_default(target.get().0);
                    }
                }
            });
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_released({
                let container = container.clone();
                let manager = manager.clone();
                let target = Rc::clone(&target);
                move |_, _, x, y| {
                    if container.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
                        let (id, active) = target.get();
                        manager.dismiss(id, active);
                    }
                }
            });
            container.add_controller(dismiss);
        }

        Self {
            container,
            summary,
            time,
            body,
            progress,
            target,
        }
    }

    fn update(&self, notification: &super::model::Notification) {
        self.target.set((notification.id, notification.active));
        if notification.urgency.as_deref() == Some("critical") {
            self.container.add_css_class("critical");
        } else {
            self.container.remove_css_class("critical");
        }
        self.summary.set_label(&notification.summary);

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
    fn group_order_stays_stable_until_reset() {
        let strings = |keys: &[&str]| {
            keys.iter()
                .map(|key| (*key).to_string())
                .collect::<Vec<_>>()
        };
        let mut order = strings(&["chat", "mail"]);

        update_group_order(&mut order, &strings(&["mail", "chat"]), false);
        assert_eq!(order, strings(&["chat", "mail"]));

        update_group_order(&mut order, &strings(&["news", "mail", "chat"]), false);
        assert_eq!(order, strings(&["chat", "mail", "news"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), false);
        assert_eq!(order, strings(&["mail", "news"]));

        update_group_order(&mut order, &strings(&["news", "mail"]), true);
        assert_eq!(order, strings(&["news", "mail"]));
    }

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
