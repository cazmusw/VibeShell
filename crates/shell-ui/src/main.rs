mod app;
mod catalog;
mod dock;
mod dock_icon;
mod ipc;
mod launcher;
mod panel;
mod outputs;
use gtk::prelude::*;
fn main() -> gtk::glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("org.mosaic.Shell")
        .build();
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let flag = failed.clone();
    let started = std::cell::Cell::new(false);
    app.connect_activate(move |a| {
        if started.replace(true) { return; }
        if let Err(e) = app::start(a) {
            eprintln!("Mosaic: {e:#}");
            flag.set(true);
            a.quit();
        }
    });
    let status = app.run();
    final_status(failed.get(), status)
}

fn final_status(failed: bool, status: gtk::glib::ExitCode) -> gtk::glib::ExitCode {
    if failed {
        gtk::glib::ExitCode::FAILURE
    } else {
        status
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_failure_produces_nonzero_process_status() {
        assert_eq!(
            super::final_status(true, gtk::glib::ExitCode::SUCCESS),
            gtk::glib::ExitCode::FAILURE
        );
        assert_eq!(
            super::final_status(false, gtk::glib::ExitCode::SUCCESS),
            gtk::glib::ExitCode::SUCCESS
        );
    }
}
