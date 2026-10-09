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
    pub fn new(manager: &Rc<Manager>) -> Self {
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
            let manager = Rc::downgrade(manager);
            move |_| {
                if let Some(manager) = manager.upgrade() {
                    manager.toggle();
                }
            }
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

    pub fn update(&self, snapshot: &Snapshot, center_open: bool) {
        let alt = snapshot.alt();
        self.state.set(BellState {
            dnd: snapshot.dnd,
            notified: snapshot.count > 0,
        });
        self.icon.queue_draw();
        // GTK chains a tooltip shown while the center opens under the center
        // popover before it has mapped; that popup is never configured and the
        // shell blocks waiting for it. The open center says the same anyway.
        self.button
            .set_tooltip_text((!center_open).then(|| snapshot.tooltip()).as_deref());

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

fn draw_icon(
    icon: &gtk::DrawingArea,
    context: &cairo::Context,
    width: i32,
    height: i32,
    state: BellState,
) {
    let glyph = if state.dnd { "󰂛" } else { "󰂚" };
    let layout = icon.create_pango_layout(Some(glyph));
    let (ink, _) = layout.extents();
    let baseline = layout.baseline();
    let pixels = |units: i32| f64::from(units) / f64::from(gtk::pango::SCALE);
    // Round the ink box out to whole pixels from the baseline, as hinted text
    // extents do, before centering it.
    let left = pixels(ink.x()).floor();
    let right = pixels(ink.x() + ink.width()).ceil();
    let top = pixels(ink.y() - baseline).floor();
    let bottom = pixels(ink.y() + ink.height() - baseline).ceil();
    let x = (f64::from(width) - (right - left)) / 2.0 - left;
    let y = (f64::from(height) - (bottom - top)) / 2.0 - top - pixels(baseline);

    // A snapshot draws the layout in the widget's CSS font through Pango.
    let snapshot = gtk::Snapshot::new();
    snapshot.translate(&gtk::graphene::Point::new(x as f32, y as f32));
    snapshot.append_layout(&layout, &icon.color());
    if let Some(node) = snapshot.to_node() {
        node.draw(context);
    }

    if !state.notified {
        return;
    }
    #[allow(deprecated)]
    let Some(accent) = icon.style_context().lookup_color("accent_color") else {
        return;
    };

    let dot_x = f64::from(width) - DOT_RADIUS;
    let dot_y = DOT_TOP + DOT_RADIUS;
    context.set_operator(cairo::Operator::Clear);
    context.arc(dot_x, dot_y, DOT_RADIUS + DOT_GAP, 0.0, TAU);
    let _ = context.fill();

    context.set_operator(cairo::Operator::Over);
    context.set_source_rgba(
        f64::from(accent.red()),
        f64::from(accent.green()),
        f64::from(accent.blue()),
        f64::from(accent.alpha()),
    );
    context.arc(dot_x, dot_y, DOT_RADIUS, 0.0, TAU);
    let _ = context.fill();
}
