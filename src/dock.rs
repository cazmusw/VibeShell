use crate::{
    dock_model::{self, AutoHide, Event, HoverFrame, HoverMotion, WindowIndicator},
    settings::{self, DockEdge},
    trash::TrashState,
    zoom_icon::ZoomIcon,
};
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

pub struct Item {
    pub id: String,
    pub name: String,
    pub custom_icon: Option<String>,
    pub pinned: bool,
    pub windows: usize,
    pub active: bool,
    pub pending: bool,
}
pub enum Action {
    Activate(String),
    Hover(String, gtk::Button),
    HoverLeave,
    CancelHover,
    Menu(String, gtk::Button),
    Reorder(String, String),
    DesktopDrop(gdk::Drop, String),
    Applications,
    OpenTrash,
    ConfirmEmptyTrash,
}
struct Button {
    id: String,
    custom_icon: Option<String>,
    button: gtk::Button,
    icon: ZoomIcon,
    indicator: Indicator,
    content: gtk::Box,
    pinned: Cell<bool>,
}
impl Button {
    fn set_edge(&self, edge: DockEdge) {
        self.icon.set_edge(edge);
        self.indicator.set_vertical(edge.vertical());
        self.content.set_orientation(if edge.vertical() {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        });
        self.content.set_halign(if edge.vertical() {
            gtk::Align::Center
        } else {
            gtk::Align::Fill
        });
        if edge == DockEdge::Left {
            self.content
                .reorder_child_after(&self.indicator.area, gtk::Widget::NONE);
        } else {
            self.content
                .reorder_child_after(&self.icon, gtk::Widget::NONE);
        }
    }
}

struct Indicator {
    area: gtk::DrawingArea,
    state: Rc<Cell<WindowIndicator>>,
}

impl Indicator {
    fn new() -> Self {
        let area = gtk::DrawingArea::builder()
            .accessible_role(gtk::AccessibleRole::Presentation)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .can_target(false)
            .build();
        area.add_css_class("dock-window-indicator");
        let state = Rc::new(Cell::new(WindowIndicator::Empty));
        let drawn = state.clone();
        area.set_draw_func(move |area, context, width, height| {
            let center_x = f64::from(width) / 2.;
            let center_y = f64::from(height) / 2.;
            let position = |offset| {
                if width < height {
                    (center_x, center_y + offset)
                } else {
                    (center_x + offset, center_y)
                }
            };
            context.set_source_color(&area.color());
            match drawn.get() {
                WindowIndicator::Empty => {}
                WindowIndicator::Pending => {
                    context.arc(center_x, center_y, 2.5, 0., std::f64::consts::TAU);
                    context.set_line_width(1.25);
                    let _ = context.stroke();
                }
                WindowIndicator::Dots { count, overflow } => {
                    let span = f64::from(count + u8::from(overflow) - 1) * 8.;
                    for index in 0..count {
                        let (x, y) = position(f64::from(index) * 8. - span / 2.);
                        context.arc(x, y, 2.5, 0., std::f64::consts::TAU);
                        let _ = context.fill();
                    }
                    if overflow {
                        let (x, y) = position(span / 2.);
                        context.move_to(x - 2.5, y);
                        context.line_to(x + 2.5, y);
                        context.move_to(x, y - 2.5);
                        context.line_to(x, y + 2.5);
                        context.set_line_width(2.);
                        let _ = context.stroke();
                    }
                }
            }
        });
        Self { area, state }
    }

    fn set_vertical(&self, vertical: bool) {
        let (width, height) = if vertical { (16, 32) } else { (32, 16) };
        self.area.set_content_width(width);
        self.area.set_content_height(height);
    }

