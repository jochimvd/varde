use gtk::prelude::*;
use gtk4_layer_shell::LayerShell;

use crate::modules::{connectivity, hyprland::Hyprland, services, system, tray};

const HEIGHT: i32 = 32;
const MODULE_GAP: i32 = 10;
const TRAY_REVEAL_GAP: i32 = 10;

pub fn show(app: &gtk::Application) {
    if !app.windows().is_empty() {
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .default_height(HEIGHT)
        .height_request(HEIGHT)
        .build();
    window.add_css_class("bar");
    if std::env::var_os("SHELL_DEVELOPMENT").is_some() {
        window.add_css_class("development");
    }

    window.init_layer_shell();
    window.set_namespace(Some("shell"));
    window.set_layer(gtk4_layer_shell::Layer::Top);
    window.set_anchor(gtk4_layer_shell::Edge::Top, true);
    window.set_anchor(gtk4_layer_shell::Edge::Left, true);
    window.set_anchor(gtk4_layer_shell::Edge::Right, true);
    window.set_exclusive_zone(HEIGHT);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::None);

    let layout = gtk::Grid::builder().column_homogeneous(true).build();

    let left = region("left", gtk::Align::Start);
    let center = region("center", gtk::Align::Center);
    let right = region("right", gtk::Align::End);
    right.set_spacing(MODULE_GAP);

    let hyprland = Hyprland::new();
    left.append(hyprland.widget());

    let system = system::widgets();
    let services = services::widgets();
    center.append(&system.center);
    right.append(&tray_group(&services.idle, &tray::widget()));
    right.append(&connectivity::bluetooth());
    right.append(&connectivity::network());
    right.append(&connectivity::audio());
    right.append(&system.right);
    right.append(&connectivity::notification());
    right.append(&services.privacy);

    layout.attach(&left, 0, 0, 1, 1);
    layout.attach(&center, 1, 0, 1, 1);
    layout.attach(&right, 2, 0, 1, 1);

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

fn region(name: &str, align: gtk::Align) -> gtk::Box {
    let region = gtk::Box::builder()
        .hexpand(true)
        .halign(align)
        .valign(gtk::Align::Center)
        .build();
    region.add_css_class("region");
    region.add_css_class(name);
    region
}
