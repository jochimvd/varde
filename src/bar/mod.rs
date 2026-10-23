use std::{cell::Cell, rc::Rc, time::Duration};

use gtk::{glib, prelude::*};
use gtk4_layer_shell::LayerShell;

mod modules;
mod tooltips;

use crate::notifications;
use modules::{
    connectivity, hyprland,
    services::{idle, privacy},
    system, tray,
};

const HEIGHT: i32 = 32;
const MODULE_GAP: i32 = 10;
const CENTER_GAP: i32 = 5;
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
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let state = self.clone();
        gtk::glib::timeout_add_local_once(RETRACT_DELAY, move || {
            if !state.hovered.get() && state.generation.get() == generation {
                action();
            }
        });
    }
}

/// Something the modules report, for every bar to show.
enum Msg {
    Monitors,
    Tick,
    Hyprland(hyprland::Update),
    Title(Option<hyprland::TitleOverlay>),
    Network(connectivity::Shown),
    Bluetooth(connectivity::Shown),
    Audio(connectivity::Shown),
    Idle(idle::Inhibition),
    Privacy(privacy::Change),
    Cpu(system::CpuUsage),
    Memory(system::Memory),
    Battery(system::Battery),
    Tray(tray::Event),
    Notifications(notifications::Indicator),
}

/// Where the modules send what they report. Everything reaches the bars
/// through the main loop, so a bar never updates while another update runs.
#[derive(Clone)]
struct Updates(async_channel::Sender<Msg>);

impl Updates {
    fn send(&self, msg: Msg) {
        let _ = self.0.try_send(msg);
    }

    fn forward<T: 'static>(&self, receiver: async_channel::Receiver<T>, msg: fn(T) -> Msg) {
        let updates = self.clone();
        crate::background::listen(receiver, move |value| updates.send(msg(value)));
    }
}

/// What the bars show, the same on every monitor.
#[derive(Default)]
struct State {
    hyprland: hyprland::State,
    title: Option<hyprland::TitleOverlay>,
    network: Option<connectivity::Shown>,
    bluetooth: Option<connectivity::Shown>,
    audio: Option<connectivity::Shown>,
    idle: idle::Inhibition,
    privacy: privacy::Sources,
    cpu: Option<system::CpuUsage>,
    memory: Option<system::Memory>,
    battery: Option<system::Battery>,
    tray: Vec<tray::Item>,
    notifications: Option<notifications::Indicator>,
}

impl State {
    /// Records `msg`, returning whether the bars have anything new to show.
    fn apply(&mut self, msg: &Msg) -> bool {
        match msg {
            Msg::Monitors | Msg::Tick => {}
            Msg::Hyprland(update) => self.hyprland.apply(update),
            Msg::Title(title) => self.title = title.clone(),
            Msg::Network(shown) => self.network = Some(shown.clone()),
            Msg::Bluetooth(shown) => self.bluetooth = Some(shown.clone()),
            Msg::Audio(shown) => self.audio = Some(shown.clone()),
            Msg::Idle(idle) => self.idle = idle.clone(),
            Msg::Privacy(change) => return self.privacy.apply(change),
            Msg::Cpu(usage) => self.cpu = Some(usage.clone()),
            Msg::Memory(memory) => self.memory = Some(memory.clone()),
            Msg::Battery(battery) => self.battery = Some(battery.clone()),
            Msg::Tray(event) => tray::apply(&mut self.tray, event),
            Msg::Notifications(indicator) => self.notifications = Some(indicator.clone()),
        }
        true
    }
}

/// Starts the module workers and keeps a bar on every monitor.
pub fn start(
    app: &gtk::Application,
    notifications: &Rc<notifications::Manager>,
    title: &Rc<crate::title::Manager>,
) {
    let (sender, receiver) = async_channel::unbounded();
    let updates = Updates(sender);
    hyprland::start(title, &updates);
    privacy::start(&updates);
    notifications.connect({
        let updates = updates.clone();
        move |indicator| updates.send(Msg::Notifications(indicator))
    });
    let modules = Modules {
        system: system::System::new(&updates),
        idle: idle::Idle::new(&updates),
        tray: tray::Tray::new(&updates),
        bluetooth: connectivity::Bluetooth::new(&updates),
        network: connectivity::Network::new(&updates),
        audio: connectivity::Audio::new(&updates),
    };
    glib::timeout_add_seconds_local(1, {
        let updates = updates.clone();
        move || {
            updates.send(Msg::Tick);
            glib::ControlFlow::Continue
        }
    });
    let monitors = gtk::gdk::Display::default()
        .expect("a display is required")
        .monitors();
    monitors.connect_items_changed({
        let updates = updates.clone();
        move |_, _, _, _| updates.send(Msg::Monitors)
    });
    updates.send(Msg::Monitors);

    let mut bars = Bars {
        app: app.clone(),
        notifications: notifications.clone(),
        modules,
        monitors,
        updates,
        state: State::default(),
        bars: Vec::new(),
    };
    crate::background::listen(receiver, move |msg| bars.update(msg));
}

