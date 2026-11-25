use crate::settings::{DockEdge, IconFilter};
use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};
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
        pub visual: Cell<i32>,
        pub filter: Cell<IconFilter>,
        pub texture: std::cell::OnceCell<gdk::Texture>,
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
                if self.backdrop.get() {
                    child.allocate(width, height, -1, None);
                } else {
                    let size = self.visual.get();
                    let placement = gsk::Transform::new().translate(&graphene::Point::new(
                        (width - size) as f32 / 2.,
                        (height - size) as f32 / 2.,
                    ));
                    child.allocate(size, size, -1, Some(placement));
                }
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
                if let Some(texture) = self.texture.get() {
                    let size = self.visual.get() as f32;
                    let ratio = size / texture.width().max(texture.height()) as f32;
                    let width = texture.width() as f32 * ratio;
                    let height = texture.height() as f32 * ratio;
                    let bounds = graphene::Rect::new(
                        (object.width() as f32 - width) / 2.,
                        (object.height() as f32 - height) / 2.,
                        width,
                        height,
                    );
                    let filter = match self.filter.get() {
                        IconFilter::Linear => gsk::ScalingFilter::Linear,
                        IconFilter::Smooth => gsk::ScalingFilter::Trilinear,
                        IconFilter::Nearest => gsk::ScalingFilter::Nearest,
                    };
                    snapshot.append_scaled_texture(texture, filter, &bounds);
                } else {
                    object.snapshot_child(child, snapshot);
                }
                snapshot.restore();
            }
        }
    }
}
glib::wrapper! {pub struct ZoomIcon(ObjectSubclass<imp::ZoomIcon>) @extends gtk::Widget,@implements gtk::Accessible,gtk::Buildable,gtk::ConstraintTarget;}
impl ZoomIcon {
    pub fn new(child: &gtk::Widget, size: i32) -> Self {
        let icon = Self::wrap(child);
        if let Some(texture) = child
            .downcast_ref::<gtk::Picture>()
            .and_then(gtk::Picture::paintable)
            .and_downcast::<gdk::Texture>()
        {
            let _ = icon.imp().texture.set(texture);
        }
        icon.set_sizes(size, size);
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
    pub fn set_sizes(&self, size: i32, visual: i32) {
        let allocation_changed = self.imp().size.replace(size) != size;
        if self.imp().visual.replace(visual) != visual {
            if let Some(child) = self.imp().child.borrow().as_ref() {
                if let Some(image) = child.downcast_ref::<gtk::Image>() {
                    image.set_pixel_size(visual);
                } else {
                    child.set_size_request(visual, visual);
                }
            }
            self.queue_allocate();
            self.queue_draw();
        }
        if allocation_changed {
            self.queue_resize();
        }
    }
    pub fn set_filter(&self, filter: IconFilter) {
        if self.imp().filter.replace(filter) != filter {
            self.queue_draw();
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
        let visual = self.imp().visual.get() as f32;
        let inset_x = (rect.width() - visual) / 2.;
        let inset_y = (rect.height() - visual) / 2.;
        Some(gtk::graphene::Rect::new(
            rect.x() + x * (1. - sx) + inset_x * sx,
            rect.y() + y * (1. - sy) + inset_y * sy,
            visual * sx,
            visual * sy,
        ))
    }
    pub fn set_scale(&self, scale: f64) {
        let scale = scale.clamp(1., 1.7);
        if (self.imp().scale.replace(scale) - scale).abs() > 0.0001 {
            self.queue_draw();
        }
    }
}
