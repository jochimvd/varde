//! Keeps every bar's tooltips off while one of the bars' popovers is open.
//!
//! GTK moves a visible tooltip onto a popover as soon as the popover gets any
//! event, focus changes included, and then blocks until Hyprland configures
//! the tooltip there. Under a popover that has not drawn yet, or that Hyprland
//! has just dismissed, it never does, which froze the whole shell.

use std::cell::{Cell, RefCell};

use gtk::{glib, prelude::*};

thread_local! {
    static OPEN_POPOVERS: Cell<u32> = const { Cell::new(0) };
    static SUPPRESSED: RefCell<Vec<glib::WeakRef<gtk::Widget>>> = const { RefCell::new(Vec::new()) };
}

/// A popover for the bars. Every bar popover must come from here, so that the
/// bars' tooltips stay off while it is open.
pub fn popover(position: gtk::PositionType) -> gtk::Popover {
    let popover = gtk::Popover::builder()
        .autohide(true)
        .has_arrow(false)
        .position(position)
        .build();
    popover.connect_map(|_| set_open(OPEN_POPOVERS.get() + 1));
    popover.connect_unmap(|_| set_open(OPEN_POPOVERS.get().saturating_sub(1)));
    popover
}

/// Turns off the tooltips `bar` gained while a popover is open.
pub fn refresh(bar: &impl IsA<gtk::Widget>) {
    if OPEN_POPOVERS.get() > 0 {
        suppress(bar.upcast_ref());
    }
}

fn set_open(open: u32) {
    if (OPEN_POPOVERS.replace(open) > 0) == (open > 0) {
        return;
    }
    if open == 0 {
        for widget in SUPPRESSED.take().iter().filter_map(glib::WeakRef::upgrade) {
            widget.set_has_tooltip(true);
        }
    }
    for window in gtk::Window::list_toplevels() {
        if window.widget_name() == super::BAR_NAME {
            if open > 0 {
                suppress(&window);
            }
            window.trigger_tooltip_query();
        }
    }
}

fn suppress(widget: &gtk::Widget) {
    if widget.has_tooltip() {
        widget.set_has_tooltip(false);
        SUPPRESSED.with_borrow_mut(|suppressed| suppressed.push(widget.downgrade()));
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        suppress(&current);
        child = current.next_sibling();
    }
}
