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
const TEXT_OVERFLOW_TOLERANCE: i32 = 8;
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
    expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
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
        let expanded_rows = Rc::new(RefCell::new(HashSet::new()));
        let scroll = gtk::ScrolledWindow::builder()
            .child(&groups)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(MAX_CONTENT_HEIGHT)
            .propagate_natural_height(true)
            .build();
        scroll.vscrollbar().set_can_target(false);
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
            expanded_rows,
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
        self.expanded_rows.borrow_mut().retain(|identity| {
            snapshot.groups.iter().any(|group| {
                group
                    .notifications
                    .iter()
                    .any(|notification| (notification.id, notification.revision) == *identity)
            })
        });

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
                &self.expanded_rows,
                &self.popover,
                &self.manager,
                self.interactive,
            );
            self.groups.append(&view.container);
            views.push(view);
        }
        while views.len() > groups.len() {
            let view = views
                .pop()
                .expect("group view count is greater than the group count");
            self.groups.remove(&view.container);
        }
        for (view, group) in views.iter_mut().zip(groups) {
            view.update(group);
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
        for group in self.group_views.borrow().iter() {
            group.clear_hover();
        }
        self.popover.popup();
    }

    pub fn hide(&self) {
        for group in self.group_views.borrow().iter() {
            group.clear_hover();
        }
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

fn is_child_control(
    picked: &gtk::Widget,
    surface: &gtk::Widget,
    primary: &gtk::Widget,
    actions: &gtk::Widget,
) -> bool {
    let mut current = Some(picked.clone());
    while let Some(widget) = current {
        if widget == *primary || widget == *surface {
            return false;
        }
        if widget == *actions || widget.is::<gtk::Button>() {
            return true;
        }
        current = widget.parent();
    }
    false
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

fn text_needs_expansion(summary: &gtk::Label, body: &gtk::Label) -> bool {
    let summary_layout = summary.create_pango_layout(Some(&summary.text()));
    let (summary_width, _) = summary_layout.pixel_size();
    if summary_width > summary.width() + TEXT_OVERFLOW_TOLERANCE {
        return true;
    }
    if !body.is_visible() || body.width() <= 0 {
        return false;
    }

    let body_layout = body.create_pango_layout(Some(&body.text()));
    body_layout.set_width(body.width() * gtk::pango::SCALE);
    body_layout.set_wrap(gtk::pango::WrapMode::WordChar);
    body_layout.line_count() > 3
}

fn update_notification_hover(
    surface: &gtk::Box,
    primary: &gtk::Button,
    actions: &gtk::FlowBox,
    enabled: &Rc<Cell<bool>>,
    x: f64,
    y: f64,
) {
    let active = enabled.get()
        && surface
            .pick(x, y, gtk::PickFlags::DEFAULT)
            .is_some_and(|picked| {
                !is_child_control(
                    &picked,
                    surface.as_ref(),
                    primary.as_ref(),
                    actions.as_ref(),
                )
            });
    if active {
        surface.add_css_class("content-hover");
    } else {
        surface.remove_css_class("content-hover");
    }
}

fn highlight_notification_on_hover(
    surface: &gtk::Box,
    primary: &gtk::Button,
    actions: &gtk::FlowBox,
    enabled: &Rc<Cell<bool>>,
) {
    let hover = gtk::EventControllerMotion::new();
    hover.set_propagation_phase(gtk::PropagationPhase::Capture);
    hover.connect_enter({
        let surface = surface.clone();
        let primary = primary.clone();
        let actions = actions.clone();
        let enabled = Rc::clone(enabled);
        move |_, x, y| update_notification_hover(&surface, &primary, &actions, &enabled, x, y)
    });
    hover.connect_motion({
        let surface = surface.clone();
        let primary = primary.clone();
        let actions = actions.clone();
        let enabled = Rc::clone(enabled);
        move |_, x, y| update_notification_hover(&surface, &primary, &actions, &enabled, x, y)
    });
    hover.connect_leave({
        let surface = surface.clone();
        move |_| surface.remove_css_class("content-hover")
    });
    surface.add_controller(hover);
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
    key: Rc<RefCell<String>>,
    notifications: Rc<RefCell<Vec<u32>>>,
    collapsed: Rc<RefCell<HashSet<String>>>,
    expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
    manager: std::rc::Weak<Manager>,
    popover: gtk::Popover,
    interactive: bool,
}

impl GroupView {
    fn new(
        collapsed: &Rc<RefCell<HashSet<String>>>,
        expanded_rows: &Rc<RefCell<HashSet<NotificationIdentity>>>,
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
        let key = Rc::new(RefCell::new(String::new()));
        disclosure.connect_clicked({
            let collapsed = Rc::clone(collapsed);
            let key = Rc::clone(&key);
            let revealer = revealer.clone();
            let disclosure = disclosure.clone();
            let popover = popover.clone();
            move |_| {
                let reveal = !revealer.reveals_child();
                revealer.set_reveal_child(reveal);
                disclosure.set_label(if reveal { "▾" } else { "▸" });
                let key = key.borrow();
                if reveal {
                    collapsed.borrow_mut().remove(key.as_str());
                } else {
                    collapsed.borrow_mut().insert(key.clone());
                }
                resize_during_reveal(&popover, &revealer);
            }
        });
        if interactive {
            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_released({
                let notifications = Rc::clone(&notifications);
                let manager = manager.clone();
                let header = header.clone();
                move |_, _, x, y| {
                    if header.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
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
            expanded_rows: Rc::clone(expanded_rows),
            manager: manager.clone(),
            popover: popover.clone(),
            interactive,
        }
    }

    fn update(&mut self, group: &Group) {
        self.key.replace(group.key.clone());
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

        let collapsible = group.notifications.len() > 1;
        self.disclosure.set_visible(collapsible);
        if !collapsible {
            self.collapsed.borrow_mut().remove(&group.key);
        }
        let is_collapsed = collapsible && self.collapsed.borrow().contains(&group.key);

        while self.row_views.len() < group.notifications.len() {
            let view = RowView::new(
                &self.expanded_rows,
                &self.manager,
                &self.popover,
                self.interactive,
            );
            self.rows.append(&view.container);
            self.row_views.push(view);
        }
        while self.row_views.len() > group.notifications.len() {
            let view = self
                .row_views
                .pop()
                .expect("row count is greater than the notification count");
            self.rows.remove(&view.container);
        }
        for (view, notification) in self.row_views.iter().zip(&group.notifications) {
            view.update(notification);
        }

        // Snapshot updates adopt collapsed state immediately; only direct toggles animate.
        self.revealer.set_transition_duration(0);
        self.revealer.set_reveal_child(!is_collapsed);
        self.revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);
        self.disclosure
            .set_label(if is_collapsed { "▸" } else { "▾" });
    }

    fn clear_hover(&self) {
        for row in &self.row_views {
            row.surface.remove_css_class("content-hover");
        }
    }
}

struct RowView {
    container: gtk::Box,
    surface: gtk::Box,
    primary: gtk::Button,
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
    identity: Rc<Cell<NotificationIdentity>>,
    expanded: Rc<Cell<bool>>,
    expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
    manager: std::rc::Weak<Manager>,
    popover: gtk::Popover,
    interactive: bool,
}

impl RowView {
    fn new(
        expanded_rows: &Rc<RefCell<HashSet<NotificationIdentity>>>,
        manager: &std::rc::Weak<Manager>,
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
        let primary = gtk::Button::builder().child(&content).build();
        primary.add_css_class("notification-primary");

        let expand_icon = gtk::Label::new(Some("▾"));
        let expand = gtk::Button::builder().child(&expand_icon).build();
        expand.set_cursor_from_name(Some("pointer"));
        expand.set_halign(gtk::Align::End);
        expand.set_valign(gtk::Align::Start);
        expand.set_visible(false);
        expand.add_css_class("notification-expand");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&primary));
        overlay.add_overlay(&expand);

        let actions = gtk::FlowBox::builder()
            .halign(gtk::Align::End)
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(2)
            .row_spacing(2)
            .max_children_per_line(3)
            .build();
        actions.set_cursor_from_name(Some("default"));
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
        highlight_notification_on_hover(&surface, &primary, &actions, &has_default);
        let identity = Rc::new(Cell::new((0, 0)));
        let expanded = Rc::new(Cell::new(false));
        expand.connect_clicked({
            let summary = summary.clone();
            let body = body.clone();
            let expand_icon = expand_icon.clone();
            let actions_revealer = actions_revealer.clone();
            let identity = Rc::clone(&identity);
            let expanded = Rc::clone(&expanded);
            let expanded_rows = Rc::clone(expanded_rows);
            let popover = popover.clone();
            move |_| {
                let value = !expanded.get();
                expanded.set(value);
                if value {
                    expanded_rows.borrow_mut().insert(identity.get());
                } else {
                    expanded_rows.borrow_mut().remove(&identity.get());
                }
                set_row_expanded(&summary, &body, &expand_icon, &actions_revealer, value);
                resize_during_reveal(&popover, &actions_revealer);
            }
        });
        if interactive {
            primary.connect_clicked({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                let has_default = Rc::clone(&has_default);
                move |button| {
                    if has_default.get()
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.invoke_action(target.get(), "default", activation_token(button));
                        manager.close();
                    }
                }
            });
            let pending_surface = Rc::new(Cell::new(false));
            let activate_surface = gtk::GestureClick::new();
            activate_surface.set_button(1);
            activate_surface.set_propagation_phase(gtk::PropagationPhase::Capture);
            activate_surface.connect_pressed({
                let surface = surface.clone();
                let primary = primary.clone();
                let actions = actions.clone();
                let has_default = Rc::clone(&has_default);
                let pending_surface = Rc::clone(&pending_surface);
                move |_, _, x, y| {
                    let active = has_default.get()
                        && surface
                            .pick(x, y, gtk::PickFlags::DEFAULT)
                            .is_some_and(|picked| {
                                picked != primary
                                    && !picked.is_ancestor(&primary)
                                    && !is_child_control(
                                        &picked,
                                        surface.as_ref(),
                                        primary.as_ref(),
                                        actions.as_ref(),
                                    )
                            });
                    pending_surface.set(active);
                }
            });
            activate_surface.connect_released({
                let surface = surface.clone();
                let primary = primary.clone();
                let actions = actions.clone();
                let manager = manager.clone();
                let target = Rc::clone(&target);
                move |gesture, _, x, y| {
                    if !pending_surface.take() {
                        return;
                    }
                    let active =
                        surface
                            .pick(x, y, gtk::PickFlags::DEFAULT)
                            .is_some_and(|picked| {
                                picked != primary
                                    && !picked.is_ancestor(&primary)
                                    && !is_child_control(
                                        &picked,
                                        surface.as_ref(),
                                        primary.as_ref(),
                                        actions.as_ref(),
                                    )
                            });
                    if active && let Some(manager) = manager.upgrade() {
                        manager.invoke_action(
                            target.get(),
                            "default",
                            gesture.widget().as_ref().and_then(activation_token),
                        );
                        manager.close();
                    }
                }
            });
            surface.add_controller(activate_surface);

            let dismiss = gtk::GestureClick::new();
            dismiss.set_button(3);
            dismiss.connect_released({
                let manager = manager.clone();
                let target = Rc::clone(&target);
                let container = container.clone();
                move |_, _, x, y| {
                    if container.contains(x, y)
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.dismiss(target.get());
                    }
                }
            });
            container.add_controller(dismiss);
        }

        Self {
            container,
            surface,
            primary,
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
            expanded_rows: Rc::clone(expanded_rows),
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

        self.surface.remove_css_class("content-hover");
        self.expanded
            .set(self.expanded_rows.borrow().contains(&identity));
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
        let actionable = self.interactive && has_default;
        if self.has_default.replace(actionable) != actionable {
            self.primary.set_can_target(actionable);
            self.primary.set_focusable(actionable);
            self.surface
                .set_cursor_from_name(actionable.then_some("pointer"));
            self.primary
                .set_cursor_from_name(actionable.then_some("pointer"));
        }

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
            button.set_cursor_from_name(self.interactive.then_some("pointer"));
            button.add_css_class("notification-action");
            button.set_sensitive(self.interactive);
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
        let current_identity = Rc::clone(&self.identity);
        let layout_ready = Cell::new(false);
        self.summary.add_tick_callback(move |summary, _| {
            if current_identity.get() != identity {
                return glib::ControlFlow::Break;
            }
            if !layout_ready.replace(true) {
                return glib::ControlFlow::Continue;
            }
            let value = has_named_actions || text_needs_expansion(summary, &body);
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
        self.actions_revealer.set_transition_duration(0);
        set_row_expanded(
            &self.summary,
            &self.body,
            &self.expand_icon,
            &self.actions_revealer,
            expanded,
        );
        self.actions_revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);

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
