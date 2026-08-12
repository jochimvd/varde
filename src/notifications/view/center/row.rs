use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
};

use gtk::{glib, graphene, prelude::*};

use crate::notifications::view::common::{
    activation_token, notification_time, progress_bar, set_picture,
};
use crate::notifications::{Manager, model::Notification, state::Urgency};

use super::{
    EXPAND_BUTTON_WIDTH, GROUP_TRANSITION_DURATION, NOTIFICATION_TEXT_WIDTH, NotificationIdentity,
    PICTURE_TEXT_OFFSET,
};

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

fn update_notification_hover(
    surface: &gtk::Box,
    primary: &gtk::Button,
    actions: &gtk::FlowBox,
    enabled: &Rc<Cell<bool>>,
    x: f64,
    y: f64,
) -> bool {
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
    active
}

fn pointer_position(widget: &gtk::Widget) -> Option<(f64, f64)> {
    let native = widget.native()?;
    let pointer = native.display().default_seat()?.pointer()?;
    let (pointer_surface, x, y) = pointer.surface_at_position();
    if pointer_surface? != native.surface()? {
        return None;
    }
    let (offset_x, offset_y) = native.surface_transform();
    let point = native.compute_point(
        widget,
        &graphene::Point::new((x + offset_x) as f32, (y + offset_y) as f32),
    )?;
    Some((f64::from(point.x()), f64::from(point.y())))
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
        move |_, x, y| {
            update_notification_hover(&surface, &primary, &actions, &enabled, x, y);
        }
    });
    hover.connect_motion({
        let surface = surface.clone();
        let primary = primary.clone();
        let actions = actions.clone();
        let enabled = Rc::clone(enabled);
        move |_, x, y| {
            update_notification_hover(&surface, &primary, &actions, &enabled, x, y);
        }
    });
    hover.connect_leave({
        let surface = surface.clone();
        move |_| surface.remove_css_class("content-hover")
    });
    surface.add_controller(hover);
}

pub(super) struct RowView {
    pub(super) container: gtk::Box,
    pub(super) surface: gtk::Box,
    primary: gtk::Button,
    text: gtk::Box,
    picture: gtk::Image,
    notification_text: NotificationText,
    expand: gtk::Button,
    expand_icon: gtk::Label,
    actions: gtk::FlowBox,
    actions_revealer: gtk::Revealer,
    pub(super) target: Rc<Cell<u32>>,
    has_default: Rc<Cell<bool>>,
    identity: Rc<Cell<NotificationIdentity>>,
    expanded: Rc<Cell<bool>>,
    expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
    manager: std::rc::Weak<Manager>,
    interactive: bool,
}

struct NotificationText {
    container: gtk::Box,
    summary: gtk::Label,
    time: gtk::Label,
    body: gtk::Label,
    body_expansion: BodyExpansion,
    progress: gtk::ProgressBar,
    expand_space: gtk::Box,
}

#[derive(Clone)]
struct BodyExpansion {
    body: gtk::Label,
    overflow: gtk::Label,
    revealer: gtk::Revealer,
    full: Rc<RefCell<String>>,
    split: Rc<RefCell<Option<(String, String)>>>,
}

impl BodyExpansion {
    fn set_expanded(&self, expanded: bool) {
        if expanded {
            if let Some((prefix, suffix)) = self.split.borrow().as_ref() {
                self.body.set_label(prefix);
                self.overflow.set_label(suffix);
                self.body.set_lines(-1);
                self.body.set_ellipsize(gtk::pango::EllipsizeMode::None);
                self.revealer.set_reveal_child(true);
            } else {
                self.body.set_label(&self.full.borrow());
                set_body_expanded(&self.body, true);
                self.revealer.set_reveal_child(false);
            }
        } else {
            self.revealer.set_reveal_child(false);
            if !self.revealer.is_child_revealed() {
                self.finish_collapsed();
            }
        }
    }

    fn finish_collapsed(&self) {
        self.body.set_label(&self.full.borrow());
        set_body_expanded(&self.body, false);
    }
}

