use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};
use zbus::{fdo, zvariant::OwnedObjectPath};

const LOGIN: &str = "/org/freedesktop/login1";
const SESSION: &str = "/org/freedesktop/login1/session/_31";
const AUTO: &str = "/org/freedesktop/login1/session/auto";
const SEAT: &str = "/org/freedesktop/login1/seat/seat0";

struct State {
    root: PathBuf,
    active: bool,
    remote: bool,
    denied: bool,
    attempts: u32,
    accepted: u32,
    last_name: String,
    last_level: u32,
}

type Shared = Arc<Mutex<State>>;

fn path(value: &'static str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value).expect("fixture object path")
}

struct Manager;

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn get_session(&self, id: &str) -> fdo::Result<OwnedObjectPath> {
        if id != "1" {
            return Err(fdo::Error::InvalidArgs("Unknown fixture session".into()));
        }
        Ok(path(SESSION))
    }
}

struct Session(Shared);

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(property)]
    fn id(&self) -> &str {
        "1"
    }

    #[zbus(property)]
    fn active(&self) -> bool {
        self.0.lock().unwrap().active
    }

    #[zbus(property)]
    fn remote(&self) -> bool {
        self.0.lock().unwrap().remote
    }

    #[zbus(property)]
    fn seat(&self) -> (&str, OwnedObjectPath) {
        ("seat0", path(SEAT))
    }

    fn set_brightness(&self, subsystem: &str, name: &str, level: u32) -> fdo::Result<()> {
        let mut state = self.0.lock().unwrap();
        state.attempts += 1;
        if !state.active || state.remote || state.denied {
            return Err(fdo::Error::AccessDenied("Fixture brightness denied".into()));
        }
        if subsystem != "backlight"
            || name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
            })
            || name == "."
            || name == ".."
        {
            return Err(fdo::Error::InvalidArgs("Invalid fixture backlight".into()));
        }

        let device = state.root.join(name);
        let maximum = fixture_file(&device, "max_brightness")?;
        let requested = fixture_file(&device, "brightness")?;
        let actual = fixture_file(&device, "actual_brightness")?;
        let max = fs::read_to_string(maximum)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .ok_or_else(|| fdo::Error::Failed("Invalid fixture maximum".into()))?;
        if level == 0 || level > max {
            return Err(fdo::Error::InvalidArgs(
                "Brightness outside fixture range".into(),
            ));
        }

        let value = format!("{level}\n");
        fs::write(requested, &value)
            .and_then(|()| fs::write(actual, &value))
            .map_err(|error| fdo::Error::Failed(format!("Write fixture brightness: {error}")))?;
        state.accepted += 1;
        state.last_name = name.to_owned();
        state.last_level = level;
        Ok(())
    }
}

fn fixture_file(device: &Path, name: &str) -> fdo::Result<PathBuf> {
    let file = device.join(name);
    if file.canonicalize().as_deref().ok() == Some(file.as_path()) && file.is_file() {
        return Ok(file);
    }
    Err(fdo::Error::InvalidArgs(
        "Missing fixture backlight file".into(),
    ))
}

struct Control(Shared);

#[zbus::interface(name = "org.mosaic.BrightnessFixture")]
impl Control {
    fn set_session(&self, active: bool, remote: bool) {
        let mut state = self.0.lock().unwrap();
        state.active = active;
        state.remote = remote;
    }

    fn set_denied(&self, denied: bool) {
        self.0.lock().unwrap().denied = denied;
    }

    fn writes(&self) -> (u32, u32, String, u32) {
        let state = self.0.lock().unwrap();
        (
            state.attempts,
            state.accepted,
            state.last_name.clone(),
            state.last_level,
        )
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bus_address = env::var("DBUS_SESSION_BUS_ADDRESS")?;
    if env::var("DBUS_SYSTEM_BUS_ADDRESS")? != bus_address {
        return Err("brightness fixture requires an isolated test bus".into());
    }
    let root = PathBuf::from(env::var_os("MOSAIC_BRIGHTNESS_FIXTURE_ROOT").ok_or("fixture root")?)
        .canonicalize()?;
    if !root.is_dir()
        || root.file_name().and_then(|name| name.to_str()) != Some("backlight")
        || !root
            .parent()
            .and_then(Path::file_name)
            .and_then(|parent| parent.to_str())
            .is_some_and(|parent| parent.starts_with("mosaic-vertical-"))
    {
        return Err("brightness fixture requires the vertical test backlight directory".into());
    }

    let state = Arc::new(Mutex::new(State {
        root,
        active: true,
        remote: false,
        denied: false,
        attempts: 0,
        accepted: 0,
        last_name: String::new(),
        last_level: 0,
    }));
    let _connection = zbus::blocking::connection::Builder::session()?
        .name("org.freedesktop.login1")?
        .serve_at(LOGIN, Manager)?
        .serve_at(AUTO, Session(state.clone()))?
        .serve_at(SESSION, Session(state.clone()))?
        .serve_at("/org/mosaic/BrightnessFixture", Control(state))?
        .build()?;
    loop {
        thread::park();
    }
}
