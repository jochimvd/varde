use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::{Rc, Weak},
};

use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

mod group;
mod row;

use group::GroupView;

use super::{
    super::{Manager, model::Snapshot},
    common::message,
};

const CENTER_NAME: &str = "varde-notification-center";
const BACKDROP_NAME: &str = "varde-notification-backdrop";
const PANEL_WIDTH: i32 = 460;
/// The panel's gap from the screen corner, kept inside the surface on every
/// side so its shadow is not clipped.
const PANEL_OFFSET: i32 = 18;
const MIN_CONTENT_HEIGHT: i32 = 98;
const MAX_CONTENT_HEIGHT: i32 = 520;
const GROUP_SPACING: i32 = 8;
pub(super) const REVEAL_DURATION: u32 = 150;
const TIME_REFRESH_SECONDS: u32 = 15;
/// Chevrons show what the click does: reveal, or collapse again.
pub(super) const CHEVRON_DOWN: &str = "\u{f0140}";
pub(super) const CHEVRON_UP: &str = "\u{f0143}";

/// The notifications on screen when the center opened; anything else arrived
/// while the user was reading it.
type Seen = HashSet<(u32, u64)>;

pub(in crate::notifications) struct Center {
    window: gtk::ApplicationWindow,
    backdrop: gtk::ApplicationWindow,
    scroll: gtk::ScrolledWindow,
    list: gtk::Box,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    groups: Rc<RefCell<Vec<GroupView>>>,
    seen: RefCell<Seen>,
    ticking: Rc<Cell<bool>>,
    manager: Weak<Manager>,
    interactive: bool,
}

impl Center {
    pub fn new(app: &gtk::Application, manager: &Rc<Manager>, interactive: bool) -> Self {
        let backdrop = gtk::ApplicationWindow::builder()
            .application(app)
            .name(BACKDROP_NAME)
            .build();
        backdrop.add_css_class("notification-backdrop");
        backdrop.init_layer_shell();
        backdrop.set_namespace(Some(BACKDROP_NAME));
        backdrop.set_layer(Layer::Overlay);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            backdrop.set_anchor(edge, true);
        }
        backdrop.set_exclusive_zone(-1);
        backdrop.set_keyboard_mode(KeyboardMode::None);
        let outside = gtk::GestureClick::builder().button(0).build();
        outside.connect_released({
            let manager = Rc::downgrade(manager);
            move |_, _, _, _| {
                if let Some(manager) = manager.upgrade() {
                    manager.close();
                }
            }
        });
        // GTK skips rendering a window with nothing to draw, and a surface
        // without a buffer never maps; an empty drawing still renders one.
        let area = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .build();
        area.set_draw_func(|_, _, _, _| {});
        area.add_controller(outside);
        backdrop.set_child(Some(&area));

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .name(CENTER_NAME)
            .resizable(false)
            .build();
        window.add_css_class("notifications");
        window.init_layer_shell();
        window.set_namespace(Some(CENTER_NAME));
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        window.set_keyboard_mode(KeyboardMode::OnDemand);
        for window in [&window, &backdrop] {
            window.connect_close_request({
                let manager = Rc::downgrade(manager);
                move |_| {
                    if let Some(manager) = manager.upgrade() {
                        manager.close();
                    }
                    glib::Propagation::Stop
                }
            });
        }

