mod background;
mod bar;

use gtk::prelude::*;

fn main() {
    let app = gtk::Application::builder()
        .application_id("be.jochim.shell")
        .build();

    app.connect_startup(load_styles);
    app.connect_activate(bar::show);
    app.run();
}

fn load_styles(_: &gtk::Application) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("style.css"));

    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display is required"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
