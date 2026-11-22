use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use zbus::{
    fdo,
    zvariant::{OwnedObjectPath, OwnedValue, Str},
};

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const DISPLAY: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const PPD: &str = "org.freedesktop.UPower.PowerProfiles";
const PPD_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
type State = Arc<Mutex<(String, bool, bool)>>;

struct UPower;
#[zbus::interface(name = "org.freedesktop.UPower")]
impl UPower {
    fn get_display_device(&self) -> OwnedObjectPath {
        DISPLAY.try_into().unwrap()
    }
    #[zbus(property)]
    fn on_battery(&self) -> bool {
        true
    }
}

struct Display(State);
#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl Display {
    #[zbus(property)]
    fn is_present(&self) -> bool {
        self.0.lock().unwrap().2
    }
    #[zbus(property, name = "Type")]
    fn device_type(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn state(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn percentage(&self) -> f64 {
        63.4
    }
    #[zbus(property)]
    fn time_to_empty(&self) -> i64 {
        5400
    }
    #[zbus(property)]
    fn time_to_full(&self) -> i64 {
        0
    }
}

struct PowerProfiles(State);
#[zbus::interface(name = "org.freedesktop.UPower.PowerProfiles")]
impl PowerProfiles {
    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.0.lock().unwrap().0.clone()
    }
    #[zbus(property)]
    fn set_active_profile(&self, value: &str) -> fdo::Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.1 {
            return Err(fdo::Error::AccessDenied("fixture denied change".into()));
        }
        if !matches!(value, "balanced" | "power-saver") {
            return Err(fdo::Error::InvalidArgs("unknown profile".into()));
        }
        state.0 = value.into();
        Ok(())
    }
    #[zbus(property)]
    fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
        ["power-saver", "balanced"]
            .into_iter()
            .map(|profile| {
                HashMap::from([("Profile".into(), OwnedValue::from(Str::from(profile)))])
            })
            .collect()
    }
    #[zbus(property)]
    fn performance_degraded(&self) -> &str {
        ""
    }
}

struct Control(State);
#[zbus::interface(name = "org.mosaic.PowerFixture")]
impl Control {
    fn configure(&self, profile: &str, denied: bool, present: bool) {
        *self.0.lock().unwrap() = (profile.into(), denied, present);
    }
}

fn main() -> zbus::Result<()> {
    let state = Arc::new(Mutex::new(("balanced".into(), false, true)));
    let _bus = zbus::blocking::connection::Builder::session()?
        .serve_at(UPOWER_PATH, UPower)?
        .serve_at(DISPLAY, Display(state.clone()))?
        .serve_at(PPD_PATH, PowerProfiles(state.clone()))?
        .serve_at("/Fixture", Control(state))?
        .name(UPOWER)?
        .name(PPD)?
        .name("org.mosaic.PowerFixture")?
        .build()?;
    loop {
        std::thread::park();
    }
}
