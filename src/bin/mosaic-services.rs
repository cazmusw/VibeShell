use mosaic_shell_tools::{diagnostics, ipc::Services};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    diagnostics::install_panic_hook("services");
    diagnostics::event("services", "start", 0);
    let result = run();
    diagnostics::event("services", "exit", i32::from(result.is_err()));
    result
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos() as u64;
    let (audio, start_audio) = mosaic_shell_tools::audio::AudioService::new(epoch);
    let bus = zbus::blocking::connection::Builder::session()
        .inspect_err(|_| diagnostics::event("services", "dbus_start_failed", 1))?
        .method_timeout(std::time::Duration::from_secs(3))
        .serve_at(
            "/org/mosaic/Services1",
            Services {
                epoch,
                stop: tx,
                audio,
                network: Default::default(),
                brightness: Default::default(),
                power: Default::default(),
                bluetooth: mosaic_shell_tools::bluetooth::Service::with_pairing(
                    std::env::var("MOSAIC_BLUETOOTH_PAIRING").as_deref() == Ok("1"),
                ),
            },
        )?
        .build()
        .inspect_err(|_| diagnostics::event("services", "dbus_start_failed", 2))?;
    bus.request_name_with_flags(
        "org.mosaic.Services1",
        zbus::fdo::RequestNameFlags::DoNotQueue.into(),
    )
    .inspect_err(|_| diagnostics::event("services", "dbus_name_failed", 1))?;
    let _tray = mosaic_shell_tools::tray_backend::Runtime::start()
        .inspect_err(|_| diagnostics::event("services", "tray_start_failed", 1))?;
    let _audio = start_audio(bus);
    rx.recv()?;
    Ok(())
}
