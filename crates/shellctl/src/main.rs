use anyhow::{bail, Context, Result};
fn main() {
    if let Err(e) = run() {
        eprintln!("shellctl: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
  Some("version")=>println!("Mosaic {}",env!("CARGO_PKG_VERSION")),
  Some("doctor")=>return doctor(),
  Some("dock")=>{
    if args.get(1).map(String::as_str)!=Some("show") || args.len()!=2 { bail!("Используйте shellctl dock show"); }
    let conn=zbus::blocking::Connection::session()?;
    let proxy=zbus::blocking::Proxy::new(&conn,"org.mosaic.Shell1","/org/mosaic/Shell1","org.mosaic.Shell1")?;
    proxy.call::<_,_,()>("ShowDock",&()).context("Оболочка не запущена или недоступна")?;
  },
  Some(command)=>{
   let method=match command {"launcher"=>"ShowLauncher","applications"=>"ShowApplications","settings"=>"OpenSettings","notifications"=>"ToggleNotifications","reload"=>"Reload",_=>bail!("Неизвестная команда {command}. Используйте doctor, version, launcher, applications, settings, notifications, reload")};
   let conn=zbus::blocking::Connection::session().context("Session D-Bus недоступен")?;
   let proxy=zbus::blocking::Proxy::new(&conn,"org.mosaic.Shell1","/org/mosaic/Shell1","org.mosaic.Shell1")?;
   proxy.call::<_,_,()>(method,&()).context("Оболочка не запущена или отклонила команду")?;
  },None=>println!("shellctl doctor | version | launcher | applications | settings | notifications | dock show | reload")
 }
    Ok(())
}
fn doctor() -> Result<()> {
    let config = shell_core::Config::load();
    let wayland = shell_wayland::probe();
    let conn = zbus::blocking::Connection::session();
    let mut owners = std::collections::BTreeMap::new();
    if let Ok(c) = &conn {
        let bus = zbus::blocking::Proxy::new(
            c,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )?;
        for name in [
            "org.mosaic.Shell1",
            "org.mosaic.Services1",
            "org.freedesktop.Notifications",
            "org.kde.StatusNotifierWatcher",
        ] {
            let owned: bool = bus.call("NameHasOwner", &(name,))?;
            owners.insert(name, owned);
        }
    }
    let ready = config.is_ok() && conn.is_ok() && wayland.as_ref().is_ok_and(|c| c.ready());
    let report = serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"ready":ready,"environment":{"wayland_display":std::env::var("WAYLAND_DISPLAY").ok(),"desktop":std::env::var("XDG_CURRENT_DESKTOP").ok()},"config_error":config.err().map(|e|e.to_string()),"session_bus_error":conn.err().map(|e|e.to_string()),"owners":owners,"capabilities":wayland.as_ref().ok(),"missing_protocols":wayland.as_ref().ok().map(|c|c.missing()),"wayland_error":wayland.err().map(|e|e.to_string())});
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !ready {
        bail!("Обязательные компоненты среды недоступны; см. отчёт выше")
    };
    Ok(())
}
