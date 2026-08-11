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
        state::Urgency,
    },
    common::{
        activation_token, application, message, notification_time, progress_bar, set_picture,
    },
};

const PANEL_WIDTH: i32 = 460;
const PANEL_RIGHT: i32 = 20;
const PANEL_TOP: i32 = 18;
const MAX_CONTENT_HEIGHT: i32 = 520;
const GROUP_TRANSITION_DURATION: u32 = 150;
const EXPAND_BUTTON_WIDTH: i32 = 30;
type NotificationIdentity = (u32, u64);

pub(in crate::notifications) struct Center {
    popover: gtk::Popover,
    groups: gtk::Box,
    group_views: RefCell<Vec<GroupView>>,
    group_order: RefCell<Vec<String>>,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    collapsed: Rc<RefCell<HashSet<String>>>,
    close_on_release: Rc<Cell<bool>>,
    manager: std::rc::Weak<Manager>,
    interactive: bool,
}

impl Center {
    pub fn new(anchor: &gtk::ApplicationWindow, manager: &Rc<Manager>, interactive: bool) -> Self {
        let close_on_release = Rc::new(Cell::new(false));
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
            let close_on_release = Rc::clone(&close_on_release);
            move |_| {
                close_on_release.set(false);
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
        dnd.set_cursor_from_name(Some("pointer"));
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
        clear.set_cursor_from_name(Some("pointer"));
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
        let pointer = gtk::EventControllerLegacy::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        pointer.connect_event({
            let manager = Rc::downgrade(manager);
            let close_on_release = Rc::clone(&close_on_release);
            move |_, event| {
                if event.event_type() == gdk::EventType::ButtonRelease
                    && event
                        .downcast_ref::<gdk::ButtonEvent>()
                        .is_some_and(|event| event.button() == 1)
                    && close_on_release.replace(false)
                    && let Some(manager) = manager.upgrade()
                {
                    manager.close();
                }
                glib::Propagation::Proceed
            }
        });
        popover.add_controller(pointer);

        Self {
            popover,
            groups,
            group_views: RefCell::new(Vec::new()),
            group_order: RefCell::new(Vec::new()),
            stack,
            dnd,
            clear,
            collapsed,
            close_on_release,
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
        let mut available = std::mem::take(&mut *views);
        for group in groups {
            let mut view = available
                .iter()
                .position(|view| view.key == group.key)
                .map(|index| available.remove(index))
                .unwrap_or_else(|| {
                    let view = GroupView::new(
                        group.key.clone(),
                        &self.collapsed,
                        &self.close_on_release,
                        &self.popover,
                        &self.manager,
                        self.interactive,
                    );
                    self.groups.append(&view.container);
                    view
                });
            view.update(group);
            views.push(view);
        }
        for view in available {
            self.groups.remove(&view.container);
        }
        for (index, view) in views.iter().enumerate() {
            let previous = index
                .checked_sub(1)
                .map(|previous| &views[previous].container);
            self.groups.reorder_child_after(&view.container, previous);
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

fn resize_during_reveal(popover: &gtk::Popover, revealer: &gtk::Revealer) {
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

fn highlight_on_hover(
    target: &impl IsA<gtk::Widget>,
    surface: &gtk::Box,
    enabled: &Rc<Cell<bool>>,
) {
    let hover = gtk::EventControllerMotion::new();
    hover.connect_enter({
        let surface = surface.clone();
        let enabled = Rc::clone(enabled);
        move |_, _, _| {
            if enabled.get() {
                surface.add_css_class("content-hover");
            }
        }
    });
    hover.connect_leave({
        let surface = surface.clone();
        move |_| surface.remove_css_class("content-hover")
    });
    target.add_controller(hover);
}

struct GroupView {
    container: gtk::Box,
    icon: gtk::Stack,
    image: gtk::Image,
    name: gtk::Label,
    count: gtk::Label,
    disclosure: gtk::Button,
    rows: gtk::Box,
    revealer: gtk::Revealer,
    row_views: Vec<RowView>,
    key: String,
    notifications: Rc<RefCell<Vec<u32>>>,
    collapsed: Rc<RefCell<HashSet<String>>>,
    close_on_release: Rc<Cell<bool>>,
    manager: std::rc::Weak<Manager>,
    popover: gtk::Popover,
    interactive: bool,
}

impl GroupView {
    fn new(
        key: String,
        collapsed: &Rc<RefCell<HashSet<String>>>,
        close_on_release: &Rc<Cell<bool>>,
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
        let disclosure = gtk::Button::builder().focusable(false).build();
        disclosure.set_cursor_from_name(Some("pointer"));
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

        let notifications = Rc::new(RefCell::new(Vec::new()));
        disclosure.connect_clicked({
            let collapsed = Rc::clone(collapsed);
            let key = key.clone();
            let revealer = revealer.clone();
            let disclosure = disclosure.clone();
            let popover = popover.clone();
            move |_| {
                let reveal = !revealer.reveals_child();
                revealer.set_reveal_child(reveal);
                disclosure.set_label(if reveal { "▾" } else { "▸" });
                if reveal {
                    collapsed.borrow_mut().remove(&key);
                } else {
                    collapsed.borrow_mut().insert(key.clone());
                }
                resize_during_reveal(&popover, &revealer);
            }
        });
        if interactive {
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_pressed({
                let notifications = Rc::clone(&notifications);
                let manager = manager.clone();
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        manager.dismiss_group(notifications.borrow().clone());
                    }
                }
            });
            header.add_controller(dismiss);
        }

        container.append(&header);
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
            close_on_release: Rc::clone(close_on_release),
            manager: manager.clone(),
            popover: popover.clone(),
            interactive,
        }
    }

    fn update(&mut self, group: &Group) {
        debug_assert_eq!(self.key, group.key);
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
        self.notifications.replace(
            group
                .notifications
                .iter()
                .map(|notification| notification.id)
                .collect(),
        );

        let is_collapsed = self.collapsed.borrow().contains(&group.key);

        let mut available = std::mem::take(&mut self.row_views);
        for notification in &group.notifications {
            let identity = (notification.id, notification.revision);
            let view = available
                .iter()
                .position(|view| view.identity.get() == identity)
                .map(|index| available.remove(index))
                .unwrap_or_else(|| {
                    let view = RowView::new(
                        &self.manager,
                        &self.close_on_release,
                        &self.popover,
                        self.interactive,
                    );
                    self.rows.append(&view.container);
                    view
                });
            view.update(notification);
            self.row_views.push(view);
        }
        for view in available {
            self.rows.remove(&view.container);
        }
        for (index, view) in self.row_views.iter().enumerate() {
            let previous = index
                .checked_sub(1)
                .map(|previous| &self.row_views[previous].container);
            self.rows.reorder_child_after(&view.container, previous);
        }

        // Snapshot updates adopt collapsed state immediately; only direct toggles animate.
        self.revealer.set_transition_duration(0);
        self.revealer.set_reveal_child(!is_collapsed);
        self.revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);
        self.disclosure
            .set_label(if is_collapsed { "▸" } else { "▾" });
    }
}

struct RowView {
    container: gtk::Box,
    surface: gtk::Box,
    picture: gtk::Image,
    summary: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    progress: gtk::ProgressBar,
    expand: gtk::Button,
    expand_icon: gtk::Label,
    expand_space: gtk::Box,
    actions: gtk::FlowBox,
    actions_revealer: gtk::Revealer,
    target: Rc<Cell<u32>>,
    has_default: Rc<Cell<bool>>,
    identity: Cell<NotificationIdentity>,
    expanded: Rc<Cell<bool>>,
    manager: std::rc::Weak<Manager>,
    popover: gtk::Popover,
    interactive: bool,
}

impl RowView {
    fn new(
        manager: &std::rc::Weak<Manager>,
        close_on_release: &Rc<Cell<bool>>,
        popover: &gtk::Popover,
        interactive: bool,
    ) -> Self {
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();

        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(1)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("notification-summary");
        let time = gtk::Label::new(None);
        time.add_css_class("notification-time");
        let header_spacer = gtk::Box::builder().hexpand(true).build();
        let expand_space = gtk::Box::builder()
            .width_request(EXPAND_BUTTON_WIDTH)
            .build();
        expand_space.set_visible(false);
        let header = gtk::Box::builder()
            .spacing(5)
            .valign(gtk::Align::Center)
            .build();
        header.append(&summary);
        header.append(&time);
        header.append(&header_spacer);
        header.append(&expand_space);
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

        content.add_css_class("notification-content");

        let expand_icon = gtk::Label::new(Some("▾"));
        let expand = gtk::Button::builder().child(&expand_icon).build();
        expand.set_cursor_from_name(Some("pointer"));
        expand.set_halign(gtk::Align::End);
        expand.set_valign(gtk::Align::Start);
        expand.set_visible(false);
        expand.add_css_class("notification-expand");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&content));
        overlay.add_overlay(&expand);

        let actions = gtk::FlowBox::builder()
            .halign(gtk::Align::End)
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(2)
            .row_spacing(2)
            .max_children_per_line(3)
            .build();
        actions.add_css_class("notification-actions");
        let actions_revealer = gtk::Revealer::builder()
            .transition_duration(GROUP_TRANSITION_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&actions)
            .build();

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-row");
        let surface = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        surface.add_css_class("notification-row-surface");
        surface.append(&overlay);
        surface.append(&actions_revealer);
        container.append(&surface);

        let target = Rc::new(Cell::new(0));
        let has_default = Rc::new(Cell::new(false));
        highlight_on_hover(&content, &surface, &has_default);
        highlight_on_hover(&actions_revealer, &surface, &has_default);
        let identity = Cell::new((0, 0));
        let expanded = Rc::new(Cell::new(false));
        expand.connect_clicked({
            let summary = summary.clone();
            let body = body.clone();
            let expand_icon = expand_icon.clone();
            let actions_revealer = actions_revealer.clone();
            let expanded = Rc::clone(&expanded);
            let popover = popover.clone();
            move |_| {
                let value = !expanded.get();
                expanded.set(value);
                set_row_expanded(&summary, &body, &expand_icon, &actions_revealer, value);
                resize_during_reveal(&popover, &actions_revealer);
            }
        });
        if interactive {
            let activate = gtk::GestureClick::new();
            activate.set_button(1);
            activate.connect_pressed({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                let has_default = Rc::clone(&has_default);
                let close_on_release = Rc::clone(close_on_release);
                move |gesture, _, _, _| {
                    if has_default.get()
                        && let Some(manager) = manager.upgrade()
                    {
                        close_on_release.set(true);
                        manager.invoke_action(
                            target.get(),
                            "default",
                            gesture.widget().as_ref().and_then(activation_token),
                        );
                    }
                }
            });
            content.add_controller(activate);
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_pressed({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        manager.dismiss(target.get());
                    }
                }
            });
            container.add_controller(dismiss);
        }

        Self {
            container,
            surface,
            picture,
            summary,
            time,
            body,
            progress,
            expand,
            expand_icon,
            expand_space,
            actions,
            actions_revealer,
            target,
            has_default,
            identity,
            expanded,
            manager: manager.clone(),
            popover: popover.clone(),
            interactive,
        }
    }

    fn update(&self, notification: &Notification) {
        let identity = (notification.id, notification.revision);
        self.target.set(notification.id);
        let changed = self.identity.replace(identity) != identity;
        let time = notification_time(notification.received_at);
        self.time.set_label(
            &time
                .as_deref()
                .map(|time| format!("· {time}"))
                .unwrap_or_default(),
        );
        self.time.set_visible(time.is_some());
        if !changed {
            return;
        }

        self.expanded.set(false);
        if notification.urgency == Urgency::Critical {
            self.container.add_css_class("critical");
        } else {
            self.container.remove_css_class("critical");
        }
        self.summary.set_label(&notification.summary);

        self.picture.clear();
        let has_picture = set_picture(&self.picture, notification);
        self.picture.set_visible(has_picture);

        let body = notification.body.trim();
        self.body.set_label(&notification.body);
        self.body.set_visible(!body.is_empty());

        let has_default = notification
            .actions
            .iter()
            .any(|action| action.key == "default");
        let actionable = self.interactive;
        self.has_default.set(actionable && has_default);
        self.surface
            .set_cursor_from_name((actionable && has_default).then_some("pointer"));

        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        let mut has_named_actions = false;
        for action in notification
            .actions
            .iter()
            .filter(|action| action.key != "default" && !action.label.trim().is_empty())
        {
            has_named_actions = true;
            let label = gtk::Label::builder()
                .label(&action.label)
                .max_width_chars(48)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let button = gtk::Button::builder().child(&label).build();
            button.set_cursor_from_name(Some("pointer"));
            button.add_css_class("notification-action");
            button.set_sensitive(actionable);
            button.connect_clicked({
                let manager = self.manager.clone();
                let key = action.key.clone();
                let id = notification.id;
                move |button| {
                    if let Some(manager) = manager.upgrade() {
                        manager.invoke_action(id, &key, activation_token(button));
                        manager.close();
                    }
                }
            });
            self.actions.append(&button);
        }

        self.expand.set_visible(has_named_actions);
        self.expand_space.set_visible(has_named_actions);
        let body = self.body.clone();
        let expand = self.expand.clone();
        let expand_space = self.expand_space.clone();
        let expanded = Rc::clone(&self.expanded);
        let expand_icon = self.expand_icon.clone();
        let actions_revealer = self.actions_revealer.clone();
        let popover = self.popover.clone();
        let layout_ready = Cell::new(false);
        self.summary.add_tick_callback(move |summary, _| {
            if !layout_ready.replace(true) {
                return glib::ControlFlow::Continue;
            }
            let value = has_named_actions
                || summary.layout().is_ellipsized()
                || (body.is_visible() && body.layout().is_ellipsized());
            expand.set_visible(value);
            expand_space.set_visible(value);
            let restored = value && expanded.get();
            actions_revealer.set_transition_duration(0);
            set_row_expanded(summary, &body, &expand_icon, &actions_revealer, restored);
            actions_revealer.set_transition_duration(GROUP_TRANSITION_DURATION);
            popover.queue_resize();
            popover.present();
            glib::ControlFlow::Break
        });

        let expanded = self.expand.is_visible() && self.expanded.get();
        set_row_expanded(
            &self.summary,
            &self.body,
            &self.expand_icon,
            &self.actions_revealer,
            expanded,
        );

        self.progress
            .set_fraction(f64::from(notification.progress.unwrap_or_default()) / 100.0);
        self.progress.set_visible(notification.progress.is_some());
    }
}

fn set_row_expanded(
    summary: &gtk::Label,
    body: &gtk::Label,
    expand_icon: &gtk::Label,
    actions: &gtk::Revealer,
    expanded: bool,
) {
    summary.set_wrap(expanded);
    summary.set_lines(if expanded { -1 } else { 1 });
    summary.set_ellipsize(if expanded {
        gtk::pango::EllipsizeMode::None
    } else {
        gtk::pango::EllipsizeMode::End
    });
    body.set_lines(if expanded { -1 } else { 3 });
    body.set_ellipsize(if expanded {
        gtk::pango::EllipsizeMode::None
    } else {
        gtk::pango::EllipsizeMode::End
    });
    expand_icon.set_label(if expanded { "▴" } else { "▾" });
    actions.set_reveal_child(expanded);
}
