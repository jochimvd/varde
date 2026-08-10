use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

use super::{
    super::{
        Manager,
        model::{Notification, Snapshot},
    },
    common::{application, progress_bar, set_picture},
};

const POPUP_NAME: &str = "varde-notification-popups";
const POPUP_WIDTH: i32 = 400;
const POPUP_RIGHT: i32 = 20;
const POPUP_TOP: i32 = 50;
const POPUP_PICTURE_SIZE: i32 = 54;
pub(super) const MAX_POPUPS: usize = 5;

pub(in crate::notifications) struct Popups {
    window: gtk::ApplicationWindow,
    list: gtk::Box,
    manager: std::rc::Weak<Manager>,
    state: RefCell<PopupState>,
}

#[derive(Default)]
pub(super) struct PopupState {
    pub(super) displayed: HashMap<u32, u64>,
    pub(super) visible: HashSet<u32>,
}

impl PopupState {
    pub(super) fn update(&mut self, snapshot: &Snapshot, blocked: bool) -> Vec<(u32, u64)> {
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
        let mut active = snapshot
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
        active.sort_unstable_by_key(|notification| notification.revision);
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
        window.set_margin(gtk4_layer_shell::Edge::Right, POPUP_RIGHT);
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
    notification: &Notification,
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
    picture_widget.set_overflow(gtk::Overflow::Hidden);
    picture_widget.add_css_class("notification-picture");
    picture_widget.add_css_class("notification-popup-picture");
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
    if notification
        .actions
        .iter()
        .any(|action| action.key == "default")
    {
        card.set_cursor_from_name(Some("pointer"));
    }
    card.add_css_class("notification-popup");
    if notification.urgency.as_deref() == Some("critical") {
        card.add_css_class("critical");
    }
    card.connect_clicked({
        let manager = manager.clone();
        let id = notification.id;
        move |_| {
            if let Some(manager) = manager.upgrade() {
                manager.invoke_action(id, "default");
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
