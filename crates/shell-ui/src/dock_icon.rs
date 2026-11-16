//! A fixed allocation with animated drawing, so hover never moves click targets.
use gtk::{glib, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};

mod imp {
    use super::*;
    #[derive(Default)]
    pub struct DockIcon {
        pub image: RefCell<Option<gtk::Image>>,
        pub base: Cell<i32>,
        pub maximum: Cell<f64>,
        pub scale: Cell<f64>,
        pub target: Cell<f64>,
        pub ticking: Cell<bool>,
    }
    #[glib::object_subclass]
    impl ObjectSubclass for DockIcon {
        const NAME: &'static str = "MosaicDockIcon";
        type Type = super::DockIcon;
        type ParentType = gtk::Widget;
    }
    impl ObjectImpl for DockIcon {
        fn dispose(&self) {
            if let Some(image) = self.image.borrow_mut().take() {
                image.unparent();
            }
        }
    }
    impl WidgetImpl for DockIcon {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let extent = (self.base.get() as f64 * self.maximum.get()).ceil() as i32;
            (extent, extent, -1, -1)
        }
        fn size_allocate(&self, _: i32, _: i32, baseline: i32) {
            if let Some(image) = self.image.borrow().as_ref() {
                image.allocate(self.base.get(), self.base.get(), baseline, None);
            }
        }
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(image) = self.image.borrow().as_ref().cloned() else {
                return;
            };
            let obj = self.obj();
            let scale = self.scale.get() as f32;
            let size = self.base.get() as f32 * scale;
            snapshot.save();
            snapshot.translate(&gtk::graphene::Point::new(
                (obj.width() as f32 - size) / 2.0,
                obj.height() as f32 - size,
            ));
            snapshot.scale(scale, scale);
            obj.snapshot_child(&image, snapshot);
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct DockIcon(ObjectSubclass<imp::DockIcon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
impl DockIcon {
    pub fn new(image: gtk::Image, base: i32, maximum: f64) -> Self {
        let icon: Self = glib::Object::new();
        let imp = icon.imp();
        imp.base.set(base);
        imp.maximum.set(maximum);
        imp.scale.set(1.0);
        imp.target.set(1.0);
        image.set_parent(&icon);
        *imp.image.borrow_mut() = Some(image);
        icon
    }
    pub fn animate_to(&self, target: f64, immediate: bool) {
        let imp = self.imp();
        imp.target.set(target.clamp(1.0, imp.maximum.get()));
        if immediate {
            imp.scale.set(imp.target.get());
            self.queue_draw();
            return;
        }
        if (imp.scale.get() - imp.target.get()).abs() < 0.001 || imp.ticking.replace(true) {
            return;
        }
        let weak = self.downgrade();
        let previous = Cell::new(0_i64);
        self.add_tick_callback(move |_, clock| {
            let Some(icon) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let imp = icon.imp();
            let now = clock.frame_time();
            let dt = if previous.get() == 0 {
                1.0 / 60.0
            } else {
                ((now - previous.get()) as f64 / 1_000_000.0).clamp(0.0, 0.1)
            };
            previous.set(now);
            let remaining = imp.target.get() - imp.scale.get();
            if remaining.abs() < 0.002 {
                imp.scale.set(imp.target.get());
                imp.ticking.set(false);
                icon.queue_draw();
                return glib::ControlFlow::Break;
            }
            imp.scale
                .set(imp.scale.get() + remaining * (1.0 - (-dt / 0.055).exp()));
            icon.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}
