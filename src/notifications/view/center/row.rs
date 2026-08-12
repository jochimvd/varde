use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use gtk::{graphene, pango, prelude::*};

use crate::notifications::{
    Manager,
    model::Notification,
    state::Urgency,
    view::common::{activation_token, notification_time, progress_bar, set_picture},
};

use super::{CHEVRON_DOWN, CHEVRON_UP, REVEAL_DURATION};

const PICTURE_SIZE: i32 = 48;
const CONTENT_SPACING: i32 = 10;
const HEADER_SPACING: i32 = 5;
const EXPAND_BUTTON_WIDTH: i32 = 30;
const BODY_LINES: i32 = 3;
const RESIDENT_ICON: &str = "󰐃";

/// Marks a notification the user has not been shown yet.
pub(super) fn fresh_dot() -> gtk::Box {
    let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
    dot.add_css_class("notification-fresh");
    dot.set_visible(false);
    dot
}

pub(super) struct RowView {
    pub(super) container: gtk::Box,
    surface: gtk::Box,
    picture: gtk::Image,
    actions: gtk::FlowBox,
    expansion: Rc<Expansion>,
    id: Cell<u32>,
    received_at: Cell<Option<i64>>,
    revision: Cell<Option<u64>>,
    named_actions: Cell<bool>,
    actionable: Rc<Cell<bool>>,
    manager: Weak<Manager>,
    interactive: bool,
}

/// What the pointer rests on inside the notification list.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Pointer {
    /// The header of the group with this key.
    Header(String),
    Row {
        id: u32,
        activates: bool,
    },
}

/// Everything the expansion control reveals: the full summary, the body text
/// that does not fit the collapsed row, and the named actions.
struct Expansion {
    text: Text,
    expand: gtk::Button,
    actions: gtk::Revealer,
    expanded: Cell<bool>,
}

struct Text {
    container: gtk::Box,
    summary: gtk::Label,
    time: gtk::Label,
    resident: gtk::Label,
    fresh: gtk::Box,
    body: gtk::Label,
    overflow: gtk::Label,
    overflow_revealer: gtk::Revealer,
    full_body: Rc<RefCell<String>>,
    split: RefCell<Option<(String, String)>>,
    progress: gtk::ProgressBar,
}

impl RowView {
    pub(super) fn new(
        manager: &Weak<Manager>,
        on_expansion: &Rc<dyn Fn()>,
        interactive: bool,
    ) -> Self {
        let text = Text::new();
        let picture = gtk::Image::builder()
            .pixel_size(PICTURE_SIZE)
            .width_request(PICTURE_SIZE)
            .height_request(PICTURE_SIZE)
            .valign(gtk::Align::Start)
            .build();
        picture.set_overflow(gtk::Overflow::Hidden);
        picture.add_css_class("notification-picture");
        let content = gtk::Box::builder()
            .spacing(CONTENT_SPACING)
            .valign(gtk::Align::Start)
            .build();
        content.add_css_class("notification-content");
        content.append(&picture);
        content.append(&text.container);

        let expand = gtk::Button::builder()
            .label(CHEVRON_DOWN)
            .focusable(false)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        expand.set_cursor_from_name(Some("pointer"));
        expand.add_css_class("notification-expand");
        let overlay = gtk::Overlay::builder().child(&content).build();
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
            .transition_duration(REVEAL_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&actions)
            .build();

        let surface = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        surface.add_css_class("notification-row-surface");
        surface.append(&overlay);
        surface.append(&actions_revealer);
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-row");
        container.append(&surface);

        let expansion = Rc::new(Expansion {
            text,
            expand: expand.clone(),
            actions: actions_revealer.clone(),
            expanded: Cell::new(false),
        });
        expand.connect_clicked({
            let expansion = Rc::downgrade(&expansion);
            let on_expansion = Rc::clone(on_expansion);
            move |_| {
                if let Some(expansion) = expansion.upgrade() {
                    expansion.set(!expansion.expanded.get());
                    on_expansion();
                }
            }
        });

        let actionable = Rc::new(Cell::new(false));
        track_hover(&surface, &actionable);

        Self {
            container,
            surface,
            picture,
            actions,
            expansion,
            id: Cell::new(0),
            received_at: Cell::new(None),
            revision: Cell::new(None),
            named_actions: Cell::new(false),
            actionable,
            manager: manager.clone(),
            interactive,
        }
    }

    /// Reports what the point rests on, once the caller has resolved it to a
    /// widget. Right-clicking anywhere in the row targets the notification;
    /// only the surface itself activates it.
    pub(super) fn pointer_target(&self, picked: &gtk::Widget) -> Option<Pointer> {
        within(picked, &self.container).then(|| Pointer::Row {
            id: self.id.get(),
            activates: self.actionable.get() && !on_control(picked),
        })
    }

    pub(super) fn id(&self) -> u32 {
        self.id.get()
    }

