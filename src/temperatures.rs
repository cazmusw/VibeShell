//! Read-only Linux hardware and thermal-zone monitoring.

use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_SENSORS: usize = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Cpu,
    Gpu,
    Storage,
    Other,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
            Self::Storage => "Накопители",
            Self::Other => "Другие датчики",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Reading {
    Value { milli_celsius: i32 },
    Unavailable,
    Suspended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Sensor {
    pub id: String,
    pub source: String,
    pub label: String,
    pub kind: Kind,
    #[serde(flatten)]
    pub reading: Reading,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub sensors: Vec<Sensor>,
}

impl Snapshot {
    pub fn hottest(&self, kind: Kind) -> Option<i32> {
        self.sensors
            .iter()
            .filter(|sensor| sensor.kind == kind)
            .filter_map(|sensor| match sensor.reading {
                Reading::Value { milli_celsius } => Some(milli_celsius),
                Reading::Unavailable | Reading::Suspended => None,
            })
            .max()
    }
}

pub fn read_system() -> Snapshot {
    read_class(Path::new("/sys/class"))
}

fn field(path: &Path) -> Option<String> {
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(129)
        .read_to_string(&mut text)
        .ok()?;
    let clean = text.trim();
    (text.len() <= 128 && !clean.is_empty() && !clean.chars().any(char::is_control))
        .then(|| clean.to_owned())
}

fn reading(path: &Path) -> Reading {
    field(path)
        .and_then(|text| text.parse::<i32>().ok())
        .filter(|value| (-273_150..=300_000).contains(value))
        .map_or(Reading::Unavailable, |milli_celsius| Reading::Value {
            milli_celsius,
        })
}

fn kind(source: &str) -> Kind {
    match source.to_ascii_lowercase().as_str() {
        "coretemp" | "k8temp" | "k10temp" | "zenpower" | "x86_pkg_temp" | "cpu_thermal"
        | "cpu-thermal" => Kind::Cpu,
        "amdgpu" | "radeon" | "nouveau" | "nvidia" | "i915" | "xe" | "gpu_thermal"
        | "gpu-thermal" => Kind::Gpu,
        "nvme" | "drivetemp" => Kind::Storage,
        _ => Kind::Other,
    }
}

fn numbered(name: &str, prefix: &str) -> Option<u32> {
    let digits = name.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn numbered_entries(
    path: &Path,
    number: impl Fn(&str) -> Option<u32>,
) -> Vec<(u32, String, PathBuf)> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut matching: Vec<_> = entries
        .take(512)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((number(&name)?, name, entry.path()))
        })
        .collect();
    matching.sort_unstable_by_key(|entry| entry.0);
    matching
}

pub fn read_class(class: &Path) -> Snapshot {
    let mut sensors = Vec::new();
    for (_, device, path) in numbered_entries(&class.join("hwmon"), |n| numbered(n, "hwmon")) {
        let source = field(&path.join("name")).unwrap_or_else(|| device.clone());
        let category = kind(&source);
        let suspended = field(&path.join("device/power/runtime_status"))
            .is_some_and(|status| matches!(status.as_str(), "suspended" | "suspending"));
        let inputs = numbered_entries(&path, |n| numbered(n.strip_suffix("_input")?, "temp"));
        for (number, _, input) in inputs.into_iter().take(MAX_SENSORS - sensors.len()) {
            let key = format!("temp{number}");
            let label = field(&path.join(format!("{key}_label"))).unwrap_or_else(|| key.clone());
            sensors.push(Sensor {
                id: format!("{device}/{key}"),
                source: source.clone(),
                label,
                kind: category,
                reading: if suspended {
                    Reading::Suspended
                } else {
                    reading(&input)
                },
            });
        }
        if sensors.len() == MAX_SENSORS {
            break;
        }
    }
    for (_, name, path) in numbered_entries(&class.join("thermal"), |n| numbered(n, "thermal_zone"))
        .into_iter()
        .take(MAX_SENSORS - sensors.len())
    {
        let source = field(&path.join("type")).unwrap_or_else(|| name.clone());
        sensors.push(Sensor {
            id: name.clone(),
            kind: kind(&source),
            source,
            label: name,
            reading: reading(&path.join("temp")),
        });
    }
    Snapshot { sensors }
}
