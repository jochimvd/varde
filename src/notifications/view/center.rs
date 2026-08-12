use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use gtk::{gdk, glib, prelude::*};

mod group;
mod row;

use group::GroupView;
use row::Pointer;

use super::{
    super::{Manager, model::Snapshot},
    common::{activation_token, message},
};

const PANEL_WIDTH: i32 = 460;
const PANEL_OFFSET: i32 = 18;
const MIN_CONTENT_HEIGHT: i32 = 98;
const MAX_CONTENT_HEIGHT: i32 = 520;
const GROUP_SPACING: i32 = 8;
const REVEAL_DURATION: u32 = 150;

pub(in crate::notifications) struct Center {
    popover: gtk::Popover,
    scroll: gtk::ScrolledWindow,
    list: gtk::Box,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    groups: Rc<RefCell<Vec<GroupView>>>,
    on_expansion: Rc<dyn Fn()>,
    manager: Weak<Manager>,
    interactive: bool,
}

impl Center {
    pub fn new(anchor: &gtk::ApplicationWindow, manager: &Rc<Manager>, interactive: bool) -> Self {
        let popover = gtk::Popover::builder()
            .autohide(true)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::End)
            .build();
        popover.add_css_class("notifications");
        popover.set_offset(-PANEL_OFFSET, PANEL_OFFSET);
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

        let groups = Rc::new(RefCell::new(Vec::new()));
        if interactive {
            install_pointer_actions(&popover, &groups, manager);
        }
        Self {
            on_expansion: expansion_tracker(&popover, &groups),
            popover,
            scroll,
            list,
            stack,
            dnd,
            clear,
            groups,
            manager: Rc::downgrade(manager),
            interactive,
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        self.render(snapshot, !self.popover.is_visible());
    }

    pub fn show(&self, snapshot: &Snapshot) {
        self.render(snapshot, true);
        self.scroll.vadjustment().set_value(0.0);
        self.popover.popup();
        self.refresh_after_layout();
    }

    pub fn hide(&self) {
        for group in self.groups.borrow().iter() {
            group.clear_hover();
        }
        self.popover.popdown();
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
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
                None => GroupView::new(key, &self.manager, &self.on_expansion, self.interactive),
            };
            view.update(group);
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

        if self.popover.is_visible() {
            self.popover.present();
            self.refresh_after_layout();
        }
    }

    /// Hover state and ellipsization both depend on the rendered layout, which
    /// only settles on the frame after the one this render is part of.
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

/// Activates and dismisses notifications from the event position instead of
/// from the widget the click was delivered to. GTK keeps pointing a motionless
/// pointer at the widget it entered, so a row that moves under the pointer -
/// because the row above it was dismissed - would otherwise swallow the first
/// click while GTK catches up.
fn install_pointer_actions(
    popover: &gtk::Popover,
    groups: &Rc<RefCell<Vec<GroupView>>>,
    manager: &Rc<Manager>,
) {
    // Bubble phase: closing the center from the capture phase would tear the
    // popover down while GTK is still delivering the event, and it stays up.
    let buttons = gtk::EventControllerLegacy::new();
    buttons.connect_event({
        let popover = popover.downgrade();
        let groups = Rc::downgrade(groups);
        let manager = Rc::downgrade(manager);
        let pressed = RefCell::new(None);
        move |_, event| {
            let Some(event) = event.downcast_ref::<gdk::ButtonEvent>() else {
                return glib::Propagation::Proceed;
            };
            let press = match event.event_type() {
                gdk::EventType::ButtonPress => true,
                gdk::EventType::ButtonRelease => false,
                _ => return glib::Propagation::Proceed,
            };
            let (Some(popover), Some(groups), Some(manager), Some((x, y))) = (
                popover.upgrade(),
                groups.upgrade(),
                manager.upgrade(),
                event.position(),
            ) else {
                return glib::Propagation::Proceed;
            };
            let button = event.button();
            let target = popover
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .and_then(|picked| {
                    groups
                        .borrow()
                        .iter()
                        .find_map(|group| group.pointer_target(&picked))
                });
            if press {
                pressed.replace(target);
                return glib::Propagation::Proceed;
            }
            if pressed.take() != target {
                return glib::Propagation::Proceed;
            }
            match (button, target) {
                (gdk::BUTTON_SECONDARY, Some(Pointer::Header(key))) => {
                    if let Some(group) = groups.borrow().iter().find(|group| group.key == key) {
                        manager.dismiss_group(group.notifications());
                    }
                }
                (gdk::BUTTON_SECONDARY, Some(Pointer::Row { id, .. })) => manager.dismiss(id),
                (
                    gdk::BUTTON_PRIMARY,
                    Some(Pointer::Row {
                        id,
                        activates: true,
                    }),
                ) => {
                    manager.invoke_action(id, "default", activation_token(&popover));
                    manager.close();
                }
                _ => {}
            }
            glib::Propagation::Proceed
        }
    });
    popover.add_controller(buttons);
}

/// Follows an expansion that a group or a notification just started. A popover
/// only resizes its surface when it is presented, so it has to be presented for
/// every frame of the animation; the rows it moved settle once it ends.
fn expansion_tracker(popover: &gtk::Popover, groups: &Rc<RefCell<Vec<GroupView>>>) -> Rc<dyn Fn()> {
    let popover = popover.downgrade();
    let groups = Rc::downgrade(groups);
    Rc::new(move || {
        let (Some(popover), Some(groups)) = (popover.upgrade(), groups.upgrade()) else {
            return;
        };
        let Some(clock) = popover.frame_clock() else {
            return;
        };
        // The reveal duration plus a frame, so the settled size is presented.
        let deadline = clock.frame_time() + i64::from(REVEAL_DURATION + 20) * 1000;
        popover.add_tick_callback(move |popover, clock| {
            popover.present();
            if clock.frame_time() < deadline {
                return glib::ControlFlow::Continue;
            }
            for group in groups.borrow().iter() {
                group.refresh();
            }
            glib::ControlFlow::Break
        });
    })
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

    order.retain(|key| keys.contains(key));
    for key in keys {
        if !order.contains(key) {
            order.push(key.clone());
        }
    }
}