/// The modules' handles that bar widgets act through.
struct Modules {
    system: system::System,
    idle: idle::Idle,
    tray: tray::Tray,
    bluetooth: connectivity::Bluetooth,
    network: connectivity::Network,
    audio: connectivity::Audio,
}

struct Bars {
    app: gtk::Application,
    notifications: Rc<notifications::Manager>,
    modules: Modules,
    monitors: gtk::gio::ListModel,
    updates: Updates,
    state: State,
    bars: Vec<Bar>,
}

impl Bars {
    fn update(&mut self, msg: Msg) {
        if let Msg::Monitors = msg {
            self.place();
        }
        if self.state.apply(&msg) {
            for bar in &mut self.bars {
                bar.render(Some(&msg), &self.state);
            }
        }
    }

    /// Gives every monitor a bar and destroys the bars of monitors that went away.
    fn place(&mut self) {
        let current: Vec<gtk::gdk::Monitor> = self
            .monitors
            .iter::<gtk::gdk::Monitor>()
            .filter_map(Result::ok)
            .collect();
        self.bars.retain(|bar| {
            let kept = current.contains(&bar.monitor);
            if !kept {
                #[cfg(debug_assertions)]
                report_survivors(&bar.window);
                bar.window.destroy();
            }
            kept
        });
        for monitor in current {
            if !self.bars.iter().any(|bar| bar.monitor == monitor) {
                let bar = Bar::new(self, monitor);
                self.bars.push(bar);
            }
        }
    }
}

struct Bar {
    window: gtk::ApplicationWindow,
    monitor: gtk::gdk::Monitor,
    hyprland: hyprland::Widgets,
    system: system::Widgets,
    idle: gtk::Button,
    tray: tray::Widget,
    bluetooth: connectivity::Button,
    network: connectivity::Button,
    audio: connectivity::Button,
    notifications: notifications::Bell,
    privacy: privacy::Widget,
}