    pub(super) fn refresh_time(&self) {
        let time = notification_time(self.received_at.get());
        self.expansion.text.set_time(time.as_deref());
    }

    pub(super) fn update(&self, notification: &Notification, fresh: bool) {
        self.id.set(notification.id);
        self.received_at.set(notification.received_at);
        let time = notification_time(notification.received_at);
        self.expansion.text.set_time(time.as_deref());
        self.expansion.text.fresh.set_visible(fresh);
        if self.revision.replace(Some(notification.revision)) == Some(notification.revision) {
            return;
        }

        self.clear_hover();
        if notification.urgency == Urgency::Critical {
            self.container.add_css_class("critical");
        } else {
            self.container.remove_css_class("critical");
        }

        self.picture.clear();
        self.picture
            .set_visible(set_picture(&self.picture, notification));
        self.expansion.text.update(notification, time.as_deref());

        let actionable = self.interactive
            && notification
                .actions
                .iter()
                .any(|action| action.key == "default");
        self.actionable.set(actionable);

        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        for action in notification
            .actions
            .iter()
            .filter(|action| action.key != "default" && !action.label.trim().is_empty())
        {
            let label = gtk::Label::builder()
                .label(&action.label)
                .max_width_chars(48)
                .ellipsize(pango::EllipsizeMode::End)
                .build();
            let button = gtk::Button::builder().child(&label).build();
            button.set_cursor_from_name(self.interactive.then_some("pointer"));
            button.set_sensitive(self.interactive);
            button.add_css_class("notification-action");
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
        self.named_actions.set(self.actions.first_child().is_some());

        self.expansion.set(false);
        self.expansion.expand.set_visible(self.named_actions.get());
    }

    /// Applies the state that only the rendered layout can settle: whether the
    /// pointer rests on this row, and how much of its text is cut off.
    pub(super) fn refresh(&self) {
        self.refresh_hover();
        if self.expansion.expanded.get() {
            return;
        }
        let overflows = self.expansion.text.measure_overflow();
        self.expansion
            .expand
            .set_visible(self.named_actions.get() || overflows);
    }

    pub(super) fn clear_hover(&self) {
        set_hover(&self.surface, false);
    }

    fn refresh_hover(&self) {
        let hovered = self.actionable.get()
            && pointer_position(&self.surface)
                .and_then(|(x, y)| self.surface.pick(x, y, gtk::PickFlags::DEFAULT))
                .is_some_and(|picked| !on_control(&picked));
        set_hover(&self.surface, hovered);
    }
}

impl Expansion {
    fn set(&self, expanded: bool) {
        self.expanded.set(expanded);
        self.expand
            .set_label(if expanded { CHEVRON_UP } else { CHEVRON_DOWN });
        self.text.set_expanded(expanded);
        self.actions.set_reveal_child(expanded);
    }
}

impl Text {
    fn new() -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();

        let summary = gtk::Label::builder()
            .xalign(0.0)
            .wrap_mode(pango::WrapMode::WordChar)
            .lines(1)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("notification-summary");
        let time = gtk::Label::new(None);
        time.add_css_class("notification-time");
        let resident = gtk::Label::new(Some(RESIDENT_ICON));
        resident.add_css_class("notification-resident");
        resident.set_tooltip_text(Some("Ongoing"));
        resident.set_visible(false);
        let fresh = fresh_dot();
        let spacer = gtk::Box::builder().hexpand(true).build();
        // Reserved for the expansion button so the summary width does not
        // depend on whether the row can be expanded.
        let expand_space = gtk::Box::builder()
            .width_request(EXPAND_BUTTON_WIDTH)
            .build();
        let header = gtk::Box::builder()
            .spacing(HEADER_SPACING)
            .valign(gtk::Align::Center)
            .build();
        header.append(&summary);
        header.append(&time);
        header.append(&resident);
        header.append(&fresh);
        header.append(&spacer);
        header.append(&expand_space);
        container.append(&header);

        let body = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .lines(BODY_LINES)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        body.add_css_class("notification-body");
        container.append(&body);

        let overflow = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .build();
        overflow.add_css_class("notification-body");
        overflow.add_css_class("overflow");
        let overflow_revealer = gtk::Revealer::builder()
            .transition_duration(REVEAL_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&overflow)
            .build();
        container.append(&overflow_revealer);

        let full_body = Rc::new(RefCell::new(String::new()));
        overflow_revealer.connect_child_revealed_notify({
            let body = body.clone();
            let full_body = Rc::clone(&full_body);
            move |revealer| {
                if !revealer.reveals_child() && !revealer.is_child_revealed() {
                    body.set_label(&full_body.borrow());
                    set_body_lines(&body, BODY_LINES);
                }
            }
        });

        let progress = progress_bar(0);
        container.append(&progress);

        Self {
            container,
            summary,
            time,
            resident,
            fresh,
            body,
            overflow,
            overflow_revealer,
            full_body,
            split: RefCell::new(None),
            progress,
        }
    }

