use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

use gtk::{glib, prelude::*};
use gtk4_layer_shell::LayerShell;

use super::{
    super::{
        Manager,
        model::{Notification, Snapshot},
        state::{POPUP_TIMEOUT, PopupTimer, Urgency},
    },
    center::{CHEVRON_DOWN, CHEVRON_UP, REVEAL_DURATION},
    common::{activation_token, application, progress_bar, set_picture},
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
    hovered: Rc<Cell<bool>>,
    /// The (id, revision) of each popup on screen, in order.
    rendered: RefCell<Vec<(u32, u64)>>,
    timers: RefCell<HashMap<u32, Rc<Cell<Option<PopupTimer>>>>>,
}

#[derive(Default)]
pub(super) struct PopupState {
    pub(super) displayed: HashMap<u32, u64>,
    pub(super) visible: HashSet<u32>,
}

impl PopupState {
    pub(super) fn update(&mut self, snapshot: &Snapshot, blocked: bool) -> Vec<(u32, u64)> {
        let popup_ids = snapshot
            .groups
            .iter()
            .flat_map(|group| &group.notifications)
            .filter(|notification| notification.show_popup)
            .map(|notification| notification.id)
            .collect::<HashSet<_>>();
        self.displayed.retain(|id, _| popup_ids.contains(id));
        self.visible.retain(|id| popup_ids.contains(id));
        if blocked {
            self.visible.clear();
            return Vec::new();
        }
        let mut pending = snapshot
            .groups
            .iter()
            .flat_map(|group| &group.notifications)
            .filter(|notification| notification.show_popup)
            .collect::<Vec<_>>();
        let mut displayed = Vec::new();
        for notification in &pending {
            if self.visible.contains(&notification.id)
                && self.displayed.get(&notification.id) != Some(&notification.revision)
            {
                self.displayed
                    .insert(notification.id, notification.revision);
                displayed.push((notification.id, notification.revision));
            }
        }
        pending.sort_unstable_by_key(|notification| notification.revision);
        let mut available = MAX_POPUPS.saturating_sub(self.visible.len());
        for notification in pending {
            let displayed_this_popup = self
                .displayed
                .get(&notification.id)
                .is_some_and(|revision| *revision >= notification.popup_revision);
            if available == 0 || displayed_this_popup {
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

        // Hovering holds every popup on screen, as reading one usually means
        // reading the others.
        let hovered = Rc::new(Cell::new(false));
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let manager = Rc::downgrade(manager);
            let hovered = hovered.clone();
            move |_, _, _| {
                if !hovered.replace(true)
                    && let Some(manager) = manager.upgrade()
                {
                    manager.pause_popups(true);
                }
            }
        });
        motion.connect_leave({
            let manager = Rc::downgrade(manager);
            let hovered = hovered.clone();
            move |_| {
                if hovered.replace(false)
                    && let Some(manager) = manager.upgrade()
                {
                    manager.pause_popups(false);
                }
            }
        });
        window.add_controller(motion);

        Self {
            window,
            list,
            manager: Rc::downgrade(manager),
            state: RefCell::new(PopupState::default()),
            hovered,
            rendered: RefCell::default(),
            timers: RefCell::default(),
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

        let popups = snapshot
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .notifications
                    .iter()
                    .filter(|notification| state.visible.contains(&notification.id))
                    .map(move |notification| (group, notification))
            })
            .take(MAX_POPUPS)
            .collect::<Vec<_>>();
        let keys = popups
            .iter()
            .map(|(_, notification)| (notification.id, notification.revision))
            .collect::<Vec<_>>();

        // Rebuilding only when the popups change keeps the widgets under the
        // pointer alive while timers start, pause, and resume.
        let mut timers = self.timers.borrow_mut();
        if *self.rendered.borrow() == keys {
            for (_, notification) in &popups {
                if let Some(timer) = timers.get(&notification.id) {
                    timer.set(notification.popup_timer);
                }
            }
        } else {
            while let Some(child) = self.list.first_child() {
                self.list.remove(&child);
            }
            timers.clear();
            for (group, notification) in &popups {
                let (app_name, app_icon) = application(group);
                let timer = Rc::new(Cell::new(notification.popup_timer));
                self.list.append(&popup_widget(
                    &app_name,
                    app_icon.as_ref(),
                    notification,
                    &timer,
                    &self.manager,
                ));
                timers.insert(notification.id, timer);
            }
            self.rendered.replace(keys);
        }
        let visible = !popups.is_empty() && !snapshot.dnd && !center_open;
        self.window.set_visible(visible);
        if !visible {
            self.resume();
        }
    }

    pub fn hide(&self) {
        self.state.borrow_mut().visible.clear();
        self.window.set_visible(false);
        self.resume();
    }

    // A hidden window may never report the pointer leaving it.
    fn resume(&self) {
        if self.hovered.replace(false)
            && let Some(manager) = self.manager.upgrade()
        {
            manager.pause_popups(false);
        }
    }
}

