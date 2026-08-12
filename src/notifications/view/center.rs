use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gtk::{gdk, glib, prelude::*};

mod item;
mod row;

use item::{ItemContext, ItemView};

use super::{
    super::{
        Manager,
        model::{Group, Notification, Snapshot},
    },
    common::message,
};

const PANEL_WIDTH: i32 = 460;
const PANEL_RIGHT: i32 = PANEL_TOP;
const PANEL_TOP: i32 = 18;
const MIN_CONTENT_HEIGHT: i32 = 98;
const MAX_CONTENT_HEIGHT: i32 = 520;
const GROUP_TRANSITION_DURATION: u32 = 150;
const EXPAND_BUTTON_WIDTH: i32 = 30;
const NOTIFICATION_TEXT_WIDTH: i32 = PANEL_WIDTH - 32;
const PICTURE_TEXT_OFFSET: i32 = 58;
type NotificationIdentity = (u32, u64);
type ItemViews = Rc<RefCell<Vec<(gtk::ListItem, Rc<ItemView>)>>>;
type WeakItemViews = std::rc::Weak<RefCell<Vec<(gtk::ListItem, Rc<ItemView>)>>>;

#[derive(Clone, Eq, PartialEq)]
enum CenterItem {
    Group(Group),
    Notification {
        notification: Notification,
        last: bool,
    },
}

pub(in crate::notifications) struct Center {
    popover: gtk::Popover,
    model: gio::ListStore,
    tree_model: gtk::TreeListModel,
    list: gtk::ListView,
    scroll: gtk::ScrolledWindow,
    item_views: ItemViews,
    group_order: RefCell<Vec<String>>,
    stack: gtk::Stack,
    dnd: gtk::Button,
    clear: gtk::Button,
    expanded_rows: Rc<RefCell<HashSet<NotificationIdentity>>>,
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

        let expanded_rows = Rc::new(RefCell::new(HashSet::new()));
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree_model = gtk::TreeListModel::new(model.clone(), false, false, |object| {
            let item = object
                .downcast_ref::<glib::BoxedAnyObject>()
                .expect("notification center model item")
                .borrow::<CenterItem>();
            let CenterItem::Group(group) = &*item else {
                return None;
            };
            let children = gio::ListStore::new::<glib::BoxedAnyObject>();
            let items = group
                .notifications
                .iter()
                .enumerate()
                .map(|(index, notification)| {
                    glib::BoxedAnyObject::new(CenterItem::Notification {
                        notification: notification.clone(),
                        last: index + 1 == group.notifications.len(),
                    })
                })
                .collect::<Vec<_>>();
            children.splice(0, 0, &items);
            Some(children.upcast())
        });
        let selection = gtk::NoSelection::new(Some(tree_model.clone()));
        let item_views: ItemViews = Rc::new(RefCell::new(Vec::new()));
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .height_request(MIN_CONTENT_HEIGHT)
            .min_content_height(MIN_CONTENT_HEIGHT)
            .max_content_height(MAX_CONTENT_HEIGHT)
            .propagate_natural_height(false)
            .build();
        scroll.vscrollbar().set_can_target(false);
        scroll.add_css_class("notification-center-scroll");
        let refresh_layout: Rc<dyn Fn()> = Rc::new({
            let scroll = scroll.clone();
            let popover = popover.clone();
            let item_views = Rc::downgrade(&item_views);
            let tree_model = tree_model.clone();
            move || schedule_content_resize(&scroll, &popover, &item_views, &tree_model)
        });
        let item_context = Rc::new(ItemContext {
            expanded_rows: Rc::clone(&expanded_rows),
            manager: Rc::downgrade(manager),
            refresh_layout,
            interactive,
        });
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup({
            let item_views = Rc::clone(&item_views);
            let item_context = Rc::clone(&item_context);
            move |_, item| {
                let item = item
                    .downcast_ref::<gtk::ListItem>()
                    .expect("notification center list item");
                item.set_focusable(false);
                let view = Rc::new(ItemView::new(&item_context));
                item.set_child(Some(&view.container));
                item_views.borrow_mut().push((item.clone(), view));
            }
        });
        factory.connect_bind({
            let item_views = Rc::clone(&item_views);
            move |_, item| {
                let item = item
                    .downcast_ref::<gtk::ListItem>()
                    .expect("notification center list item");
                let model_item = item
                    .item()
                    .and_downcast::<gtk::TreeListRow>()
                    .expect("notification center tree row");
                let view = {
                    let views = item_views.borrow();
                    Rc::clone(
                        &views
                            .iter()
                            .find(|(candidate, _)| candidate == item)
                            .expect("notification center item view")
                            .1,
                    )
                };
                view.update(&model_item);
            }
        });
        factory.connect_teardown({
            let item_views = Rc::clone(&item_views);
            move |_, item| {
                let item = item
                    .downcast_ref::<gtk::ListItem>()
                    .expect("notification center list item");
                item.set_child(gtk::Widget::NONE);
                item_views
                    .borrow_mut()
                    .retain(|(candidate, _)| candidate != item);
            }
        });
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .width_request(PANEL_WIDTH)
            .build();
        list.add_css_class("notification-center-list");
        scroll.set_child(Some(&list));
        if interactive {
            let dismiss = gtk::EventControllerLegacy::builder()
                .propagation_phase(gtk::PropagationPhase::Capture)
                .build();
            dismiss.connect_event({
                let item_views = Rc::clone(&item_views);
                let manager = Rc::downgrade(manager);
                let popover = popover.clone();
                move |_, event| {
                    let release = event.downcast_ref::<gdk::ButtonEvent>().filter(|event| {
                        event.event_type() == gdk::EventType::ButtonRelease && event.button() == 3
                    });
                    let Some((x, y)) = release.and_then(|event| event.position()) else {
                        return glib::Propagation::Proceed;
                    };
                    let Some(picked) = popover.pick(x, y, gtk::PickFlags::DEFAULT) else {
                        return glib::Propagation::Proceed;
                    };
                    let target = item_views
                        .borrow()
                        .iter()
                        .find_map(|(_, view)| view.dismiss_target(&picked));
                    if let (Some(manager), Some((notification, group))) =
                        (manager.upgrade(), target)
                    {
                        if let Some(notification) = notification {
                            manager.dismiss(notification);
                        } else {
                            manager.dismiss_group(group);
                        }
                    }
                    glib::Propagation::Proceed
                }
            });
            popover.add_controller(dismiss);
        }
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
            model,
            tree_model,
            list,
            scroll,
            item_views,
            group_order: RefCell::new(Vec::new()),
            stack,
            dnd,
            clear,
            expanded_rows,
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        self.render(snapshot, !self.popover.is_visible());
    }

    fn render(&self, snapshot: &Snapshot, reset_order: bool) {
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

        let expanded = (0..self.model.n_items())
            .filter_map(|position| self.tree_model.child_row(position))
            .filter_map(|row| {
                let item = row.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
                let item = item.borrow::<CenterItem>();
                let CenterItem::Group(group) = &*item else {
                    return None;
                };
                Some((group.key.clone(), row.is_expanded()))
            })
            .collect::<HashMap<_, _>>();
        let items = groups
            .iter()
            .map(|group| CenterItem::Group((*group).clone()))
            .collect::<Vec<_>>();
        let current = (0..self.model.n_items())
            .map(|position| {
                self.model
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .expect("notification center model item")
                    .borrow::<CenterItem>()
                    .clone()
            })
            .collect::<Vec<_>>();
        let prefix = current
            .iter()
            .zip(&items)
            .take_while(|(current, desired)| current == desired)
            .count();
        let suffix = current[prefix..]
            .iter()
            .rev()
            .zip(items[prefix..].iter().rev())
            .take_while(|(current, desired)| current == desired)
            .count();
        let additions = items[prefix..items.len() - suffix]
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect::<Vec<_>>();
        self.model.splice(
            prefix as u32,
            (current.len() - prefix - suffix) as u32,
            &additions,
        );
        for (position, group) in groups.iter().enumerate() {
            if let Some(row) = self.tree_model.child_row(position as u32) {
                row.set_expanded(expanded.get(&group.key).copied().unwrap_or(true));
            }
        }
        self.refresh_after_layout(reset_order);
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
    }

    pub fn show(&self, snapshot: &Snapshot) {
        self.render(snapshot, true);
        for (_, view) in self.item_views.borrow().iter() {
            view.clear_hover();
        }
        self.popover.popup();
    }

    pub fn hide(&self) {
        for (_, view) in self.item_views.borrow().iter() {
            view.clear_hover();
        }
        self.popover.popdown();
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    fn refresh_after_layout(&self, scroll_to_top: bool) {
        let frames = Cell::new(0);
        let item_views = Rc::downgrade(&self.item_views);
        let scroll = self.scroll.clone();
        let popover = self.popover.clone();
        let tree_model = self.tree_model.clone();
        self.list.add_tick_callback(move |_, _| {
            if frames.get() < 2 {
                frames.set(frames.get() + 1);
                return glib::ControlFlow::Continue;
            }
            if let Some(item_views) = item_views.upgrade() {
                for (_, view) in item_views.borrow().iter() {
                    view.refresh_hover();
                }
            }
            let adjustment = scroll.vadjustment();
            if scroll_to_top {
                adjustment.set_value(0.0);
            }
            schedule_content_resize(&scroll, &popover, &item_views, &tree_model);
            glib::ControlFlow::Break
        });
    }
}