    fn update(&self, windows: usize, pending: bool) {
        let next = WindowIndicator::new(windows, pending);
        if self.state.replace(next) != next {
            self.area.queue_draw();
        }
    }
}
type Callback = Rc<dyn Fn(Action)>;
pub(crate) const FILE_DROP_MIME: &str = "text/uri-list";
struct Inner {
    window: gtk::ApplicationWindow,
    trigger: gtk::ApplicationWindow,
    root: gtk::Overlay,
    background: gtk::Box,
    spacer: gtk::Box,
    edge_paint: gtk::DrawingArea,
    body: ZoomIcon,
    items: gtk::Box,
    divider: gtk::Separator,
    scroll: gtk::ScrolledWindow,
    buttons: RefCell<Vec<Button>>,
    trash_button: RefCell<Option<gtk::Button>>,
    trash_state: Cell<TrashState>,
    callback: RefCell<Option<Callback>>,
    config: RefCell<settings::Dock>,
    reduced: Cell<bool>,
    editable: Cell<bool>,
    available: Cell<bool>,
    model: RefCell<AutoHide>,
    start: Instant,
    timer: RefCell<Option<glib::SourceId>>,
    timer_deadline: Cell<Option<u64>>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
    settle: Cell<u32>,
    pointer: Cell<Option<f64>>,
    hover: RefCell<HoverMotion>,
    frames: Cell<u64>,
    layout_revision: Cell<u64>,
    actual_size: Cell<u32>,
    monitor_size: Cell<(i32, i32)>,
}
#[derive(Clone)]
pub struct Dock(Rc<Inner>);
impl Dock {
    pub fn new(app: &gtk::Application, monitor: Option<&gdk::Monitor>) -> Self {
        let window = gtk::ApplicationWindow::builder().application(app).build();
        window.add_css_class("mosaic-dock");
        window.init_layer_shell();
        window.set_namespace(Some("mosaic-dock"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, 12);
        window.auto_exclusive_zone_enable();
        let trigger = gtk::ApplicationWindow::builder().application(app).build();
        trigger.add_css_class("dock-trigger");
        trigger.init_layer_shell();
        trigger.set_namespace(Some("mosaic-dock-edge"));
        trigger.set_layer(Layer::Top);
        trigger.set_keyboard_mode(KeyboardMode::None);
        trigger.set_anchor(Edge::Bottom, true);
        trigger.set_exclusive_zone(0);
        // An empty transparent widget has no render node and GTK never attaches
        // a buffer. A transparent Cairo node makes this layer surface map.
        let edge_paint = gtk::DrawingArea::new();
        edge_paint.set_content_height(2);
        edge_paint.set_content_width(1);
        edge_paint.set_draw_func(|_, context, _, _| {
            context.set_source_rgba(0., 0., 0., 0.);
            let _ = context.paint();
        });
        trigger.set_child(Some(&edge_paint));
        trigger.set_default_size(200, 2);
        let monitor_size = monitor.map_or((1280, 720), |monitor| {
            let geometry = monitor.geometry();
            (geometry.width(), geometry.height())
        });
        if let Some(m) = monitor {
            window.set_monitor(Some(m));
            trigger.set_monitor(Some(m));
        }
        let root = gtk::Overlay::new();
        let background = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        background.append(&spacer);
        let surface = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        surface.add_css_class("dock-surface");
        surface.set_can_target(false);
        // React to the GTK allocation, which can arrive after the Wayland resize.
        let allocation = gtk::DrawingArea::new();
        allocation.set_hexpand(true);
        allocation.set_vexpand(true);
        allocation.set_can_target(false);
        surface.append(&allocation);
        let body = ZoomIcon::backdrop(&surface);
        body.set_height_request(88);
        body.set_margin_start(16);
        body.set_margin_end(16);
        background.append(&body);
        root.set_child(Some(&background));
        let items = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let divider = gtk::Separator::new(gtk::Orientation::Vertical);
        divider.add_css_class("dock-app-divider");
        divider.set_can_target(false);
        divider.set_valign(gtk::Align::Center);
        divider.set_size_request(1, 32);
        items.set_margin_top(26);
        items.set_margin_bottom(8);
        items.set_margin_start(24);
        items.set_margin_end(24);
        items.set_overflow(gtk::Overflow::Visible);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .max_content_width((monitor_size.0 - 24).max(1))
            .child(&items)
            .build();
        scroll.add_css_class("dock-scroll");
        root.add_overlay(&scroll);
        root.set_measure_overlay(&scroll, true);
        root.set_clip_overlay(&scroll, false);
        window.set_child(Some(&root));
        let inner = Rc::new(Inner {
            window,
            trigger,
            root,
            background,
            spacer,
            edge_paint,
            body,
            items,
            divider,
            scroll,
            buttons: RefCell::new(Vec::new()),
            trash_button: RefCell::new(None),
            trash_state: Cell::new(TrashState::Unavailable),
            callback: RefCell::new(None),
            config: RefCell::new(settings::Dock::default()),
            reduced: Cell::new(false),
            editable: Cell::new(true),
            available: Cell::new(monitor.is_some()),
            model: RefCell::new(AutoHide::new(false, 150, 700, 0)),
            start: Instant::now(),
            timer: RefCell::new(None),
            timer_deadline: Cell::new(None),
            tick: RefCell::new(None),
            settle: Cell::new(0),
            pointer: Cell::new(None),
            hover: RefCell::default(),
            frames: Cell::new(0),
            layout_revision: Cell::new(0),
            actual_size: Cell::new(48),
            monitor_size: Cell::new(monitor_size),
        });
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&inner);
        motion.connect_enter(move |_, x, y| {
            if let Some(i) = weak.upgrade() {
                i.event(Event::Enter);
                i.pointer.set(Some(i.along_axis(x, y)));
                i.schedule_hover();
            }
        });
        let weak = Rc::downgrade(&inner);
        motion.connect_motion(move |_, x, y| {
            if let Some(i) = weak.upgrade() {
                let axis = Some(i.along_axis(x, y));
                if i.pointer.replace(axis) != axis {
                    i.schedule_hover();
                }
            }
        });
        let weak = Rc::downgrade(&inner);
        motion.connect_leave(move |_| {
            if let Some(i) = weak.upgrade() {
                i.pointer.set(None);
                i.schedule_hover();
                i.event(Event::Leave);
            }
        });
        inner.root.add_controller(motion);
        let edge = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&inner);
        edge.connect_enter(move |_, _, _| {
            if let Some(i) = weak.upgrade() {
                i.event(Event::EdgeEnter);
            }
        });
        let weak = Rc::downgrade(&inner);
        edge.connect_leave(move |_| {
            if let Some(i) = weak.upgrade() {
                i.event(Event::EdgeLeave);
            }
        });
        inner.trigger.add_controller(edge);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&inner);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(i) = weak.upgrade() {
                    i.window.set_keyboard_mode(KeyboardMode::None);
                    i.event(Event::Keyboard(false));
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        inner.window.add_controller(keys);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&inner);
        click.connect_pressed(move |gesture, _, x, y| {
            if let Some(i) = weak.upgrade() {
                i.outside_click(gesture, x, y);
            }
        });
        inner.root.add_controller(click);
        let weak = Rc::downgrade(&inner);
        inner.scroll.hadjustment().connect_value_changed(move |_| {
            if let Some(i) = weak.upgrade() {
                i.kick();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.scroll.vadjustment().connect_value_changed(move |_| {
            if let Some(i) = weak.upgrade() {
                i.kick();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.root.connect_map(move |_| {
            if let Some(i) = weak.upgrade() {
                i.kick();
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.root.connect_unmap(move |_| {
            if let Some(i) = weak.upgrade() {
                i.pointer.set(None);
                i.reset_hover();
            }
        });
        let weak = Rc::downgrade(&inner);
        allocation.connect_resize(move |_, _, _| {
            if let Some(i) = weak.upgrade() {
                i.kick();
            }
        });
        inner.install_pin_drop();
        inner.install_file_drop();
        Self(inner)
    }
    pub fn window(&self) -> &gtk::ApplicationWindow {
        &self.0.window
    }
    pub fn trigger(&self) -> &gtk::ApplicationWindow {
        &self.0.trigger
    }
    pub fn edge(&self) -> DockEdge {
        self.0.config.borrow().edge
    }
    pub fn popup_position(&self) -> gtk::PositionType {
        match self.edge() {
            DockEdge::Bottom => gtk::PositionType::Top,
            DockEdge::Left => gtk::PositionType::Right,
            DockEdge::Right => gtk::PositionType::Left,
        }
    }
    pub fn button(&self, id: &str) -> Option<gtk::Button> {
        self.0
            .buttons
            .borrow()
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.button.clone())
    }
    pub fn release_focus(&self) {
        self.0.window.set_keyboard_mode(KeyboardMode::None);
        self.0.event(Event::Keyboard(false));
    }
    pub fn callback(&self, callback: impl Fn(Action) + 'static) {
        *self.0.callback.borrow_mut() = Some(Rc::new(callback));
    }
    pub fn set_monitor(&self, monitor: Option<&gdk::Monitor>) {
        self.release_focus();
        self.0.window.set_visible(false);
        self.0.trigger.set_visible(false);
        self.0.available.set(monitor.is_some());
        self.0.pointer.set(None);
        {
            let mut model = self.0.model.borrow_mut();
            for event in [
                Event::Leave,
                Event::EdgeLeave,
                Event::Menu(false),
                Event::Drag(false),
            ] {
                model.event(event, self.0.now());
            }
        }
        self.0.window.set_monitor(monitor);
        self.0.trigger.set_monitor(monitor);
        if let Some(m) = monitor {
            let geometry = m.geometry();
            self.0
                .monitor_size
                .set((geometry.width(), geometry.height()));
            self.0.resize();
        }
        self.0.schedule_hover();
        self.0.sync();
    }
    pub fn configure(&self, config: &settings::Dock, reduced: bool, editable: bool) {
        let edge_changed = self.0.config.borrow().edge != config.edge;
        *self.0.config.borrow_mut() = config.clone();
        self.0.reduced.set(reduced);
        self.0.editable.set(editable);
        if edge_changed {
            self.0.window.set_keyboard_mode(KeyboardMode::None);
            self.0.window.set_visible(false);
            self.0.trigger.set_visible(false);
            self.0.pointer.set(None);
            let mut model = self.0.model.borrow_mut();
            let now = self.0.now();
            for event in [
                Event::Leave,
                Event::EdgeLeave,
                Event::Drag(false),
                Event::Keyboard(false),
            ] {
                model.event(event, now);
            }
            drop(model);
            self.0.set_edge(config.edge);
        }
        if config.auto_hide {
            self.0.window.set_exclusive_zone(0);
        } else {
            self.0.window.auto_exclusive_zone_enable();
        }
        self.0.model.borrow_mut().configure(
            config.auto_hide,
            config.show_delay_ms,
            config.hide_delay_ms,
            self.0.now(),
        );
        self.0.resize();
        self.0.sync();
        self.0.schedule_hover();
    }
    pub fn fullscreen(&self, value: bool) {
        if self.0.model.borrow().fullscreen != value {
            self.0.event(Event::Fullscreen(value));
        }
    }
    pub fn menu(&self, value: bool) {
        self.0.event(Event::Menu(value));
    }
    pub fn show(&self, focus: bool) {
        self.0.event(Event::Show);
        if focus {
            self.0.window.set_keyboard_mode(KeyboardMode::Exclusive);
            self.0.event(Event::Keyboard(true));
            if let Some(b) = self.0.buttons.borrow().first() {
                b.button.grab_focus();
            }
        }
    }
    pub fn update(
        &self,
        items: Vec<Item>,
        image: impl Fn(&Item, i32) -> gtk::Widget,
        before_rebuild: impl FnOnce(),
    ) {
        self.0.update(items, image, before_rebuild);
    }
    pub fn set_trash_state(&self, state: TrashState) {
        self.0.trash_state.set(state);
        if let Some(button) = self.0.trash_button.borrow().as_ref() {
            Self::update_trash_button(button, state);
        }
    }
    fn update_trash_button(button: &gtk::Button, state: TrashState) {
        let (icon, text) = match state {
            TrashState::Unavailable => ("user-trash-symbolic", "Корзина недоступна"),
            TrashState::Empty => ("user-trash-symbolic", "Корзина пуста"),
            TrashState::Full => ("user-trash-full-symbolic", "В корзине есть файлы"),
            TrashState::Clearing => ("user-trash-full-symbolic", "Корзина очищается"),
        };
        button.set_icon_name(icon);
        button.set_tooltip_text(Some(text));
        button.update_property(&[gtk::accessible::Property::Label(text)]);
    }
    pub fn info(&self) -> crate::ipc::DockInfo {
        let m = self.0.model.borrow();
        crate::ipc::DockInfo {
            visible: m.visible && self.0.available.get(),
            auto_hide: m.enabled,
            icon_size: self.0.actual_size.get(),
            reduced_motion: self.0.reduced.get(),
            magnification: self.0.config.borrow().magnification,
            animation_active: self.0.tick.borrow().is_some(),
            frames: self.0.frames.get(),
            layout_revision: self.0.layout_revision.get(),
            width: self.0.window.width(),
            height: self.0.window.height(),
            exclusive_zone: self.0.window.exclusive_zone(),
            max_scale: self
                .0
                .buttons
                .borrow()
                .iter()
                .map(|b| b.icon.scale())
                .fold(1., f64::max),
            trigger_mapped: self.0.trigger.is_mapped(),
            trigger_width: self.0.trigger.width(),
            trigger_height: self.0.trigger.height(),
        }
    }
}
impl Inner {
    fn append_trash_button(self: &Rc<Self>) {
        let button = gtk::Button::new();
        button.add_css_class("dock-grid");
        button.add_css_class("dock-trash");
        Dock::update_trash_button(&button, self.trash_state.get());
        let weak = Rc::downgrade(self);
        button.connect_clicked(move |_| {
            if let Some(dock) = weak.upgrade() {
                dock.emit(Action::OpenTrash);
            }
        });
        let right = gtk::GestureClick::new();
        right.set_button(3);
        let weak = Rc::downgrade(self);
        right.connect_released(move |_, _, _, _| {
            if let Some(dock) = weak.upgrade() {
                dock.emit(Action::ConfirmEmptyTrash);
            }
        });
        button.add_controller(right);
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if (key == gdk::Key::Menu
                || (key == gdk::Key::F10 && modifiers.contains(gdk::ModifierType::SHIFT_MASK)))
                && let Some(dock) = weak.upgrade()
            {
                dock.emit(Action::ConfirmEmptyTrash);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        button.add_controller(keys);
        self.items.append(&button);
        *self.trash_button.borrow_mut() = Some(button);
    }
    fn install_file_drop(self: &Rc<Self>) {
        let format = gdk::ContentFormats::new(&[FILE_DROP_MIME]);
        let target = gtk::DropTargetAsync::new(Some(format), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        target.connect_accept(move |_, drop| {
            weak.upgrade().is_some_and(|i| i.editable.get())
                && drop.actions().contains(gdk::DragAction::COPY)
                && drop.formats().contain_mime_type(FILE_DROP_MIME)
        });
        let weak = Rc::downgrade(self);
        target.connect_drag_enter(move |_, _, _, _| {
            if let Some(i) = weak.upgrade() {
                i.emit(Action::CancelHover);
                i.event(Event::Drag(true));
            }
            gdk::DragAction::COPY
        });
        let weak = Rc::downgrade(self);
        target.connect_drag_leave(move |_, _| {
            if let Some(i) = weak.upgrade() {
                i.event(Event::Drag(false));
            }
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, drop, x, y| {
            let Some(i) = weak.upgrade() else {
                return false;
            };
            i.event(Event::Drag(false));
            if !i.editable.get() {
                return false;
            }
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let before = i
                .buttons
                .borrow()
                .iter()
                .find(|b| {
                    b.pinned.get()
                        && b.button
                            .compute_bounds(&i.root)
                            .is_some_and(|rect| rect.contains_point(&point))
                })
                .map(|b| b.id.clone())
                .unwrap_or_default();
            i.emit(Action::DesktopDrop(drop.clone(), before));
            true
        });
        self.root.add_controller(target);

        let edge_motion = gtk::DropControllerMotion::new();
        let weak = Rc::downgrade(self);
        edge_motion.connect_enter(move |motion, _, _| {
            if let Some(i) = weak.upgrade()
                && i.editable.get()
                && motion.drop().is_some_and(|drop| {
                    drop.actions().contains(gdk::DragAction::COPY)
                        && drop.formats().contain_mime_type(FILE_DROP_MIME)
                })
            {
                i.event(Event::EdgeEnter);
            }
        });
        let weak = Rc::downgrade(self);
        edge_motion.connect_leave(move |_| {
            if let Some(i) = weak.upgrade() {
                i.event(Event::EdgeLeave);
            }
        });
        self.trigger.add_controller(edge_motion);
    }

    fn along_axis(&self, x: f64, y: f64) -> f64 {
        if self.config.borrow().edge.vertical() {
            y
        } else {
            x
        }
    }
    fn set_edge(&self, edge: DockEdge) {
        for (choice, anchor) in [
            (DockEdge::Bottom, Edge::Bottom),
            (DockEdge::Left, Edge::Left),
            (DockEdge::Right, Edge::Right),
        ] {
            self.window.set_anchor(anchor, choice == edge);
            self.window
                .set_margin(anchor, if choice == edge { 12 } else { 0 });
            self.trigger.set_anchor(anchor, choice == edge);
        }
        let side = edge.vertical();
        let axis = if side {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        };
        let cross = if side {
            gtk::Orientation::Horizontal
        } else {
            gtk::Orientation::Vertical
        };
        self.items.set_orientation(axis);
        self.divider.set_orientation(cross);
        self.divider.set_halign(if side {
            gtk::Align::Center
        } else {
            gtk::Align::Fill
        });
        self.divider.set_valign(if side {
            gtk::Align::Fill
        } else {
            gtk::Align::Center
        });
        self.divider
            .set_size_request(if side { 32 } else { 1 }, if side { 1 } else { 32 });
        self.background.set_orientation(cross);
        self.body.set_edge(edge);
        let direction = if side {
            gtk::TextDirection::Ltr
        } else {
            gtk::TextDirection::None
        };
        self.items.set_direction(direction);
        self.background.set_direction(direction);
        self.spacer.set_hexpand(side);
        self.spacer.set_vexpand(!side);
        if edge == DockEdge::Left {
            self.background
                .reorder_child_after(&self.body, gtk::Widget::NONE);
        } else {
            self.background
                .reorder_child_after(&self.spacer, gtk::Widget::NONE);
        }
        self.edge_paint.set_content_width(if side { 2 } else { 1 });
        self.edge_paint.set_content_height(if side { 1 } else { 2 });
        self.scroll.set_hscrollbar_policy(if side {
            gtk::PolicyType::Never
        } else {
            gtk::PolicyType::Automatic
        });
        self.scroll.set_vscrollbar_policy(if side {
            gtk::PolicyType::Automatic
        } else {
            gtk::PolicyType::Never
        });
        self.scroll.set_propagate_natural_width(!side);
        self.scroll.set_propagate_natural_height(side);
        for button in self.buttons.borrow().iter() {
            button.set_edge(edge);
        }
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn emit(&self, action: Action) {
        let callback = self.callback.borrow().clone();
        if let Some(callback) = callback {
            callback(action);
        }
    }
    fn event(self: &Rc<Self>, event: Event) {
        self.model.borrow_mut().event(event, self.now());
        self.sync();
    }
    fn sync(self: &Rc<Self>) {
        let (visible, trigger, deadline) = {
            let m = self.model.borrow();
            (m.visible, m.enabled && !m.fullscreen, m.deadline)
        };
        self.window.set_visible(visible && self.available.get());
        self.trigger.set_visible(trigger && self.available.get());
        if self.timer_deadline.replace(deadline) != deadline {
            if let Some(timer) = self.timer.borrow_mut().take() {
                timer.remove();
            }
            if let Some(deadline) = deadline {
                let weak = Rc::downgrade(self);
                let timer = glib::timeout_add_local_once(
                    Duration::from_millis(deadline.saturating_sub(self.now())),
                    move || {
                        if let Some(i) = weak.upgrade() {
                            i.timer.borrow_mut().take();
                            i.timer_deadline.set(None);
                            i.event(Event::Timer);
                        }
                    },
                );
                *self.timer.borrow_mut() = Some(timer);
            }
        }
        if visible {
            self.kick();
        }
    }
    fn update(
        self: &Rc<Self>,
        items: Vec<Item>,
        image: impl Fn(&Item, i32) -> gtk::Widget,
        before_rebuild: impl FnOnce(),
    ) {
        let same = self.layout_revision.get() > 0
            && self
                .buttons
                .borrow()
                .iter()
                .map(|b| (b.id.as_str(), b.pinned.get(), &b.custom_icon))
                .eq(items
                    .iter()
                    .map(|i| (i.id.as_str(), i.pinned, &i.custom_icon)));
        if !same {
            // Popovers must be detached while their old anchor is still alive.
            before_rebuild();
            while let Some(child) = self.items.first_child() {
                self.items.remove(&child);
            }
            self.buttons.borrow_mut().clear();
            self.layout_revision.set(self.layout_revision.get() + 1);
            let divider = dock_model::divider_index(items.iter().map(|item| item.pinned));
            for (index, item) in items.iter().enumerate() {
                if divider == Some(index) {
                    self.items.append(&self.divider);
                }
                let button = gtk::Button::new();
                button.add_css_class("dock-item");
                button.set_overflow(gtk::Overflow::Visible);
                let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
                col.set_overflow(gtk::Overflow::Visible);
                let size = self.actual_size.get() as i32;
                let icon = ZoomIcon::new(&image(item, 80), size);
                icon.set_halign(gtk::Align::Center);
                let indicator = Indicator::new();
                col.append(&icon);
                col.append(&indicator.area);
                button.set_child(Some(&col));
                let id = item.id.clone();
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(i) = weak.upgrade() {
                        i.emit(Action::Activate(id.clone()));
                    }
                });
                let hover = gtk::EventControllerMotion::new();
                let weak = Rc::downgrade(self);
                let id = item.id.clone();
                hover.connect_enter(move |controller, _, _| {
                    if let Some(i) = weak.upgrade()
                        && let Some(button) = controller.widget().and_downcast::<gtk::Button>()
                    {
                        i.emit(Action::Hover(id.clone(), button));
                    }
                });
                let weak = Rc::downgrade(self);
                hover.connect_leave(move |_| {
                    if let Some(i) = weak.upgrade() {
                        i.emit(Action::HoverLeave);
                    }
                });
                button.add_controller(hover);
                let right = gtk::GestureClick::new();
                right.set_button(3);
                let id = item.id.clone();
                let weak = Rc::downgrade(self);
                let b = button.downgrade();
                right.connect_pressed(move |_, _, _, _| {
                    if let (Some(i), Some(b)) = (weak.upgrade(), b.upgrade()) {
                        i.emit(Action::Menu(id.clone(), b));
                    }
                });
                button.add_controller(right);
                let keys = gtk::EventControllerKey::new();
                let id = item.id.clone();
                let weak = Rc::downgrade(self);
                keys.connect_key_pressed(move |keys, key, _, modifiers| {
                    if (key == gdk::Key::Menu
                        || key == gdk::Key::F10
                            && modifiers.contains(gdk::ModifierType::SHIFT_MASK))
                        && let Some(i) = weak.upgrade()
                        && let Some(button) = keys.widget().and_downcast::<gtk::Button>()
                    {
                        i.emit(Action::Menu(id.clone(), button));
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                });
                button.add_controller(keys);
                let source = gtk::DragSource::new();
                source.set_actions(gdk::DragAction::MOVE);
                source.set_propagation_phase(gtk::PropagationPhase::Capture);
                let id = item.id.clone();
                let weak = Rc::downgrade(self);
                source.connect_prepare(move |_, _, _| {
                    let i = weak.upgrade()?;
                    if !i.editable.get()
                        || !i
                            .buttons
                            .borrow()
                            .iter()
                            .any(|b| b.id == id && b.pinned.get())
                    {
                        return None;
                    }
                    Some(gdk::ContentProvider::for_value(
                        &format!("mosaic-pin:{id}").to_value(),
                    ))
                });
                let weak = Rc::downgrade(self);
                let icon_widget = icon.downgrade();
                source.connect_drag_begin(move |source, _| {
                    if let Some(icon) = icon_widget.upgrade() {
                        let preview = gtk::WidgetPaintable::new(Some(&icon));
                        source.set_icon(Some(&preview), icon.width() / 2, icon.height() / 2);
                    }
                    if let Some(i) = weak.upgrade() {
                        i.emit(Action::CancelHover);
                        i.event(Event::Drag(true));
                    }
                });
                let weak = Rc::downgrade(self);
                source.connect_drag_end(move |_, _, _| {
                    if let Some(i) = weak.upgrade() {
                        i.event(Event::Drag(false));
                    }
                });
                button.add_controller(source);
                self.items.append(&button);
                let button = Button {
                    id: item.id.clone(),
                    custom_icon: item.custom_icon.clone(),
                    button,
                    icon,
                    indicator,
                    content: col,
                    pinned: Cell::new(item.pinned),
                };
                button.set_edge(self.config.borrow().edge);
                self.buttons.borrow_mut().push(button);
            }
            self.append_trash_button();
            let grid = gtk::Button::with_label("▦");
            grid.add_css_class("dock-grid");
            grid.set_tooltip_text(Some("Приложения"));
            grid.update_property(&[gtk::accessible::Property::Label("Приложения")]);
            let weak = Rc::downgrade(self);
            grid.connect_clicked(move |_| {
                if let Some(i) = weak.upgrade() {
                    i.emit(Action::Applications);
                }
            });
            self.items.append(&grid);
            self.resize();
        }
        for (button, item) in self.buttons.borrow().iter().zip(&items) {
            button.pinned.set(item.pinned);
            let description = if item.windows == 0 {
                item.name.clone()
            } else {
                format!("{}\nОкон: {}", item.name, item.windows)
            };
            button.button.set_tooltip_text(Some(&description));
            button
                .button
                .update_property(&[gtk::accessible::Property::Label(&description)]);
            if item.active {
                button.button.add_css_class("active");
            } else {
                button.button.remove_css_class("active");
            }
            button.indicator.update(item.windows, item.pending);
        }
    }
    fn install_pin_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        target.connect_accept(move |_, drop| {
            weak.upgrade().is_some_and(|i| i.editable.get())
                && drop.drag().is_some()
                && drop.actions().contains(gdk::DragAction::MOVE)
                && drop.formats().contains_type(String::static_type())
        });
        let weak = Rc::downgrade(self);
        target.connect_drop(move |_, value, x, y| {
            let Some(i) = weak.upgrade() else {
                return false;
            };
            if !i.editable.get() {
                return false;
            }
            let Ok(payload) = value.get::<String>() else {
                return false;
            };
            let Some(source) = payload.strip_prefix("mosaic-pin:") else {
                return false;
            };
            let before = {
                let buttons = i.buttons.borrow();
                let pins = || buttons.iter().filter(|b| b.pinned.get());
                if !pins().any(|b| b.id == source) {
                    return false;
                }
                let slots = pins().filter_map(|b| {
                    let bounds = b.button.compute_bounds(&i.root)?;
                    Some((
                        b.id.as_str(),
                        i.along_axis(
                            f64::from(bounds.x() + bounds.width() / 2.),
                            f64::from(bounds.y() + bounds.height() / 2.),
                        ),
                    ))
                });
                dock_model::pin_before_at(slots, i.along_axis(x, y)).to_owned()
            };
            let source = source.to_owned();
            let owner = Rc::downgrade(&i);
            // Keep the drag source alive until GTK finishes dispatching the drop.
            glib::idle_add_local_once(move || {
                if let Some(i) = owner.upgrade() {
                    i.event(Event::Drag(false));
                    i.emit(Action::Reorder(source, before));
                }
            });
            true
        });
        self.root.add_controller(target);
    }
    fn resize(self: &Rc<Self>) {
        let edge = self.config.borrow().edge;
        let side = edge.vertical();
        let (width, height) = self.monitor_size.get();
        let limit = (if side { height - 32 } else { width } - 24).max(1);
        let divider_space = if self.divider.parent().is_some() {
            12
        } else {
            0
        };
        let size = dock_model::fitting_icon_size(
            self.config.borrow().icon_size,
            self.buttons.borrow().len(),
            (limit - divider_space).max(1) as u32,
        );
        self.actual_size.set(size);
        let strength = f64::from(self.config.borrow().hover.zoom_percent) / 100.;
        let x = ((f64::from(size) * strength / 2.).ceil() as i32) + 4;
        let zoom = (f64::from(size) * strength).ceil() as i32 + 4;
        let padding = x + 8;
        let (left, top, right, bottom) = match edge {
            DockEdge::Bottom => (padding, zoom, padding, 8),
            DockEdge::Left => (8, padding, zoom, padding),
            DockEdge::Right => (zoom, padding, 8, padding),
        };
        self.items.set_margin_start(left);
        self.items.set_margin_top(top);
        self.items.set_margin_end(right);
        self.items.set_margin_bottom(bottom);
        self.items
            .set_spacing(4 + (size as i32 / 4 + 2 - 14).max(0));
        self.body.set_margin_start(if side { 0 } else { x });
        self.body.set_margin_end(if side { 0 } else { x });
        self.body.set_margin_top(if side { x } else { 0 });
        self.body.set_margin_bottom(if side { x } else { 0 });
        let thickness = size as i32 + 40;
        let backdrop_percent = self.config.borrow().hover.background_percent;
        let backdrop_room = (thickness * backdrop_percent as i32 + 99) / 100;
        self.spacer.set_size_request(
            if side { backdrop_room } else { -1 },
            if side { -1 } else { backdrop_room },
        );
        self.body.set_size_request(
            if side { thickness } else { -1 },
            if side { -1 } else { thickness },
        );
        for button in self.buttons.borrow().iter() {
            button.icon.set_size(size as i32);
        }
        self.scroll.set_min_content_width(-1);
        self.scroll.set_min_content_height(-1);
        self.scroll
            .set_max_content_width(if side { -1 } else { limit });
        self.scroll
            .set_max_content_height(if side { limit } else { -1 });
        let axis = self.items.orientation();
        let length = self.items.measure(axis, -1).1.min(limit).max(1);
        if side {
            self.scroll.set_min_content_height(length);
            let width = self.root.measure(gtk::Orientation::Horizontal, -1).1;
            self.window.set_default_size(width, length);
            self.trigger.set_default_size(2, length);
        } else {
            self.scroll.set_min_content_width(length);
            let height = self.root.measure(gtk::Orientation::Vertical, -1).1;
            self.window.set_default_size(length, height);
            self.trigger.set_default_size(length, 2);
        }
        self.kick();
    }
    fn reset_hover(&self) {
        self.hover.borrow_mut().reset();
        self.body.set_scale(1.);
        for b in self.buttons.borrow().iter() {
            b.icon.set_scale(1.);
        }
    }
    fn hover_enabled(&self) -> bool {
        let config = self.config.borrow();
        !self.reduced.get()
            && config.magnification
            && (config.hover.zoom_percent > 0 || config.hover.background_percent > 0)
    }
    fn schedule_hover(self: &Rc<Self>) {
        if self.hover_enabled() {
            self.ensure_tick();
        } else {
            self.reset_hover();
        }
    }
    fn paint_hover(&self, frame: HoverFrame, hover: &settings::DockHover) {
        self.body
            .set_scale(1. + frame.amount * f64::from(hover.background_percent) / 100.);
        for b in self.buttons.borrow().iter() {
            let target = frame
                .axis
                .and_then(|axis| {
                    let rect = b.icon.compute_bounds(&self.root)?;
                    let center = self.along_axis(
                        f64::from(rect.x() + rect.width() / 2.),
                        f64::from(rect.y() + rect.height() / 2.),
                    );
                    Some(dock_model::zoom_with(
                        axis - center,
                        f64::from(self.actual_size.get()),
                        true,
                        false,
                        hover,
                    ))
                })
                .unwrap_or(1.);
            b.icon.set_scale(1. + (target - 1.) * frame.amount);
        }
    }
    fn kick(self: &Rc<Self>) {
        self.settle.set(2);
        self.ensure_tick();
    }
    fn ensure_tick(self: &Rc<Self>) {
        if self.tick.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = self.root.add_tick_callback(move |_, clock| {
            let Some(i) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            i.frames.set(i.frames.get() + 1);
            let active = if i.hover_enabled() {
                let config = i.config.borrow();
                let frame =
                    i.hover
                        .borrow_mut()
                        .frame(clock.frame_time(), i.pointer.get(), &config.hover);
                let active = frame.active;
                // Only the painted transform changes. Widgets keep their allocation.
                i.paint_hover(frame, &config.hover);
                active
            } else {
                false
            };
            let settle = i.settle.get();
            if settle > 0 {
                i.input_region();
                i.settle.set(settle - 1);
            }
            if !active && settle == 0 {
                i.tick.borrow_mut().take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.tick.borrow_mut() = Some(id);
    }
    fn input_region(&self) {
        let Some(surface) = self.window.surface() else {
            return;
        };
        let Some(body) = self.body.compute_bounds(&self.window) else {
            return;
        };
        let region = gtk::cairo::Region::create();
        let x = body.x().floor() as i32;
        let y = body.y().floor() as i32;
        let w = body.width().ceil() as i32;
        let h = body.height().ceil() as i32;
        let radius = 18_i32.min(w / 2).min(h / 2).max(0);
        if w > 0 && h > 2 * radius {
            let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                x,
                y + radius,
                w,
                h - 2 * radius,
            ));
        }
        for distance in 1..=radius {
            let inset = (f64::from(radius)
                - (f64::from(radius * radius - distance * distance)).sqrt())
            .ceil() as i32;
            if w > 2 * inset {
                for row in [radius - distance, h - radius - 1 + distance] {
                    let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                        x + inset,
                        y + row,
                        w - 2 * inset,
                        1,
                    ));
                }
            }
        }
        if let Some(r) = self.scrollbar_bounds(&self.window) {
            let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                r.x().floor() as i32,
                r.y().floor() as i32,
                r.width().ceil() as i32,
                r.height().ceil() as i32,
            ));
        }
        for b in self.buttons.borrow().iter() {
            if let Some(r) = b.icon.compute_bounds(&self.window) {
                let left = r.x().floor().max(0.) as i32;
                let top = r.y().floor().max(0.) as i32;
                let right = (r.x() + r.width()).ceil().min(self.window.width() as f32) as i32;
                let bottom = (r.y() + r.height()).ceil().min(self.window.height() as f32) as i32;
                if right > left && bottom > top {
                    let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                        left,
                        top,
                        right - left,
                        bottom - top,
                    ));
                }
            }
        }
        surface.set_input_region(Some(&region));
        // A resize can settle without changing icon scale. Request a frame
        // so the input region is committed even when the icons are unchanged.
        self.root.queue_draw();
        let (width, height) = if self.config.borrow().edge.vertical() {
            (2, self.window.height())
        } else {
            (self.window.width(), 2)
        };
        if width > 0 && height > 0 {
            self.trigger.set_default_size(width, height);
        }
    }
    fn outside_click(&self, gesture: &gtk::GestureClick, x: f64, y: f64) {
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        if self
            .scrollbar_bounds(&self.root)
            .is_some_and(|r| r.contains_point(&point))
        {
            return;
        }
        let buttons = self.buttons.borrow();
        if buttons.iter().any(|b| {
            b.button
                .compute_bounds(&self.root)
                .is_some_and(|r| r.contains_point(&point))
        }) {
            return;
        }
        let candidate = buttons.iter().find(|b| {
            b.icon
                .painted_bounds(&self.root)
                .is_some_and(|r| r.contains_point(&point))
        });
        if let Some(b) = candidate {
            let action = match gesture.current_button() {
                1 => Action::Activate(b.id.clone()),
                3 => Action::Menu(b.id.clone(), b.button.clone()),
                _ => return,
            };
            drop(buttons);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            self.emit(action);
        }
    }
    fn scrollbar_bounds(&self, target: &impl IsA<gtk::Widget>) -> Option<gtk::graphene::Rect> {
        let side = self.config.borrow().edge.vertical();
        let (bar, adjustment) = if side {
            (self.scroll.vscrollbar(), self.scroll.vadjustment())
        } else {
            (self.scroll.hscrollbar(), self.scroll.hadjustment())
        };
        if !bar.is_mapped() || adjustment.upper() <= adjustment.page_size() {
            return None;
        }
        bar.compute_bounds(target)
    }
}
