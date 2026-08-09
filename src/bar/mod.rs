use std::{cell::Cell, rc::Rc, time::Duration};

use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

mod modules;

use modules::{connectivity, hyprland, services, system, tray};

const HEIGHT: i32 = 32;
const MODULE_GAP: i32 = 5;
const TRAY_REVEAL_GAP: i32 = 10;
pub(super) const HOVER_DELAY: Duration = Duration::from_millis(200);
pub(super) const RETRACT_DELAY: Duration = Duration::from_secs(1);
const BAR_NAME: &str = "varde-bar";

#[derive(Clone, Default)]
pub(super) struct HoverIntent {
    hovered: Rc<Cell<bool>>,
    generation: Rc<Cell<u64>>,
}

impl HoverIntent {
    pub(super) fn enter(&self, action: impl FnOnce() + 'static) {
        self.hovered.set(true);
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let state = self.clone();
        gtk::glib::timeout_add_local_once(HOVER_DELAY, move || {
            if state.hovered.get() && state.generation.get() == generation {
                action();
            }
        });
    }

    pub(super) fn leave(&self) {
        self.hovered.set(false);
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    pub(super) fn is_hovered(&self) -> bool {
        self.hovered.get()
    }

    pub(super) fn retract(&self, action: impl FnOnce() + 'static) {
        let state = self.clone();
        gtk::glib::timeout_add_local_once(RETRACT_DELAY, move || {
            if !state.hovered.get() {
                action();
            }
        });
    }
}

pub fn show(app: &gtk::Application, notifications: &std::rc::Rc<crate::notifications::Manager>) {
    if app
        .windows()
        .iter()
        .any(|window| window.widget_name() == BAR_NAME)
    {
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .name(BAR_NAME)
        .default_height(HEIGHT)
        .height_request(HEIGHT)
        .build();
    window.add_css_class("bar");
    #[cfg(debug_assertions)]
    if std::env::var_os("VARDE_DEBUG_LAYOUT").is_some() {
        window.add_css_class("development");
    }

    window.init_layer_shell();
    window.set_namespace(Some("varde"));
    window.set_layer(gtk4_layer_shell::Layer::Top);
    window.set_anchor(gtk4_layer_shell::Edge::Top, true);
    window.set_anchor(gtk4_layer_shell::Edge::Left, true);
    window.set_anchor(gtk4_layer_shell::Edge::Right, true);
    window.set_exclusive_zone(HEIGHT);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::None);

    let left = region("left", gtk::Align::Fill);
    let center = region("center", gtk::Align::Center);
    let right = region("right", gtk::Align::End);
    left.set_hexpand(true);
    right.set_spacing(MODULE_GAP);
    notifications.set_center_anchor(&window);

    left.append(&hyprland::widget());

    let system = system::widgets();
    let services = services::widgets();
    center.append(&system.center);
    right.append(&tray_group(&services.idle));
    right.append(&connectivity::bluetooth());
    right.append(&connectivity::network());
    right.append(&connectivity::audio());
    right.append(&system.right);
    right.append(&notifications.button(app));
    right.append(&services.privacy);

    let layout = bar_layout(&left, &center, &right);

    window.set_child(Some(&layout));
    window.present();
}

fn tray_group(idle: &gtk::Button) -> gtk::Overlay {
    let revealer = gtk::Revealer::builder()
        .transition_duration(500)
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .halign(gtk::Align::End)
        .build();
    let hover_intent = HoverIntent::default();
    let menu_open = Rc::new(Cell::new(false));
    let tray = tray::widget({
        let revealer = revealer.clone();
        let hover_intent = hover_intent.clone();
        let menu_open = menu_open.clone();
        move |open| {
            menu_open.set(open);
            update_tray_reveal(&revealer, &hover_intent, &menu_open);
        }
    });
    revealer.set_child(Some(&tray));
    idle.add_tick_callback({
        let revealer = revealer.clone();
        move |idle, _| {
            let width = idle.width();
            if width > 0 {
                revealer.set_margin_end(width + TRAY_REVEAL_GAP);
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        }
    });
    let group = gtk::Overlay::new();
    group.set_valign(gtk::Align::Center);
    group.set_child(Some(idle));
    group.add_overlay(&revealer);
    group.set_clip_overlay(&revealer, false);

    let hover = gtk::EventControllerMotion::new();
    hover.connect_enter({
        let revealer = revealer.clone();
        let hover_intent = hover_intent.clone();
        let menu_open = menu_open.clone();
        move |_, _, _| {
            let revealer = revealer.clone();
            let reveal_intent = hover_intent.clone();
            let menu_open = menu_open.clone();
            hover_intent.enter(move || {
                update_tray_reveal(&revealer, &reveal_intent, &menu_open);
            });
        }
    });
    hover.connect_leave({
        let revealer = revealer.clone();
        let hover_intent = hover_intent.clone();
        let menu_open = menu_open.clone();
        move |_| {
            hover_intent.leave();
            update_tray_reveal(&revealer, &hover_intent, &menu_open);
        }
    });
    group.add_controller(hover);
    group
}

fn update_tray_reveal(
    revealer: &gtk::Revealer,
    hover_intent: &HoverIntent,
    menu_open: &Rc<Cell<bool>>,
) {
    if hover_intent.is_hovered() || menu_open.get() {
        revealer.set_reveal_child(true);
        return;
    }
    let revealer = revealer.clone();
    let menu_open = menu_open.clone();
    hover_intent.retract(move || {
        if !menu_open.get() {
            revealer.set_reveal_child(false);
        }
    });
}

fn bar_layout(left: &gtk::Box, center: &gtk::Box, right: &gtk::Box) -> gtk::CenterBox {
    let layout = gtk::CenterBox::new();
    layout.set_start_widget(Some(left));
    layout.set_center_widget(Some(center));
    layout.set_end_widget(Some(right));
    layout
}

fn region(name: &str, align: gtk::Align) -> gtk::Box {
    let region = gtk::Box::builder()
        .halign(align)
        .valign(gtk::Align::Center)
        .build();
    region.add_css_class("region");
    region.add_css_class(name);
    region
}