    fn set_time(&self, time: Option<&str>) {
        self.time
            .set_label(&time.map(|time| format!("· {time}")).unwrap_or_default());
        self.time.set_visible(time.is_some());
    }

    fn update(&self, notification: &Notification, time: Option<&str>) {
        self.summary.set_label(&notification.summary);
        self.set_time(time);
        self.resident.set_visible(notification.resident);

        self.full_body.replace(notification.body.clone());
        self.split.replace(None);
        self.body.set_label(&notification.body);
        self.body.set_visible(!notification.body.trim().is_empty());
        set_body_lines(&self.body, BODY_LINES);

        self.progress
            .set_fraction(f64::from(notification.progress.unwrap_or_default()) / 100.0);
        self.progress.set_visible(notification.progress.is_some());
    }

    /// Splits off the body text the collapsed row cuts away, and reports
    /// whether any text is hidden. Only the rendered width is exact, so this
    /// runs once the row has been laid out.
    fn measure_overflow(&self) -> bool {
        self.split.replace(split_body(
            &self.body,
            &self.full_body.borrow(),
            self.body.width(),
        ));
        self.split.borrow().is_some() || self.summary.layout().is_ellipsized()
    }

    fn set_expanded(&self, expanded: bool) {
        self.summary.set_wrap(expanded);
        self.summary.set_lines(if expanded { -1 } else { 1 });
        self.summary.set_ellipsize(if expanded {
            pango::EllipsizeMode::None
        } else {
            pango::EllipsizeMode::End
        });

        let split = self.split.borrow();
        match (expanded, split.as_ref()) {
            (true, Some((head, tail))) => {
                self.body.set_label(head);
                set_body_lines(&self.body, -1);
                self.overflow.set_label(tail);
                self.overflow_revealer.set_reveal_child(true);
            }
            (true, None) => {}
            (false, _) => {
                self.overflow_revealer.set_reveal_child(false);
                if !self.overflow_revealer.is_child_revealed() {
                    self.body.set_label(&self.full_body.borrow());
                    set_body_lines(&self.body, BODY_LINES);
                }
            }
        }
    }
}

fn set_body_lines(body: &gtk::Label, lines: i32) {
    body.set_lines(lines);
    body.set_ellipsize(if lines < 0 {
        pango::EllipsizeMode::None
    } else {
        pango::EllipsizeMode::End
    });
}

/// Splits `text` into the lines the collapsed row shows and the remainder, or
/// returns `None` when all of it fits.
fn split_body(body: &gtk::Label, text: &str, width: i32) -> Option<(String, String)> {
    if width <= 0 {
        return None;
    }
    let layout = body.create_pango_layout(Some(text));
    layout.set_width(width * pango::SCALE);
    layout.set_wrap(pango::WrapMode::WordChar);
    let split = usize::try_from(layout.line_readonly(BODY_LINES)?.start_index()).ok()?;
    Some((
        text[..split].trim_end().to_string(),
        text[split..].trim_start().to_string(),
    ))
}

fn track_hover(surface: &gtk::Box, actionable: &Rc<Cell<bool>>) {
    let motion = gtk::EventControllerMotion::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    let update = {
        let surface = surface.downgrade();
        let actionable = Rc::clone(actionable);
        move |x, y| {
            let Some(surface) = surface.upgrade() else {
                return;
            };
            let hovered = actionable.get()
                && surface
                    .pick(x, y, gtk::PickFlags::DEFAULT)
                    .is_some_and(|picked| !on_control(&picked));
            set_hover(&surface, hovered);
        }
    };
    motion.connect_enter({
        let update = update.clone();
        move |_, x, y| update(x, y)
    });
    motion.connect_motion(move |_, x, y| update(x, y));
    motion.connect_leave({
        let surface = surface.downgrade();
        move |_| {
            if let Some(surface) = surface.upgrade() {
                set_hover(&surface, false);
            }
        }
    });
    surface.add_controller(motion);
}

/// Highlighting and the pointer shape share one state, so a row that stops
/// being hovered - including because it was replaced or removed under a
/// motionless pointer - always drops both at once.
fn set_hover(surface: &gtk::Box, hovered: bool) {
    if hovered == surface.has_css_class("content-hover") {
        return;
    }
    if hovered {
        surface.add_css_class("content-hover");
    } else {
        surface.remove_css_class("content-hover");
    }
    surface.set_cursor_from_name(hovered.then_some("pointer"));
}

/// Only the buttons themselves are controls; the space around them in the
/// action row still belongs to the notification.
fn on_control(picked: &gtk::Widget) -> bool {
    picked.ancestor(gtk::Button::static_type()).is_some()
}

pub(super) fn within(widget: &gtk::Widget, container: &impl IsA<gtk::Widget>) -> bool {
    widget == container.as_ref() || widget.is_ancestor(container)
}

fn pointer_position(widget: &impl IsA<gtk::Widget>) -> Option<(f64, f64)> {
    let widget = widget.as_ref();
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
