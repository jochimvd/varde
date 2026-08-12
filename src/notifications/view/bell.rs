use std::{
    cell::{Cell, RefCell},
    f64::consts::TAU,
    rc::Rc,
};

use gtk::{cairo, prelude::*};

use super::super::{Manager, model::Snapshot};

const ICON_SIZE: i32 = 14;
const ICON_HEIGHT: i32 = 18;
const DOT_RADIUS: f64 = 2.5;
const DOT_GAP: f64 = 2.0;
const DOT_TOP: f64 = 2.0;

#[derive(Clone, Copy, Default)]
struct BellState {
    dnd: bool,
    notified: bool,
}

pub(in crate::notifications) struct Bell {
    pub button: gtk::Button,
    icon: gtk::DrawingArea,
    state: Rc<Cell<BellState>>,
    class: RefCell<String>,
}

impl Bell {
    pub fn new(manager: &Rc<Manager>, app: &gtk::Application) -> Self {
        let button = gtk::Button::builder()
            .focusable(false)
            .valign(gtk::Align::Center)
            .build();
        button.set_cursor_from_name(Some("pointer"));
        button.add_css_class("module");
        button.add_css_class("notification");

        let icon = gtk::DrawingArea::builder()
            .content_width(ICON_SIZE)
            .content_height(ICON_HEIGHT)
            .build();
        let state = Rc::new(Cell::new(BellState::default()));
        icon.set_draw_func({
            let state = Rc::clone(&state);
            move |icon, context, width, height| {
                draw_icon(icon, context, width, height, state.get());
            }
        });
        button.set_child(Some(&icon));

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
            icon,
            state,
            class: RefCell::new(String::new()),
        }
    }

    pub fn update(&self, snapshot: &Snapshot) {
        let alt = snapshot.alt();
        self.state.set(BellState {
            dnd: snapshot.dnd,
            notified: snapshot.count > 0,
        });
        self.icon.queue_draw();
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

#[allow(deprecated)]
fn draw_icon(
    icon: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: BellState,
) {
    let glyph = if state.dnd { "󰂛" } else { "󰂚" };
    context.select_font_face(
        "JetBrainsMono Nerd Font Propo",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    context.set_font_size(14.0);

    let extents = context.text_extents(glyph).expect("bell glyph extents");
    context.move_to(
        (f64::from(width) - extents.width()) / 2.0 - extents.x_bearing(),
        (f64::from(height) - extents.height()) / 2.0 - extents.y_bearing(),
    );
    let color = icon.style_context().color();
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
    context.show_text(glyph).expect("draw bell glyph");

    if !state.notified {
        return;
    }

    let dot_x = f64::from(width) - DOT_RADIUS;
    let dot_y = DOT_TOP + DOT_RADIUS;
    context.set_operator(cairo::Operator::Clear);
    context.arc(dot_x, dot_y, DOT_RADIUS + DOT_GAP, 0.0, TAU);
    context.fill().expect("cut notification gap");

    context.set_operator(cairo::Operator::Over);
    let accent = icon
        .style_context()
        .lookup_color("accent_color")
        .expect("accent color is defined");
    context.set_source_rgba(
        f64::from(accent.red()),
        f64::from(accent.green()),
        f64::from(accent.blue()),
        f64::from(accent.alpha()),
    );
    context.arc(dot_x, dot_y, DOT_RADIUS, 0.0, TAU);
    context.fill().expect("draw notification dot");
}
