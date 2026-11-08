//! A small pill near the bottom of the screen confirming a change made without
//! looking at the bar, such as a volume key or a muted microphone.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk::{cairo, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

const OSD_NAME: &str = "varde-osd";
const BOTTOM: i32 = 48;
/// Kept inside the surface around the pill so its shadow is not clipped.
const SHADOW_ROOM: i32 = 24;
const TEXT_LIMIT: usize = 40;
const TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub icon: String,
    /// Beside the icon, or above the level when there is one.
    pub text: Option<String>,
    /// A percentage to show as a bar, dimmed when `muted`.
    pub level: Option<u8>,
    pub muted: bool,
}

pub struct Osd {
    view: RefCell<Option<View>>,
    generation: Cell<u64>,
}

struct View {
    window: gtk::ApplicationWindow,
    icon: gtk::Label,
    text: gtk::Label,
    level: gtk::ProgressBar,
    value: gtk::Label,
}

impl Osd {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            view: RefCell::new(None),
            generation: Cell::new(0),
        })
    }

    pub fn start(&self, app: &gtk::Application) {
        self.view.replace(Some(View::new(app)));
    }

    pub fn show(self: &Rc<Self>, shown: &Shown) {
        let view = self.view.borrow();
        let Some(view) = view.as_ref() else {
            return;
        };
        view.set(shown);
        // A window on screen keeps its size; asking for none fits it to the new content.
        view.window.set_default_size(1, 1);
        // Hidden between changes, so each new one maps on the focused monitor.
        view.window.present();

        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let osd = Rc::downgrade(self);
        glib::timeout_add_local_once(TIMEOUT, move || {
            if let Some(osd) = osd.upgrade()
                && osd.generation.get() == generation
                && let Some(view) = osd.view.borrow().as_ref()
            {
                view.window.set_visible(false);
            }
        });
    }
}

impl View {
    fn new(app: &gtk::Application) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .name(OSD_NAME)
            .build();
        window.add_css_class("osd");
        window.init_layer_shell();
        window.set_namespace(Some(OSD_NAME));
        window.set_layer(gtk4_layer_shell::Layer::Overlay);
        window.set_anchor(gtk4_layer_shell::Edge::Bottom, true);
        window.set_margin(gtk4_layer_shell::Edge::Bottom, BOTTOM - SHADOW_ROOM);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::None);
        // Clicks pass through to whatever is underneath.
        window.connect_realize(|window| {
            if let Some(surface) = window.surface() {
                surface.set_input_region(&cairo::Region::create());
            }
        });

        let icon = gtk::Label::new(None);
        icon.add_css_class("osd-icon");
        // Not ellipsized: the label's minimum is then its whole text, which
        // grows the window when a longer text replaces one still on screen.
        let text = gtk::Label::builder().xalign(0.0).build();
        text.add_css_class("osd-text");
        let level = gtk::ProgressBar::new();
        level.add_css_class("osd-level");
        let value = gtk::Label::builder().xalign(1.0).build();
        value.add_css_class("osd-value");

        let middle = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .spacing(4)
            .build();
        middle.append(&text);
        middle.append(&level);
        let pill = gtk::Box::builder()
            .spacing(12)
            .margin_top(SHADOW_ROOM)
            .margin_bottom(SHADOW_ROOM)
            .margin_start(SHADOW_ROOM)
            .margin_end(SHADOW_ROOM)
            .build();
        pill.add_css_class("osd-pill");
        pill.append(&icon);
        pill.append(&middle);
        pill.append(&value);
        window.set_child(Some(&pill));

        Self {
            window,
            icon,
            text,
            level,
            value,
        }
    }

    fn set(&self, shown: &Shown) {
        self.icon.set_text(&shown.icon);
        self.text
            .set_text(&truncate(shown.text.as_deref().unwrap_or_default()));
        self.text.set_visible(shown.text.is_some());
        self.level.set_visible(shown.level.is_some());
        self.value.set_visible(shown.level.is_some());
        if let Some(percent) = shown.level {
            self.level.set_fraction(f64::from(percent.min(100)) / 100.0);
            self.value.set_text(&percent.to_string());
            self.text.add_css_class("caption");
        } else {
            self.text.remove_css_class("caption");
        }
        if shown.muted {
            self.window.add_css_class("muted");
        } else {
            self.window.remove_css_class("muted");
        }
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= TEXT_LIMIT {
        return text.into();
    }
    text.chars().take(TEXT_LIMIT - 1).chain(['…']).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_text() {
        assert_eq!(truncate("Do not disturb off"), "Do not disturb off");
        let long = "x".repeat(TEXT_LIMIT + 5);
        assert_eq!(truncate(&long).chars().count(), TEXT_LIMIT);
        assert!(truncate(&long).ends_with('…'));
    }
}
