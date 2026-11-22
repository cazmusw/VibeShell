use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatteryState {
    Unknown,
    Charging,
    Discharging,
    Empty,
    Full,
    PendingCharge,
    PendingDischarge,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub percent: u8,
    pub state: BatteryState,
    pub seconds: Option<u64>,
}

impl Battery {
    pub fn from_values(
        percent: f64,
        state: u32,
        until_empty: i64,
        until_full: i64,
    ) -> Option<Self> {
        if !(0. ..=100.).contains(&percent) {
            return None;
        }
        let state = match state {
            1 => BatteryState::Charging,
            2 => BatteryState::Discharging,
            3 => BatteryState::Empty,
            4 => BatteryState::Full,
            5 => BatteryState::PendingCharge,
            6 => BatteryState::PendingDischarge,
            _ => BatteryState::Unknown,
        };
        let estimate = match state {
            BatteryState::Charging => until_full,
            BatteryState::Discharging => until_empty,
            _ => 0,
        };
        Some(Self {
            percent: percent.round() as u8,
            state,
            seconds: u64::try_from(estimate).ok().filter(|secs| *secs > 0),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    PowerSaver,
    Balanced,
    Performance,
}

impl Profile {
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "power-saver" => Some(Self::PowerSaver),
            "balanced" => Some(Self::Balanced),
            "performance" => Some(Self::Performance),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::PowerSaver => "Экономия энергии",
            Self::Balanced => "Сбалансированный",
            Self::Performance => "Производительность",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profiles {
    pub owner: String,
    pub offered: Vec<Profile>,
    pub active: Profile,
    pub degraded: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub upower_available: bool,
    pub on_battery: Option<bool>,
    pub battery_present: Option<bool>,
    pub battery: Option<Battery>,
    pub profiles: Option<Profiles>,
}

#[cfg(feature = "desktop")]
#[path = "power_bus.rs"]
mod bus;
#[cfg(feature = "desktop")]
pub use bus::Service;
