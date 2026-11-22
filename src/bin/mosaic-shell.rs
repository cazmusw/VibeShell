use gtk::{gio, glib, prelude::*};
use mosaic_shell_tools::diagnostics;
use std::{os::unix::process::CommandExt, process::Command};

fn main() -> glib::ExitCode {
    if let Some(schema_dir) = option_env!("MOSAIC_GTK_SCHEMA_DIR")
        && std::env::var_os("GSETTINGS_SCHEMA_DIR").is_none()
    {
        let error = match std::env::current_exe() {
            Ok(executable) => Command::new(executable)
                .args(std::env::args_os().skip(1))
                .env("GSETTINGS_SCHEMA_DIR", schema_dir)
                .exec(),
            Err(error) => error,
        };
        eprintln!("Mosaic could not set up GTK settings: {error}");
        return glib::ExitCode::FAILURE;
    }
    diagnostics::install_panic_hook("shell");
    diagnostics::event("shell", "start", 0);
    let app = gtk::Application::builder()
        .application_id("org.mosaic.Shell.Ui")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let (tx, rx) = async_channel::bounded(32);
    let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let _bus = match mosaic_shell_tools::ipc::ui_bus(tx, shared.clone()) {
        Ok(bus) => bus,
        Err(e) => {
            diagnostics::event("shell", "dbus_start_failed", 1);
            eprintln!("Mosaic D-Bus unavailable or another instance is running: {e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let failure = failed.clone();
    app.connect_activate(move |app| {
        if let Err(e) = mosaic_shell_tools::ui::run(app, rx.clone(), shared.clone()) {
            failure.set(true);
            diagnostics::event("shell", "ui_start_failed", 1);
            eprintln!("Mosaic UI startup failed: {e}");
            app.quit();
        }
    });
    let code = app.run();
    let code = if failed.get() {
        glib::ExitCode::FAILURE
    } else {
        code
    };
    diagnostics::event("shell", "exit", i32::from(code));
    code
}