fn popup_widget(
    app_name: &str,
    app_icon: Option<&gio::Icon>,
    notification: &Notification,
    timer: &Rc<Cell<Option<PopupTimer>>>,
    manager: &std::rc::Weak<Manager>,
) -> gtk::Box {
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

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .overflow(gtk::Overflow::Hidden)
        .build();
    card.append(&card_content);
    if let Some(actions) = action_buttons(notification, manager) {
        // Collapsed actions keep popups short; the chevron says they exist.
        let chevron = gtk::Label::new(Some(CHEVRON_DOWN));
        chevron.add_css_class("notification-popup-expand");
        header.append(&chevron);
        let revealer = gtk::Revealer::builder()
            .transition_duration(REVEAL_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&actions)
            .build();
        card.append(&revealer);
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter({
            let revealer = revealer.clone();
            let chevron = chevron.clone();
            move |_, _, _| {
                revealer.set_reveal_child(true);
                chevron.set_label(CHEVRON_UP);
            }
        });
        hover.connect_leave(move |_| {
            revealer.set_reveal_child(false);
            chevron.set_label(CHEVRON_DOWN);
        });
        card.add_controller(hover);
    }
    if notification.urgency != Urgency::Critical {
        card.append(&timer_bar(timer.clone()));
    }
    card.add_css_class("notification-popup");
    if notification.urgency == Urgency::Critical {
        card.add_css_class("critical");
    }

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
                manager.dismiss(id);
            }
        }
    });
    card.add_controller(dismiss);

    // The whole card is the default action's target. Action buttons claim
    // their own presses, so this gesture never sees them.
    let has_default = notification
        .actions
        .iter()
        .any(|action| action.key == "default");
    if has_default {
        card.set_cursor_from_name(Some("pointer"));
        let activate = gtk::GestureClick::new();
        activate.set_button(1);
        activate.connect_released({
            let card = card.clone();
            let manager = manager.clone();
            let id = notification.id;
            move |_, _, x, y| {
                if card.contains(x, y)
                    && let Some(manager) = manager.upgrade()
                {
                    manager.invoke_action(id, "default", activation_token(&card));
                }
            }
        });
        card.add_controller(activate);
    }
    card
}

/// Rows of plain boxes rather than a flow box, which claims clicks on the
/// space around its children and would take them from the card.
fn action_buttons(
    notification: &Notification,
    manager: &std::rc::Weak<Manager>,
) -> Option<gtk::Box> {
    const PER_ROW: usize = 3;
    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    actions.add_css_class("notification-popup-actions");
    let named = notification
        .actions
        .iter()
        .filter(|action| action.key != "default" && !action.label.trim().is_empty())
        .collect::<Vec<_>>();
    for (index, action) in named.iter().enumerate() {
        if index % PER_ROW == 0 {
            actions.append(&gtk::Box::builder().spacing(4).build());
        }
        let label = gtk::Label::builder()
            .label(&action.label)
            .max_width_chars(24)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let button = gtk::Button::builder().child(&label).build();
        button.set_cursor_from_name(Some("pointer"));
        button.add_css_class("notification-action");
        button.connect_clicked({
            let manager = manager.clone();
            let key = action.key.clone();
            let id = notification.id;
            move |button| {
                if let Some(manager) = manager.upgrade() {
                    manager.invoke_action(id, &key, activation_token(button));
                }
            }
        });
        actions
            .last_child()
            .and_downcast::<gtk::Box>()
            .expect("a row was just appended")
            .append(&button);
    }
    (!named.is_empty()).then_some(actions)
}

/// A popup's timer starts once it is displayed; until then the bar is full.
fn timer_bar(timer: Rc<Cell<Option<PopupTimer>>>) -> gtk::ProgressBar {
    let fraction = move || {
        timer.get().map_or(1.0, |timer| {
            timer.remaining(Instant::now()).as_secs_f64() / POPUP_TIMEOUT.as_secs_f64()
        })
    };
    let bar = gtk::ProgressBar::builder().fraction(fraction()).build();
    bar.add_css_class("notification-popup-timer");
    bar.add_tick_callback(move |bar, _| {
        bar.set_fraction(fraction());
        glib::ControlFlow::Continue
    });
    bar
}
