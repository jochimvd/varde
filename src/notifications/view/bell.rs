use std::{cell::RefCell, rc::Rc};

use gtk::{gdk, prelude::*};

use super::super::{Manager, model::Snapshot};

const DOT_SIZE: i32 = 5;
const DOT_RIGHT_OFFSET: i32 = 0;
const DOT_TOP: i32 = 4;

pub(in crate::notifications) struct Bell {
    pub button: gtk::Button,
    label: gtk::Label,
    dot: gtk::Box,
    class: RefCell<String>,
}

impl Bell {
    pub fn new(manager: &Rc<Manager>, app: &gtk::Application) -> Self {
        let button = gtk::Button::builder().focusable(false).build();
        button.add_css_class("module");
        button.add_css_class("notification");

        let label = gtk::Label::new(None);
        let dot = gtk::Box::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .can_target(false)
            .build();
        dot.add_css_class("notification-dot");

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&label));
        overlay.add_overlay(&dot);
        overlay.set_clip_overlay(&dot, false);
        overlay.connect_get_child_position(|overlay, _| {
            Some(gdk::Rectangle::new(
                overlay.width() - DOT_SIZE + DOT_RIGHT_OFFSET,
                DOT_TOP,
                DOT_SIZE,
                DOT_SIZE,
            ))
        });
        button.set_overflow(gtk::Overflow::Visible);
        button.set_child(Some(&overlay));

        button.connect_clicked({
            let app = app.clone();
            move |_| app.activate_action("notifications", None)
        });
        for mouse_button in [2, 3] {
            let click = gtk::GestureClick::new();
            click.set_button(mouse_button);
            click.connect_released({
                let manager = Rc::downgrade(manager);
                move |_, _, _, _| {
                    if let Some(manager) = manager.upgrade() {
                        match mouse_button {
                            2 => manager.toggle_dnd(),
                            3 => manager.clear(),
                            _ => unreachable!(),
                        }
                    }
                }
            });
            button.add_controller(click);
        }

        Self {
            button,
            label,
            dot,
            class: RefCell::new(String::new()),
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        let alt = snapshot.alt();
        self.label.set_text(if snapshot.dnd { "󰂛" } else { "󰂚" });
        self.dot.set_visible(snapshot.count > 0);
        self.button.set_tooltip_text(Some(&snapshot.tooltip()));

        let mut current = self.class.borrow_mut();
        if *current != alt {
            if !current.is_empty() {
                self.button.remove_css_class(&current);
            }
            self.button.add_css_class(alt);
            *current = alt.into();
        }
    }
}