impl Bar {
    fn new(bars: &Bars, monitor: gtk::gdk::Monitor) -> Self {
        let modules = &bars.modules;
        let window = gtk::ApplicationWindow::builder()
            .application(&bars.app)
            .name(BAR_NAME)
            .default_height(HEIGHT)
            .height_request(HEIGHT)
            .build();
        window.add_css_class("bar");
        #[cfg(debug_assertions)]
        if std::env::var("VARDE_DEBUG_LAYOUT").as_deref() == Ok("1") {
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

        let hyprland = hyprland::Widgets::new();
        left.append(&hyprland.root);

        let system = modules.system.widgets();
        let idle = modules.idle.widget();
        let (tray_group, tray) = tray_group(&idle, &modules.tray);
        let bluetooth = modules.bluetooth.widget();
        let network = modules.network.widget();
        let audio = modules.audio.widget();
        let notifications = bars.notifications.bell();
        let privacy = privacy::widget();
        center.append(&system.center);
        right.append(&tray_group);
        right.append(&bluetooth.button);
        right.append(&network.button);
        right.append(&audio.button);
        right.append(&system.right);
        right.append(&notifications.button);
        right.append(&privacy.root);

        window.set_child(Some(&bar_layout(&left, &center, &right)));
        window.set_monitor(Some(&monitor));
        // A monitor plugged in while running is listed before it is named.
        monitor.connect_connector_notify({
            let updates = bars.updates.clone();
            move |_| updates.send(Msg::Monitors)
        });

        let mut bar = Self {
            window,
            monitor,
            hyprland,
            system,
            idle,
            tray,
            bluetooth,
            network,
            audio,
            notifications,
            privacy,
        };
        bar.render(None, &bars.state);
        bar.window.present();
        bar
    }

    /// Shows what `msg` changed in `state`, or all of it for a new bar.
    fn render(&mut self, msg: Option<&Msg>, state: &State) {
        let changed = |changed: fn(&Msg) -> bool| msg.is_none_or(changed);
        if let Some(Msg::Hyprland(hyprland::Update::Title(title))) = msg {
            self.hyprland.show_title(title);
        } else if changed(|msg| matches!(msg, Msg::Monitors | Msg::Hyprland(_))) {
            self.hyprland.show(&state.hyprland, &self.connector());
        }
        if changed(|msg| matches!(msg, Msg::Title(_))) {
            self.hyprland.show_overlay(state.title.clone());
        }
        if changed(|msg| matches!(msg, Msg::Tick)) {
            self.system.tick();
        }
        if changed(|msg| matches!(msg, Msg::Network(_)))
            && let Some(shown) = &state.network
        {
            self.network.show(shown);
        }
        if changed(|msg| matches!(msg, Msg::Bluetooth(_)))
            && let Some(shown) = &state.bluetooth
        {
            self.bluetooth.show(shown);
        }
        if changed(|msg| matches!(msg, Msg::Audio(_)))
            && let Some(shown) = &state.audio
        {
            self.audio.show(shown);
        }
        if changed(|msg| matches!(msg, Msg::Idle(_))) {
            idle::show(&self.idle, &state.idle);
        }
        if changed(|msg| matches!(msg, Msg::Privacy(_))) {
            self.privacy.show(state.privacy.shown());
        }
        if changed(|msg| matches!(msg, Msg::Cpu(_)))
            && let Some(cpu) = &state.cpu
        {
            self.system.show_cpu(cpu);
        }
        if changed(|msg| matches!(msg, Msg::Memory(_)))
            && let Some(memory) = &state.memory
        {
            self.system.show_memory(memory);
        }
        if changed(|msg| matches!(msg, Msg::Battery(_)))
            && let Some(battery) = &state.battery
        {
            self.system.show_battery(battery);
        }
        if changed(|msg| matches!(msg, Msg::Tray(_))) {
            let item = match msg {
                Some(Msg::Tray(event)) => Some(event.id()),
                _ => None,
            };
            self.tray.show(&state.tray, item);
        }
        if changed(|msg| matches!(msg, Msg::Notifications(_)))
            && let Some(indicator) = &state.notifications
        {
            self.notifications.show(indicator);
        }
        tooltips::refresh(&self.window);
    }

    fn connector(&self) -> String {
        self.monitor.connector().map(Into::into).unwrap_or_default()
    }
}

/// Reports the widgets of a destroyed bar that something still keeps alive,
/// such as a handler holding its own widget.
#[cfg(debug_assertions)]
fn report_survivors(window: &gtk::ApplicationWindow) {
    fn collect(widget: &gtk::Widget, widgets: &mut Vec<glib::WeakRef<gtk::Widget>>) {
        widgets.push(widget.downgrade());
        let mut child = widget.first_child();
        while let Some(current) = child {
            collect(&current, widgets);
            child = current.next_sibling();
        }
    }
    let mut widgets = Vec::new();
    collect(window.upcast_ref(), &mut widgets);
    glib::timeout_add_local_once(Duration::from_secs(2), move || {
        for widget in widgets.iter().filter_map(glib::WeakRef::upgrade) {
            eprintln!(
                "varde: {} {:?} outlived its bar",
                widget.type_().name(),
                widget.css_classes()
            );
        }
    });
}

fn tray_group(idle: &gtk::Button, tray: &tray::Tray) -> (gtk::Overlay, tray::Widget) {
    let revealer = gtk::Revealer::builder()
        .transition_duration(500)
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .halign(gtk::Align::End)
        .build();
    let hover_intent = HoverIntent::default();
    let menu_open = Rc::new(Cell::new(false));
    let tray = tray.widget({
        // The tray menus that report through this live inside the revealer.
        let revealer = revealer.downgrade();
        let hover_intent = hover_intent.clone();
        let menu_open = menu_open.clone();
        move |open| {
            menu_open.set(open);
            if let Some(revealer) = revealer.upgrade() {
                update_tray_reveal(&revealer, &hover_intent, &menu_open);
            }
        }
    });
    revealer.set_child(Some(&tray.root));
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
    (group, tray)
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
