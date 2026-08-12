use std::{cell::RefCell, collections::HashSet, rc::Rc};

use gtk::{glib, prelude::*};

use crate::notifications::Manager;

use super::{CenterItem, NotificationIdentity, row::RowView};
use crate::notifications::view::common::application;

pub(super) struct ItemView {
    pub(super) container: gtk::Box,
    expander: gtk::TreeExpander,
    header: gtk::Box,
    icon: gtk::Stack,
    image: gtk::Image,
    name: gtk::Label,
    count: gtk::Label,
    disclosure: gtk::Button,
    tree_row: Rc<RefCell<Option<gtk::TreeListRow>>>,
    expanded_handler: RefCell<Option<(gtk::TreeListRow, glib::SignalHandlerId)>>,
    notifications: Rc<RefCell<Vec<u32>>>,
    row: RowView,
}

pub(super) struct ItemContext {
    pub(super) expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
    pub(super) manager: std::rc::Weak<Manager>,
    pub(super) refresh_layout: Rc<dyn Fn()>,
    pub(super) interactive: bool,
}

impl ItemView {
    pub(super) fn new(context: &ItemContext) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        container.add_css_class("notification-center-item");
        container.set_overflow(gtk::Overflow::Hidden);

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

        let expander = gtk::TreeExpander::new();
        expander.set_child(Some(&header));
        expander.set_hide_expander(true);
        expander.set_indent_for_depth(false);
        expander.set_indent_for_icon(false);

        let tree_row = Rc::new(RefCell::new(None::<gtk::TreeListRow>));
        disclosure.connect_clicked({
            let tree_row = Rc::clone(&tree_row);
            let refresh_layout = Rc::clone(&context.refresh_layout);
            move |_| {
                let Some(row) = tree_row.borrow().as_ref().cloned() else {
                    return;
                };
                row.set_expanded(!row.is_expanded());
                refresh_layout();
            }
        });

        let row = RowView::new(
            &context.expanded_rows,
            &context.manager,
            &context.refresh_layout,
            context.interactive,
        );
        container.append(&expander);
        container.append(&row.container);

        Self {
            container,
            expander,
            header,
            icon,
            image,
            name,
            count,
            disclosure,
            tree_row,
            expanded_handler: RefCell::new(None),
            notifications: Rc::new(RefCell::new(Vec::new())),
            row,
        }
    }

    pub(super) fn update(&self, tree_row: &gtk::TreeListRow) {
        if let Some((row, handler)) = self.expanded_handler.take() {
            row.disconnect(handler);
        }
        let model_item = tree_row
            .item()
            .and_downcast::<glib::BoxedAnyObject>()
            .expect("notification center model item");
        self.container.remove_css_class("notification-group-start");
        self.container.remove_css_class("notification-group-end");
        self.container
            .remove_css_class("notification-group-collapsed");
        match &*model_item.borrow::<CenterItem>() {
            CenterItem::Group(group) => {
                self.tree_row.replace(Some(tree_row.clone()));
                self.expander.set_list_row(Some(tree_row));
                self.expander.set_visible(true);
                self.header.set_visible(true);
                self.row.container.set_visible(false);
                self.container.add_css_class("notification-group-start");
                self.notifications.replace(
                    group
                        .notifications
                        .iter()
                        .map(|notification| notification.id)
                        .collect(),
                );

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

                sync_expansion(&self.container, &self.disclosure, tree_row);
                let handler = tree_row.connect_expanded_notify({
                    let container = self.container.clone();
                    let disclosure = self.disclosure.clone();
                    move |row| sync_expansion(&container, &disclosure, row)
                });
                self.expanded_handler
                    .replace(Some((tree_row.clone(), handler)));
            }
            CenterItem::Notification { notification, last } => {
                self.tree_row.replace(None);
                self.expander.set_list_row(None::<&gtk::TreeListRow>);
                self.expander.set_visible(false);
                self.header.set_visible(false);
                self.row.container.set_visible(true);
                if *last {
                    self.container.add_css_class("notification-group-end");
                }
                self.row.update(notification);
            }
        }
    }

    pub(super) fn dismiss_target(&self, picked: &gtk::Widget) -> Option<(Option<u32>, Vec<u32>)> {
        if self.header.is_visible()
            && (picked == self.header.upcast_ref::<gtk::Widget>()
                || picked.is_ancestor(&self.header))
        {
            return Some((None, self.notifications.borrow().clone()));
        }
        if self.row.container.is_visible()
            && (picked == self.row.container.upcast_ref::<gtk::Widget>()
                || picked.is_ancestor(&self.row.container))
        {
            return Some((Some(self.row.target.get()), Vec::new()));
        }
        None
    }

    pub(super) fn clear_hover(&self) {
        self.row.surface.remove_css_class("content-hover");
    }

    pub(super) fn refresh_hover(&self) {
        if self.row.container.is_visible() {
            self.row.refresh_hover();
        }
    }
}

fn sync_expansion(container: &gtk::Box, disclosure: &gtk::Button, row: &gtk::TreeListRow) {
    let collapsed = !row.is_expanded();
    disclosure.set_label(if collapsed { "▸" } else { "▾" });
    if collapsed {
        container.add_css_class("notification-group-collapsed");
    } else {
        container.remove_css_class("notification-group-collapsed");
    }
}
