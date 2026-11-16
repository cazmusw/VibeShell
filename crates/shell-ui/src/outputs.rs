//! Recreate output-bound surfaces after GDK monitor changes, coalesced on the main loop.
use crate::{app::Shell, dock::Dock, panel};
use gtk::{glib, prelude::*};
use std::rc::Rc;
impl Shell {
    pub fn schedule_outputs(self: &Rc<Self>) {
        if self.outputs_pending.replace(true) { return; }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(shell) = weak.upgrade() {
                shell.outputs_pending.set(false);
                shell.rebuild_outputs();
            }
        });
    }
    pub fn rebuild_outputs(self: &Rc<Self>) {
        // Take lists out of RefCells before GTK can synchronously emit callbacks.
        let handlers = std::mem::take(&mut *self.monitor_handlers.borrow_mut());
        for (monitor, handler) in handlers { monitor.disconnect(handler); }
        let overlay = self.overlay.borrow_mut().take();
        if let Some(window) = overlay { window.close(); }
        let docks = std::mem::take(&mut *self.docks.borrow_mut());
        for dock in docks { dock.destroy(); }
        let panels = std::mem::take(&mut *self.panels.borrow_mut());
        for window in panels { window.close(); }
        self.active_labels.borrow_mut().clear();
        let Some(display) = gtk::gdk::Display::default() else { return };
        let monitors = display.monitors();
        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gtk::gdk::Monitor>() else { continue };
            self.panels.borrow_mut().push(panel::create(self, &monitor));
            for property in ["geometry", "scale-factor"] {
                let weak = Rc::downgrade(self);
                let handler = monitor.connect_notify_local(Some(property), move |_, _| {
                    if let Some(shell) = weak.upgrade() { shell.schedule_outputs(); }
                });
                self.monitor_handlers.borrow_mut().push((monitor.clone(), handler));
            }
            if index == 0 {
                let dock = Dock::new(self, &monitor);
                dock.rebuild(self, &self.wayland.snapshot().windows);
                self.docks.borrow_mut().push(dock);
            }
        }
        let snapshot = self.wayland.snapshot();
        let active = snapshot.windows.iter().find(|w| w.active);
        let name = active.map(|w| self.catalog.match_window(&w.app_id).map(|a| a.name.as_str()).unwrap_or(&w.app_id)).unwrap_or("Рабочий стол");
        for label in self.active_labels.borrow().iter() { label.set_text(name); }
    }
}
