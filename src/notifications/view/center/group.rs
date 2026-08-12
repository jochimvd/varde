use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use gtk::prelude::*;

use crate::notifications::{Manager, model::Group, view::common::application};

use super::{
    REVEAL_DURATION,
    row::{Pointer, RowView, within},
};

const ICON_SIZE: i32 = 20;

pub(super) struct GroupView {
    pub(super) container: gtk::Box,
    pub(super) key: String,
    header: gtk::Box,
    icon: gtk::Stack,
    image: gtk::Image,
    name: gtk::Label,
    count: gtk::Label,
    list: gtk::Box,
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
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let count = gtk::Label::builder().valign(gtk::Align::Center).build();
        count.add_css_class("notification-group-count");
        let disclosure = gtk::Button::builder().focusable(false).label("▾").build();
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

        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let revealer = gtk::Revealer::builder()
            .transition_duration(REVEAL_DURATION)
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(true)
            .child(&list)
            .build();
        disclosure.connect_clicked({
            let revealer = revealer.clone();
            let on_expansion = Rc::clone(on_expansion);
            move |disclosure| {
                let expanded = !revealer.reveals_child();
                revealer.set_reveal_child(expanded);
                disclosure.set_label(if expanded { "▾" } else { "▸" });
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
            count,
            list,
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

    pub(super) fn update(&self, group: &Group) {
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
        self.count.set_label(&group.notifications.len().to_string());
        self.notifications.replace(
            group
                .notifications
                .iter()
                .map(|notification| notification.id)
                .collect(),
        );

        let mut stale = self.rows.take();
        let mut rows = Vec::with_capacity(group.notifications.len());
        let mut sibling = None::<gtk::Widget>;
        for notification in &group.notifications {
            let row = match stale.iter().position(|row| row.id() == notification.id) {
                Some(index) => stale.remove(index),
                None => RowView::new(&self.manager, &self.on_expansion, self.interactive),
            };
            row.update(notification);
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
    }

    pub(super) fn refresh(&self) {
        for row in self.rows.borrow().iter() {
            row.refresh();
        }
    }

    pub(super) fn clear_hover(&self) {
        for row in self.rows.borrow().iter() {
            row.clear_hover();
        }
    }
}