impl NotificationText {
    fn new() -> Self {
        let container = gtk::Box::builder()
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
        let spacer = gtk::Box::builder().hexpand(true).build();
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
        header.append(&spacer);
        header.append(&expand_space);
        container.append(&header);

        let body = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        body.add_css_class("notification-body");
        container.append(&body);
        let body_overflow = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        body_overflow.add_css_class("notification-body");
        let body_revealer = gtk::Revealer::builder()
            .transition_duration(GROUP_TRANSITION_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&body_overflow)
            .build();
        container.append(&body_revealer);
        let body_expansion = BodyExpansion {
            body: body.clone(),
            overflow: body_overflow,
            revealer: body_revealer,
            full: Rc::new(RefCell::new(String::new())),
            split: Rc::new(RefCell::new(None)),
        };
        body_expansion.revealer.connect_child_revealed_notify({
            let body_expansion = body_expansion.clone();
            move |revealer| {
                if !revealer.reveals_child() && !revealer.is_child_revealed() {
                    body_expansion.finish_collapsed();
                }
            }
        });
        let progress = progress_bar(0);
        container.append(&progress);

        Self {
            container,
            summary,
            time,
            body,
            body_expansion,
            progress,
            expand_space,
        }
    }

    fn set_width_request(&self, width: i32) {
        self.container.set_width_request(width);
        self.body.set_width_request(width);
        self.body_expansion.overflow.set_width_request(width);
    }

    fn update(&self, notification: &Notification, time: Option<&str>) {
        self.summary.set_label(&notification.summary);
        self.time
            .set_label(&time.map(|time| format!("· {time}")).unwrap_or_default());
        self.time.set_visible(time.is_some());
        self.body_expansion.full.replace(notification.body.clone());
        self.body_expansion.split.replace(None);
        self.body.set_label(&notification.body);
        self.body.set_visible(!notification.body.trim().is_empty());
        self.progress
            .set_fraction(f64::from(notification.progress.unwrap_or_default()) / 100.0);
        self.progress.set_visible(notification.progress.is_some());
    }
}

impl RowView {
    pub(super) fn new(
        expanded_rows: &Rc<RefCell<HashSet<NotificationIdentity>>>,
        manager: &std::rc::Weak<Manager>,
        refresh_layout: &Rc<dyn Fn()>,
        interactive: bool,
    ) -> Self {
        let notification_text = NotificationText::new();
        let text = notification_text.container.clone();

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
        actions_revealer.connect_child_revealed_notify({
            let refresh_layout = Rc::clone(refresh_layout);
            move |_| refresh_layout()
        });
        notification_text
            .body_expansion
            .revealer
            .connect_child_revealed_notify({
                let refresh_layout = Rc::clone(refresh_layout);
                move |_| refresh_layout()
            });

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-row");
        container.set_overflow(gtk::Overflow::Hidden);
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
            let summary = notification_text.summary.clone();
            let body_expansion = notification_text.body_expansion.clone();
            let expand_icon = expand_icon.clone();
            let actions_revealer = actions_revealer.clone();
            let identity = Rc::clone(&identity);
            let expanded = Rc::clone(&expanded);
            let expanded_rows = Rc::clone(expanded_rows);
            let refresh_layout = Rc::clone(refresh_layout);
            move |_| {
                let value = !expanded.get();
                expanded.set(value);
                if value {
                    expanded_rows.borrow_mut().insert(identity.get());
                } else {
                    expanded_rows.borrow_mut().remove(&identity.get());
                }
                set_row_expanded(
                    &summary,
                    &body_expansion,
                    &expand_icon,
                    &actions_revealer,
                    value,
                );
                refresh_layout();
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
        }

        Self {
            container,
            surface,
            primary,
            text,
            picture,
            notification_text,
            expand,
            expand_icon,
            actions,
            actions_revealer,
            target,
            has_default,
            identity,
            expanded,
            expanded_rows: Rc::clone(expanded_rows),
            manager: manager.clone(),
            interactive,
        }
    }