        let title = gtk::Label::builder()
            .label("Notifications")
            .hexpand(true)
            .xalign(0.0)
            .build();
        title.add_css_class("notification-center-title");
        let dnd = control_button("󰂛", "Enable Do Not Disturb");
        dnd.connect_clicked({
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.toggle_dnd();
                }
            }
        });
        let clear = control_button("󰆴", "Clear all notifications");
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

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(GROUP_SPACING)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(MIN_CONTENT_HEIGHT)
            .max_content_height(MAX_CONTENT_HEIGHT)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        scroll.add_css_class("notification-center-scroll");
        scroll.vscrollbar().set_can_target(false);

        let empty = gtk::Label::new(Some("All caught up"));
        empty.add_css_class("notification-center-empty");
        let unavailable = message(
            "Notifications are unavailable",
            "notification-center-unavailable",
        );
        let stack = gtk::Stack::builder().vhomogeneous(false).build();
        stack.add_named(&scroll, Some("content"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&unavailable, Some("unavailable"));

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .width_request(PANEL_WIDTH)
            .margin_top(PANEL_OFFSET)
            .margin_bottom(PANEL_OFFSET)
            .margin_start(PANEL_OFFSET)
            .margin_end(PANEL_OFFSET)
            .build();
        panel.add_css_class("notification-center");
        panel.append(&header);
        panel.append(&stack);
        window.set_child(Some(&panel));

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
            backdrop,
            scroll,
            list,
            stack,
            dnd,
            clear,
            groups: Rc::new(RefCell::new(Vec::new())),
            seen: RefCell::new(Seen::new()),
            ticking: Rc::new(Cell::new(false)),
            manager: Rc::downgrade(manager),
            interactive,
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        self.render(snapshot, !self.is_visible());
    }

    /// Opens on `monitor`, or where the compositor puts new surfaces.
    pub fn show(&self, snapshot: &Snapshot, monitor: Option<&gdk::Monitor>) {
        self.seen.replace(
            snapshot
                .groups
                .iter()
                .flat_map(|group| &group.notifications)
                .map(|notification| (notification.id, notification.revision))
                .collect(),
        );
        self.render(snapshot, true);
        self.scroll.vadjustment().set_value(0.0);
        // The backdrop maps first so the panel stacks above it.
        for window in [&self.backdrop, &self.window] {
            window.set_monitor(monitor);
            window.present();
        }
        self.refresh_after_layout();
        self.refresh_times_while_open();
    }

    pub fn hide(&self) {
        // Both windows get fresh surfaces. Hyprland keeps keyboard focus on a
        // layer's surface across remaps without announcing it again, while GTK
        // drops it on hide; a fresh surface gets the focus announced. Showing
        // a kept surface again trips GDK's frame timing check.
        for window in [&self.window, &self.backdrop] {
            window.set_visible(false);
            WidgetExt::unrealize(window);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    /// The monitor the open center is on.
    pub fn monitor(&self) -> Option<gdk::Monitor> {
        let surface = self.window.surface().filter(|_| self.is_visible())?;
        surface.display().monitor_at_surface(&surface)
    }

    fn render(&self, snapshot: &Snapshot, reset_order: bool) {
        let keys = snapshot
            .groups
            .iter()
            .map(|group| group.key.clone())
            .collect::<Vec<_>>();
        let mut order = self
            .groups
            .borrow()
            .iter()
            .map(|group| group.key.clone())
            .collect::<Vec<_>>();
        update_group_order(&mut order, &keys, reset_order);

        let mut stale = self.groups.take();
        let mut groups = Vec::with_capacity(order.len());
        let mut sibling = None::<gtk::Widget>;
        for key in &order {
            let group = snapshot
                .groups
                .iter()
                .find(|group| group.key == *key)
                .expect("the group order only holds keys of the current snapshot");
            let view = match stale.iter().position(|view| view.key == *key) {
                Some(index) => stale.remove(index),
                None => GroupView::new(key, &self.manager, self.interactive),
            };
            view.update(group, &self.seen.borrow());
            if view.container.parent().is_none() {
                self.list.append(&view.container);
            }
            self.list
                .reorder_child_after(&view.container, sibling.as_ref());
            sibling = Some(view.container.clone().upcast());
            groups.push(view);
        }
        for view in &stale {
            self.list.remove(&view.container);
        }
        self.groups.replace(groups);

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

        if self.is_visible() {
            self.refresh_after_layout();
        }
    }

    /// "now" stops being true while the center sits open.
    fn refresh_times_while_open(&self) {
        if self.ticking.replace(true) {
            return;
        }
        let window = self.window.downgrade();
        let groups = Rc::downgrade(&self.groups);
        let ticking = Rc::clone(&self.ticking);
        glib::timeout_add_seconds_local(TIME_REFRESH_SECONDS, move || {
            let (Some(window), Some(groups)) = (window.upgrade(), groups.upgrade()) else {
                ticking.set(false);
                return glib::ControlFlow::Break;
            };
            if !window.is_visible() {
                ticking.set(false);
                return glib::ControlFlow::Break;
            }
            for group in groups.borrow().iter() {
                group.refresh_times();
            }
            glib::ControlFlow::Continue
        });
    }

    /// Ellipsization depends on the rendered layout, which only settles on the
    /// frame after the one this render is part of.
    fn refresh_after_layout(&self) {
        let groups = Rc::downgrade(&self.groups);
        let laid_out = Cell::new(false);
        self.list.add_tick_callback(move |_, _| {
            if !laid_out.replace(true) {
                return glib::ControlFlow::Continue;
            }
            if let Some(groups) = groups.upgrade() {
                for group in groups.borrow().iter() {
                    group.refresh();
                }
            }
            glib::ControlFlow::Break
        });
    }
}

fn control_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::with_label(icon);
    button.set_cursor_from_name(Some("pointer"));
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("notification-center-control");
    button.add_css_class("notification-center-icon");
    button
}

pub(super) fn update_group_order(order: &mut Vec<String>, keys: &[String], reset: bool) {
    if reset {
        order.clear();
        order.extend_from_slice(keys);
        return;
    }

    let new = keys
        .iter()
        .filter(|key| !order.contains(key))
        .cloned()
        .collect::<Vec<_>>();
    order.retain(|key| keys.contains(key));
    order.splice(0..0, new);
}
