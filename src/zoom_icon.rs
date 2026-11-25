use crate::settings::DockEdge;
use gtk::{glib, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

fn anchor(edge: DockEdge, width: f32, height: f32) -> (f32, f32) {
    match edge {
        DockEdge::Bottom => (width / 2., height),
        DockEdge::Left => (0., height / 2.),
        DockEdge::Right => (width, height / 2.),
    }
}
mod imp {
    use super::*;
    #[derive(Default)]
    pub struct ZoomIcon {
        pub child: RefCell<Option<gtk::Widget>>,
        pub size: Cell<i32>,
        pub scale: Cell<f64>,
        pub edge: Cell<DockEdge>,
        pub backdrop: Cell<bool>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for ZoomIcon {
        const NAME: &'static str = "MosaicZoomIcon";
        type Type = super::ZoomIcon;
        type ParentType = gtk::Widget;
        fn class_init(class: &mut Self::Class) {
            class.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }
    impl ObjectImpl for ZoomIcon {
        fn constructed(&self) {
            self.parent_constructed();
            self.scale.set(1.);
            self.obj().set_overflow(gtk::Overflow::Visible);
        }
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }
    impl WidgetImpl for ZoomIcon {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if self.backdrop.get() {
                return self
                    .child
                    .borrow()
                    .as_ref()
                    .map_or((0, 0, -1, -1), |child| child.measure(orientation, for_size));
            }
            if let Some(child) = self.child.borrow().as_ref() {
                child.measure(orientation, for_size);
            }
            let size = self.size.get();
            (size, size, -1, -1)
        }
        fn size_allocate(&self, width: i32, height: i32, _: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, -1, None);
            }
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if let Some(child) = self.child.borrow().as_ref() {
                let object = self.obj();
                let (x, y) = anchor(
                    self.edge.get(),
                    object.width() as f32,
                    object.height() as f32,
                );
                snapshot.save();
                snapshot.translate(&gtk::graphene::Point::new(x, y));
                let (sx, sy) = object.axes();
                snapshot.scale(sx, sy);
                snapshot.translate(&gtk::graphene::Point::new(-x, -y));
                object.snapshot_child(child, snapshot);
                snapshot.restore();
            }
        }
    }
}
glib::wrapper! {pub struct ZoomIcon(ObjectSubclass<imp::ZoomIcon>) @extends gtk::Widget,@implements gtk::Accessible,gtk::Buildable,gtk::ConstraintTarget;}
impl ZoomIcon {
    pub fn new(child: &gtk::Widget, size: i32) -> Self {
        let icon = Self::wrap(child);
        icon.set_size(size);
        icon
    }
    pub fn backdrop(child: &impl IsA<gtk::Widget>) -> Self {
        let surface = Self::wrap(child.as_ref());
        surface.imp().backdrop.set(true);
        surface.set_can_target(false);
        surface
    }
    fn wrap(child: &gtk::Widget) -> Self {
        let widget: Self = glib::Object::builder().build();
        child.set_parent(&widget);
        *widget.imp().child.borrow_mut() = Some(child.clone());
        widget
    }
    fn axes(&self) -> (f32, f32) {
        let scale = self.scale() as f32;
        if !self.imp().backdrop.get() {
            (scale, scale)
        } else if self.imp().edge.get().vertical() {
            (scale, 1.)
        } else {
            (1., scale)
        }
    }
    pub fn set_size(&self, size: i32) {
        if self.imp().size.replace(size) != size {
            if let Some(child) = self.imp().child.borrow().as_ref() {
                if let Some(image) = child.downcast_ref::<gtk::Image>() {
                    image.set_pixel_size(size);
                } else {
                    child.set_size_request(size, size);
                }
            }
            self.queue_resize();
        }
    }
    pub fn scale(&self) -> f64 {
        self.imp().scale.get()
    }
    pub fn set_edge(&self, edge: DockEdge) {
        if self.imp().edge.replace(edge) != edge {
            self.queue_draw();
        }
    }
    pub fn painted_bounds(&self, target: &impl IsA<gtk::Widget>) -> Option<gtk::graphene::Rect> {
        let rect = self.compute_bounds(target)?;
        let (x, y) = anchor(self.imp().edge.get(), rect.width(), rect.height());
        let (sx, sy) = self.axes();
        Some(gtk::graphene::Rect::new(
            rect.x() + x * (1. - sx),
            rect.y() + y * (1. - sy),
            rect.width() * sx,
            rect.height() * sy,
        ))
    }
    pub fn set_scale(&self, scale: f64) {
        let scale = scale.clamp(1., 1.7);
        if (self.imp().scale.replace(scale) - scale).abs() > 0.0001 {
            self.queue_draw();
        }
    }
}