    pub(super) fn update(&self, notification: &Notification) {
        let identity = (notification.id, notification.revision);
        self.target.set(notification.id);
        let changed = self.identity.replace(identity) != identity;
        let time = notification_time(notification.received_at);
        self.notification_text.time.set_label(
            &time
                .as_deref()
                .map(|time| format!("· {time}"))
                .unwrap_or_default(),
        );
        self.notification_text.time.set_visible(time.is_some());
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
        self.notification_text.update(notification, time.as_deref());

        self.picture.clear();
        let has_picture = set_picture(&self.picture, notification);
        self.picture.set_visible(has_picture);
        let text_width =
            NOTIFICATION_TEXT_WIDTH - if has_picture { PICTURE_TEXT_OFFSET } else { 0 };
        self.text.set_width_request(text_width);
        self.notification_text.set_width_request(text_width);

        let has_default = notification
            .actions
            .iter()
            .any(|action| action.key == "default");
        let actionable = self.interactive && has_default;
        self.has_default.set(actionable);
        self.primary.set_can_target(actionable);
        self.primary.set_focusable(actionable);
        self.surface
            .set_cursor_from_name(actionable.then_some("pointer"));
        self.primary
            .set_cursor_from_name(actionable.then_some("pointer"));

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
        self.notification_text
            .expand_space
            .set_visible(has_named_actions);
        let body = self.notification_text.body.clone();
        let expand = self.expand.clone();
        let expand_space = self.notification_text.expand_space.clone();
        let summary_text = self.notification_text.summary.clone();
        let body_expansion = self.notification_text.body_expansion.clone();
        let expanded = Rc::clone(&self.expanded);
        let expand_icon = self.expand_icon.clone();
        let actions_revealer = self.actions_revealer.clone();
        let current_identity = Rc::clone(&self.identity);
        let layout_ready = Cell::new(false);
        self.notification_text
            .summary
            .add_tick_callback(move |summary, _| {
                if current_identity.get() != identity {
                    return glib::ControlFlow::Break;
                }
                if !layout_ready.replace(true) {
                    return glib::ControlFlow::Continue;
                }
                body_expansion.split.replace(split_body(
                    &body,
                    &body_expansion.full.borrow(),
                    text_width,
                ));
                let value = has_named_actions
                    || summary.layout().is_ellipsized()
                    || body.layout().is_ellipsized()
                    || body_expansion.split.borrow().is_some();
                expand.set_visible(value);
                expand_space.set_visible(value);
                let restored = value && expanded.get();
                actions_revealer.set_transition_duration(0);
                body_expansion.revealer.set_transition_duration(0);
                set_row_expanded(
                    &summary_text,
                    &body_expansion,
                    &expand_icon,
                    &actions_revealer,
                    restored,
                );
                actions_revealer.set_transition_duration(GROUP_TRANSITION_DURATION);
                body_expansion
                    .revealer
                    .set_transition_duration(GROUP_TRANSITION_DURATION);
                glib::ControlFlow::Break
            });

        let expanded = self.expand.is_visible() && self.expanded.get();
        self.actions_revealer.set_transition_duration(0);
        self.notification_text
            .body_expansion
            .revealer
            .set_transition_duration(0);
        set_row_expanded(
            &self.notification_text.summary,
            &self.notification_text.body_expansion,
            &self.expand_icon,
            &self.actions_revealer,
            expanded,
        );
        self.actions_revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);
        self.notification_text
            .body_expansion
            .revealer
            .set_transition_duration(GROUP_TRANSITION_DURATION);
    }

    pub(super) fn refresh_hover(&self) {
        if let Some((x, y)) = pointer_position(self.surface.as_ref()) {
            update_notification_hover(
                &self.surface,
                &self.primary,
                &self.actions,
                &self.has_default,
                x,
                y,
            );
        } else {
            self.surface.remove_css_class("content-hover");
        }
    }
}

fn split_body(body: &gtk::Label, text: &str, width: i32) -> Option<(String, String)> {
    if text.trim().is_empty() || width <= 0 {
        return None;
    }
    let layout = body.create_pango_layout(Some(text));
    layout.set_width(width * gtk::pango::SCALE);
    layout.set_wrap(gtk::pango::WrapMode::WordChar);
    let mut split = usize::try_from(layout.line_readonly(3)?.start_index()).ok()?;
    while split > 0 {
        let prefix = text[..split].trim_end();
        layout.set_text(prefix);
        if layout.line_count() <= 3 {
            split = prefix.len();
            break;
        }
        split = prefix.rfind(char::is_whitespace)?;
    }
    Some((
        text[..split].trim_end().to_string(),
        text[split..].trim_start().to_string(),
    ))
}

fn set_row_expanded(
    summary: &gtk::Label,
    body: &BodyExpansion,
    expand_icon: &gtk::Label,
    actions: &gtk::Revealer,
    expanded: bool,
) {
    set_summary_expanded(summary, expanded);
    body.set_expanded(expanded);
    expand_icon.set_label(if expanded { "▴" } else { "▾" });
    actions.set_reveal_child(expanded);
}

fn set_summary_expanded(summary: &gtk::Label, expanded: bool) {
    summary.set_wrap(expanded);
    summary.set_lines(if expanded { -1 } else { 1 });
    summary.set_ellipsize(if expanded {
        gtk::pango::EllipsizeMode::None
    } else {
        gtk::pango::EllipsizeMode::End
    });
}

fn set_body_expanded(body: &gtk::Label, expanded: bool) {
    body.set_lines(if expanded { -1 } else { 3 });
    body.set_ellipsize(if expanded {
        gtk::pango::EllipsizeMode::None
    } else {
        gtk::pango::EllipsizeMode::End
    });
}
