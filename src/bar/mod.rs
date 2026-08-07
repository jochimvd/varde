use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

mod modules;

use modules::{connectivity, hyprland, services, system, tray};

const HEIGHT: i32 = 32;
const MODULE_GAP: i32 = 10;
const TRAY_REVEAL_GAP: i32 = 10;
const BAR_NAME: &str = "varde-bar";

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
    if std::env::var_os("VARDE_DEVELOPMENT").is_some() {
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
    right.set_spacing(MODULE_GAP);

    left.append(&hyprland::widget());

    let system = system::widgets();
    let services = services::widgets();
    center.append(&system.center);
    right.append(&tray_group(&services.idle, &tray::widget()));
    right.append(&connectivity::bluetooth());
    right.append(&connectivity::network());
    right.append(&connectivity::audio());
    right.append(&system.right);
    right.append(&notifications.button(app));
    right.append(&services.privacy);

    let layout = constrained_layout(&left, &center, &right);

    window.set_child(Some(&layout));
    window.present();
}

fn tray_group(idle: &gtk::Button, tray: &gtk::Box) -> gtk::Overlay {
    let revealer = gtk::Revealer::builder()
        .transition_duration(500)
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .halign(gtk::Align::End)
        .child(tray)
        .build();
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
    group.set_child(Some(idle));
    group.add_overlay(&revealer);
    group.set_clip_overlay(&revealer, false);

    let hover = gtk::EventControllerMotion::new();
    hover.connect_enter({
        let revealer = revealer.clone();
        move |_, _, _| revealer.set_reveal_child(true)
    });
    hover.connect_leave(move |_| revealer.set_reveal_child(false));
    group.add_controller(hover);
    group
}

fn constrained_layout(left: &gtk::Box, center: &gtk::Box, right: &gtk::Box) -> gtk::Box {
    use gtk::ConstraintAttribute as Attribute;

    let manager = gtk::ConstraintLayout::new();
    manager
        .add_constraints_from_description(
            ["H:|[left][center][right]|"],
            0,
            0,
            [("left", left), ("center", center), ("right", right)],
        )
        .expect("valid bar layout constraints");
    let align = |target: &gtk::Box, attribute| {
        gtk::Constraint::new(
            Some(target),
            attribute,
            gtk::ConstraintRelation::Eq,
            None::<&gtk::Box>,
            attribute,
            1.0,
            0.0,
            gtk::ffi::GTK_CONSTRAINT_STRENGTH_REQUIRED,
        )
    };
    manager.add_constraint(align(center, Attribute::CenterX));
    for child in [left, center, right] {
        manager.add_constraint(align(child, Attribute::CenterY));
    }

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.append(left);
    layout.append(center);
    layout.append(right);
    layout.set_layout_manager(Some(manager));
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
