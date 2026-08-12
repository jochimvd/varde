use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use gtk::prelude::*;

use crate::notifications::{Manager, model::Group, state::Urgency, view::common::application};

use super::{
    CHEVRON_DOWN, CHEVRON_UP, REVEAL_DURATION, Seen,
    row::{Pointer, RowView, fresh_dot, within},
};

const ICON_SIZE: i32 = 20;

pub(super) struct GroupView {
    pub(super) container: gtk::Box,
    pub(super) key: String,
    header: gtk::Box,
    icon: gtk::Stack,
    image: gtk::Image,
    name: gtk::Label,
    fresh: gtk::Box,
    has_fresh: Rc<Cell<bool>>,
    count: gtk::Label,
    list: gtk::Box,
    revealer: gtk::Revealer,
    rows: RefCell<Vec<RowView>>,
    notifications: RefCell<Vec<u32>>,
    on_expansion: Rc<dyn Fn()>,
    manager: Weak<Manager>,
    interactive: bool,
}

impl GroupView {
    pub(super) fn new(
        key: &str,
        manager: &Weak<Manager>,
        on_expansion: &Rc<dyn Fn()>,
        interactive: bool,
    ) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-group");
        container.set_overflow(gtk::Overflow::Hidden);

        let image = gtk::Image::builder().pixel_size(ICON_SIZE).build();
        let fallback = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fallback.add_css_class("notification-group-icon-fallback");
        let icon = gtk::Stack::builder()
            .width_request(ICON_SIZE)
            .height_request(ICON_SIZE)
            .build();
        icon.add_named(&image, Some("image"));
        icon.add_named(&fallback, Some("fallback"));
        icon.add_css_class("notification-group-icon");
        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let fresh = fresh_dot();
        let count = gtk::Label::builder().valign(gtk::Align::Center).build();
        count.add_css_class("notification-group-count");
        let chevron = gtk::Label::new(Some(CHEVRON_UP));
        chevron.add_css_class("notification-group-chevron");
        let disclosure_content = gtk::Box::builder().valign(gtk::Align::Center).build();
        disclosure_content.append(&count);
        disclosure_content.append(&chevron);
        let disclosure = gtk::Button::builder()
            .focusable(false)
            .child(&disclosure_content)
            .build();
        disclosure.set_cursor_from_name(Some("pointer"));
        disclosure.add_css_class("notification-group-disclosure");
        let header = gtk::Box::builder()
            .spacing(8)
            .valign(gtk::Align::Center)
            .build();
        header.add_css_class("notification-group-header");
        let spacer = gtk::Box::builder().hexpand(true).build();
        header.append(&icon);
        header.append(&name);
        header.append(&fresh);
        header.append(&spacer);
        header.append(&disclosure);

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_duration(REVEAL_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(true)
            .child(&list)
            .build();
        let has_fresh = Rc::new(Cell::new(false));
        disclosure.connect_clicked({
            let container = container.clone();
            let revealer = revealer.clone();
            let chevron = chevron.clone();
            let fresh = fresh.clone();
            let has_fresh = Rc::clone(&has_fresh);
            let on_expansion = Rc::clone(on_expansion);
            move |_| {
                let expanded = !revealer.reveals_child();
                revealer.set_reveal_child(expanded);
                chevron.set_label(if expanded { CHEVRON_UP } else { CHEVRON_DOWN });
                fresh.set_visible(has_fresh.get() && !expanded);
                if expanded {
                    container.remove_css_class("collapsed");
                } else {
                    container.add_css_class("collapsed");
                }
                on_expansion();
            }
        });

        container.append(&header);
        container.append(&revealer);

        Self {
            container,
            key: key.to_string(),
            header,
            icon,
            image,
            name,
            fresh,
            has_fresh,
            count,
            list,
            revealer,
            rows: RefCell::new(Vec::new()),
            notifications: RefCell::new(Vec::new()),
            on_expansion: Rc::clone(on_expansion),
            manager: manager.clone(),
            interactive,
        }
    }

    pub(super) fn notifications(&self) -> Vec<u32> {
        self.notifications.borrow().clone()
    }

    pub(super) fn pointer_target(&self, picked: &gtk::Widget) -> Option<Pointer> {
        if within(picked, &self.header) {
            return Some(Pointer::Header(self.key.clone()));
        }
        self.rows
            .borrow()
            .iter()
            .find_map(|row| row.pointer_target(picked))
    }

    pub(super) fn update(&self, group: &Group, seen: &Seen) {
        let (name, icon) = application(group);
        self.image.clear();
        match icon {
            Some(icon) => {
                self.image.set_from_gicon(&icon);
                self.icon.set_visible_child_name("image");
            }
            None => self.icon.set_visible_child_name("fallback"),
        }
        self.name.set_label(&name);
        let count = group.notifications.len();
        self.count.set_label(&count.to_string());
        self.count.set_visible(count > 1);
        self.notifications.replace(
            group
                .notifications
                .iter()
                .map(|notification| notification.id)
                .collect(),
        );

        let mut fresh = false;
        let mut stale = self.rows.take();
        let mut rows = Vec::with_capacity(group.notifications.len());
        let mut sibling = None::<gtk::Widget>;
        for notification in &group.notifications {
            let row = match stale.iter().position(|row| row.id() == notification.id) {
                Some(index) => stale.remove(index),
                None => RowView::new(&self.manager, &self.on_expansion, self.interactive),
            };
            let unseen = !seen.contains(&(notification.id, notification.revision));
            fresh |= unseen;
            row.update(notification, unseen);
            if row.container.parent().is_none() {
                self.list.append(&row.container);
            }
            self.list
                .reorder_child_after(&row.container, sibling.as_ref());
            sibling = Some(row.container.clone().upcast());
            rows.push(row);
        }
        for row in &stale {
            self.list.remove(&row.container);
        }
        self.rows.replace(rows);
        // A collapsed group hides its rows, so the header carries their mark.
        let critical = group
            .notifications
            .iter()
            .any(|notification| notification.urgency == Urgency::Critical);
        if critical {
            self.container.add_css_class("critical");
        } else {
            self.container.remove_css_class("critical");
        }
        self.has_fresh.set(fresh);
        self.fresh
            .set_visible(fresh && !self.revealer.reveals_child());
    }

    pub(super) fn refresh(&self) {
        for row in self.rows.borrow().iter() {
            row.refresh();
        }
    }

    pub(super) fn refresh_times(&self) {
        for row in self.rows.borrow().iter() {
            row.refresh_time();
        }
    }

    pub(super) fn clear_hover(&self) {
        for row in self.rows.borrow().iter() {
            row.clear_hover();
        }
    }
}