fn schedule_content_resize(
    scroll: &gtk::ScrolledWindow,
    popover: &gtk::Popover,
    item_views: &WeakItemViews,
    tree_model: &gtk::TreeListModel,
) {
    let frames = Cell::new(0);
    let scroll = scroll.clone();
    let popover = popover.clone();
    let item_views = item_views.clone();
    let tree_model = tree_model.clone();
    scroll.clone().add_tick_callback(move |_, _| {
        if frames.get() < 2 {
            frames.set(frames.get() + 1);
            return glib::ControlFlow::Continue;
        }
        let height = content_height(&item_views, tree_model.n_items());
        if height.is_none() && scroll.height_request() < MAX_CONTENT_HEIGHT {
            scroll.set_height_request(MAX_CONTENT_HEIGHT);
            scroll.queue_resize();
            popover.present();
            frames.set(0);
            return glib::ControlFlow::Continue;
        }
        scroll.set_min_content_height(MIN_CONTENT_HEIGHT);
        scroll.set_height_request(height.unwrap_or(MAX_CONTENT_HEIGHT));
        scroll.queue_resize();
        popover.present();
        glib::ControlFlow::Break
    });
}

fn content_height(item_views: &WeakItemViews, item_count: u32) -> Option<i32> {
    let Some(item_views) = item_views.upgrade() else {
        return Some(MIN_CONTENT_HEIGHT);
    };
    let mut rows = HashMap::new();
    for (item, view) in item_views.borrow().iter() {
        if item.position() < item_count {
            let (_, natural, _, _) = view
                .container
                .measure(gtk::Orientation::Vertical, PANEL_WIDTH);
            rows.insert(item.position(), natural);
        }
    }
    (rows.len() == item_count as usize).then(|| {
        rows.values()
            .sum::<i32>()
            .clamp(MIN_CONTENT_HEIGHT, MAX_CONTENT_HEIGHT)
    })
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
