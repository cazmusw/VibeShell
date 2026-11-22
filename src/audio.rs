use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, BufRead, BufReader, Read},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const BUS: &str = "org.mosaic.Services1";
pub const PATH: &str = "/org/mosaic/Services1";
const COMMAND_LIMIT: Duration = Duration::from_millis(700);
const NORMAL_VOLUME: u32 = 65536;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioState {
    pub available: bool,
    pub revision: u64,
    pub default_sink: String,
    pub default_source: String,
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    #[serde(default)]
    pub playback: Vec<Playback>,
    #[serde(default)]
    pub cards: Vec<AudioCard>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    #[serde(default)]
    pub index: Option<u32>,
    pub name: String,
    pub description: String,
    pub muted: bool,
    pub volume: Option<u32>,
    #[serde(default)]
    pub balance: Option<u32>,
    #[serde(default)]
    pub active_port: String,
    #[serde(default)]
    pub ports: Vec<AudioChoice>,
    #[serde(default)]
    stereo: Option<StereoOrder>,
    channels: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioChoice {
    pub name: String,
    pub description: String,
    pub available: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AudioCard {
    pub name: String,
    pub description: String,
    pub active_profile: String,
    pub profiles: Vec<AudioChoice>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum StereoOrder {
    LeftRight,
    RightLeft,
}

impl StereoOrder {
    fn parse(channel_map: &str) -> Option<Self> {
        let mut channels = channel_map.split(',').map(str::trim);
        match (channels.next(), channels.next(), channels.next()) {
            (Some("front-left"), Some("front-right"), None) => Some(Self::LeftRight),
            (Some("front-right"), Some("front-left"), None) => Some(Self::RightLeft),
            _ => None,
        }
    }

    fn reorder(self, [first, second]: [u32; 2]) -> [u32; 2] {
        match self {
            Self::LeftRight => [first, second],
            Self::RightLeft => [second, first],
        }
    }

    fn balance(self, channels: &[u32]) -> Option<u32> {
        let [left, right] = self.reorder(channels.try_into().ok()?);
        let peak = left.max(right);
        if peak == 0 {
            return None;
        }
        // Keep 100 as the wire representation of equal channel levels.
        Some((100. + 100. * (f64::from(right) - f64::from(left)) / f64::from(peak)).round() as u32)
    }

    fn set_balance(self, channels: &[u32], balance: u32) -> Option<[u32; 2]> {
        let [left, right]: [u32; 2] = channels.try_into().ok()?;
        let peak = left.max(right);
        if peak == 0 {
            return None;
        }
        let scale = |percent: u32| ((u64::from(peak) * u64::from(percent) + 50) / 100) as u32;
        Some(self.reorder([scale(200 - balance.max(100)), scale(balance.min(100))]))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Playback {
    pub index: u32,
    pub application: String,
    pub media: String,
    pub output: String,
    pub muted: bool,
    pub volume: Option<u32>,
    client: String,
    channels: Vec<u32>,
}

#[derive(Deserialize)]
struct ServerInfo {
    #[serde(default, deserialize_with = "null_as_empty")]
    default_sink_name: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    default_source_name: String,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(de)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct PulseDevice {
    index: Option<u32>,
    name: String,
    description: Option<String>,
    active_port: Option<String>,
    #[serde(default)]
    ports: Vec<PulsePort>,
    #[serde(flatten)]
    level: PulseLevel,
}

#[derive(Deserialize)]
struct PulsePort {
    name: String,
    #[serde(flatten)]
    choice: PulseChoice,
}

#[derive(Deserialize)]
struct PulseChoice {
    description: Option<String>,
    #[serde(default)]
    priority: u32,
    available: Option<bool>,
    availability: Option<String>,
}

#[derive(Deserialize)]
struct PulseCard {
    name: String,
    active_profile: Option<String>,
    #[serde(default)]
    properties: HashMap<String, serde_json::Value>,
    #[serde(default)]
    profiles: BTreeMap<String, PulseChoice>,
}

#[derive(Deserialize)]
struct PulsePlayback {
    index: u32,
    sink: u32,
    client: Option<serde_json::Value>,
    #[serde(default)]
    properties: HashMap<String, serde_json::Value>,
    #[serde(flatten)]
    level: PulseLevel,
}

#[derive(Deserialize)]
struct PulseLevel {
    mute: bool,
    #[serde(default)]
    channel_map: String,
    volume: Option<HashMap<String, Channel>>,
}

#[derive(Deserialize)]
struct Channel {
    value: u32,
}

impl PulseLevel {
    fn volume(&self) -> Option<u32> {
        let max = self
            .volume
            .as_ref()?
            .values()
            .map(|channel| channel.value)
            .max()?;
        Some(
            ((u64::from(max) * 100 + u64::from(NORMAL_VOLUME / 2)) / u64::from(NORMAL_VOLUME))
                as u32,
        )
    }

    fn channels(&self) -> Vec<u32> {
        let Some(raw) = &self.volume else {
            return Vec::new();
        };
        let channels: Vec<_> = self
            .channel_map
            .split(',')
            .filter_map(|id| raw.get(id.trim()).map(|channel| channel.value))
            .collect();
        if channels.len() == raw.len() && channels.len() <= 32 {
            channels
        } else {
            Vec::new()
        }
    }
}

fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn choices(input: impl IntoIterator<Item = (String, PulseChoice)>) -> Vec<AudioChoice> {
    let mut entries: Vec<_> = input
        .into_iter()
        .filter(|(name, _)| valid_choice(name))
        .collect();
    entries.sort_unstable_by(|(name_a, a), (name_b, b)| {
        b.priority.cmp(&a.priority).then_with(|| name_a.cmp(name_b))
    });
    entries
        .into_iter()
        .take(64)
        .map(|(name, choice)| {
            let description = display_text(choice.description.as_deref().unwrap_or_default());
            AudioChoice {
                description: if description.is_empty() {
                    name.clone()
                } else {
                    description
                },
                name,
                available: choice
                    .available
                    .unwrap_or(choice.availability.as_deref() != Some("not available")),
            }
        })
        .collect()
}

fn valid_choice(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && !name.chars().any(char::is_control)
}

fn cards(text: &str) -> serde_json::Result<Vec<AudioCard>> {
    let mut cards: Vec<_> = serde_json::from_str::<Vec<PulseCard>>(text)?
        .into_iter()
        .filter(|card| valid_name(&card.name))
        .take(64)
        .map(|card| {
            let description = display_text(
                card.properties
                    .get("device.description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            );
            AudioCard {
                description: if description.is_empty() {
                    card.name.clone()
                } else {
                    description
                },
                name: card.name,
                active_profile: card
                    .active_profile
                    .filter(|name| valid_choice(name))
                    .unwrap_or_default(),
                profiles: choices(card.profiles),
            }
        })
        .collect();
    cards.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(cards)
}

fn devices(text: &str) -> serde_json::Result<Vec<Device>> {
    Ok(serde_json::from_str::<Vec<PulseDevice>>(text)?
        .into_iter()
        .filter(|item| valid_name(&item.name))
        .take(128)
        .map(|item| {
            let label = display_text(item.description.as_deref().unwrap_or_default());
            let channels = item.level.channels();
            let stereo = StereoOrder::parse(&item.level.channel_map);
            Device {
                index: item.index,
                description: if label.is_empty() {
                    item.name.clone()
                } else {
                    label
                },
                name: item.name,
                muted: item.level.mute,
                volume: item.level.volume(),
                balance: stereo.and_then(|order| order.balance(&channels)),
                active_port: item
                    .active_port
                    .filter(|name| valid_choice(name))
                    .unwrap_or_default(),
                ports: choices(item.ports.into_iter().map(|port| (port.name, port.choice))),
                stereo,
                channels,
            }
        })
        .collect())
}

fn playback(text: &str, sinks: &[Device]) -> serde_json::Result<Vec<Playback>> {
    let mut streams = serde_json::from_str::<Vec<PulsePlayback>>(text)?;
    streams.sort_unstable_by_key(|stream| stream.index);
    Ok(streams
        .into_iter()
        .take(128)
        .map(|stream| {
            let property = |key| {
                stream
                    .properties
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(display_text)
                    .filter(|text| !text.is_empty())
            };
            let application = property("application.name")
                .or_else(|| property("application.process.binary"))
                .or_else(|| property("application.id"))
                .unwrap_or_default();
            Playback {
                index: stream.index,
                application,
                media: property("media.name").unwrap_or_default(),
                output: sinks
                    .iter()
                    .find(|sink| sink.index == Some(stream.sink))
                    .map_or_else(String::new, |sink| sink.name.clone()),
                muted: stream.level.mute,
                volume: stream.level.volume(),
                client: stream
                    .client
                    .as_ref()
                    .map_or_else(String::new, ToString::to_string),
                channels: stream.level.channels(),
            }
        })
        .collect())
}

pub fn parse_snapshot(
    info: &str,
    sinks: &str,
    sources: &str,
    streams: &str,
    card_data: &str,
) -> serde_json::Result<AudioState> {
    let info: ServerInfo = serde_json::from_str(info)?;
    let sinks = devices(sinks)?;
    Ok(AudioState {
        available: true,
        revision: 0,
        default_sink: info.default_sink_name,
        default_source: info.default_source_name,
        playback: playback(streams, &sinks)?,
        cards: cards(card_data).unwrap_or_default(),
        sinks,
        sources: devices(sources)?,
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.mosaic.Error1")]
pub enum AudioError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Unavailable(String),
    InvalidArgument(String),
    Conflict(String),
    Timeout(String),
    Failed(String),
}

type ResultReply = mpsc::SyncSender<Result<(), AudioError>>;
enum Message {
    Event,
    Action {
        revision: u64,
        action: String,
        device: String,
        value: u32,
        deadline: Instant,
        reply: ResultReply,
    },
}

#[derive(Clone)]
pub struct AudioService {
    state: Arc<Mutex<AudioState>>,
    tx: async_channel::Sender<Message>,
}

impl AudioService {
    pub fn new(epoch: u64) -> (Self, impl FnOnce(zbus::blocking::Connection) -> Runtime) {
        let state = Arc::new(Mutex::new(AudioState {
            revision: epoch,
            ..AudioState::default()
        }));
        let (tx, rx) = async_channel::bounded(8);
        let service = Self {
            state: state.clone(),
            tx: tx.clone(),
        };
        let start = move |connection| {
            let worker_tx = tx.clone();
            let worker = thread::spawn(move || run(&connection, &state, &worker_tx, &rx));
            Runtime {
                tx,
                worker: Some(worker),
            }
        };
        (service, start)
    }

    pub fn snapshot(&self) -> String {
        toml::to_string(&*self.state.lock().unwrap()).unwrap_or_default()
    }

    pub fn apply(
        &self,
        revision: u64,
        action: &str,
        device: &str,
        value: u32,
    ) -> Result<(), AudioError> {
        let stream_index = || {
            device.len() <= 10
                && device.bytes().all(|b| b.is_ascii_digit())
                && device.parse::<u32>().is_ok()
        };
        let valid = match action {
            "sink-volume" | "source-volume" => valid_name(device) && value <= 100,
            "sink-balance" => valid_name(device) && value <= 200,
            "sink-mute" | "source-mute" => valid_name(device) && value <= 1,
            "default-sink" | "default-source" => valid_name(device) && value == 0,
            "stream-volume" => stream_index() && value <= 100,
            "stream-mute" => stream_index() && value <= 1,
            "stream-output" => valid_name(device),
            "card-profile" | "sink-port" | "source-port" => valid_name(device) && value < 64,
            _ => false,
        };
        if !valid {
            return Err(AudioError::InvalidArgument("invalid audio action".into()));
        }
        if !self.state.lock().unwrap().available {
            return Err(AudioError::Unavailable(
                "audio server is unavailable".into(),
            ));
        }
        let (reply, result) = mpsc::sync_channel(1);
        let timeout = Duration::from_secs(4);
        self.tx
            .try_send(Message::Action {
                revision,
                action: action.into(),
                device: device.into(),
                value,
                deadline: Instant::now() + Duration::from_millis(3300),
                reply,
            })
            .map_err(|_| AudioError::Failed("audio queue unavailable or full".into()))?;
        result
            .recv_timeout(timeout)
            .map_err(|_| AudioError::Timeout("audio deadline exceeded".into()))?
    }
}

pub struct Runtime {
    tx: async_channel::Sender<Message>,
    worker: Option<JoinHandle<()>>,
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.tx.close();
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn named_device<'a>(devices: &'a [Device], name: &str) -> Result<&'a Device, AudioError> {
    devices
        .iter()
        .find(|device| device.name == name)
        .ok_or_else(|| AudioError::InvalidArgument("audio device is no longer available".into()))
}

fn selection_arguments<'a>(
    state: &'a AudioState,
    action: &str,
    name: &'a str,
    index: u32,
) -> Result<Vec<&'a str>, AudioError> {
    let (command, options) = match action {
        "card-profile" => (
            "set-card-profile",
            &state
                .cards
                .iter()
                .find(|card| card.name == name)
                .ok_or_else(|| {
                    AudioError::InvalidArgument("audio card is no longer available".into())
                })?
                .profiles,
        ),
        "sink-port" => ("set-sink-port", &named_device(&state.sinks, name)?.ports),
        "source-port" => (
            "set-source-port",
            &named_device(&state.sources, name)?.ports,
        ),
        _ => {
            return Err(AudioError::InvalidArgument(
                "invalid hardware action".into(),
            ));
        }
    };
    let choice = options
        .get(index as usize)
        .filter(|choice| choice.available)
        .ok_or_else(|| {
            AudioError::InvalidArgument("audio profile or port is no longer available".into())
        })?;
    Ok(vec![command, name, &choice.name])
}

fn scaled_volume(channels: &[u32], percent: u32) -> Vec<String> {
    let absolute = percent * NORMAL_VOLUME / 100;
    let max = channels.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return vec![format!("{percent}%")];
    }
    channels
        .iter()
        .map(|value| (u64::from(*value) * u64::from(absolute) / u64::from(max)).to_string())
        .collect()
}

struct Pulse {
    program: &'static str,
    server: String,
}
impl Pulse {
    fn new() -> Option<Self> {
        let server = std::env::var("PULSE_SERVER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("XDG_RUNTIME_DIR")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(|path| format!("unix:{path}/pulse/native"))
            })?;
        Some(Self {
            program: option_env!("MOSAIC_PACTL").unwrap_or("pactl"),
            server,
        })
    }

    fn execute(&self, args: &[&str], deadline: Instant) -> io::Result<String> {
        let limit = deadline
            .saturating_duration_since(Instant::now())
            .min(COMMAND_LIMIT);
        if limit.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut command = vec!["--server", &self.server, "--format=json"];
        command.extend_from_slice(args);
        crate::process::capture_command(
            Command::new(self.program).args(command).env("LC_ALL", "C"),
            limit,
        )
    }

    fn snapshot(&self, deadline: Instant) -> io::Result<AudioState> {
        parse_snapshot(
            &self.execute(&["info"], deadline)?,
            &self.execute(&["list", "sinks"], deadline)?,
            &self.execute(&["list", "sources"], deadline)?,
            &self.execute(&["list", "sink-inputs"], deadline)?,
            &self
                .execute(&["list", "cards"], deadline)
                .unwrap_or_default(),
        )
        .map_err(io::Error::other)
    }

    fn apply(
        &self,
        state: &AudioState,
        revision: u64,
        action: &str,
        name: &str,
        value: u32,
        deadline: Instant,
    ) -> Result<(), AudioError> {
        if !state.available {
            return Err(AudioError::Unavailable(
                "audio server is unavailable".into(),
            ));
        }
        if state.revision != revision {
            return Err(AudioError::Conflict("audio state changed".into()));
        }
        let stream = |index| {
            state
                .playback
                .iter()
                .find(|stream| stream.index == index)
                .ok_or_else(|| {
                    AudioError::InvalidArgument("playback stream no longer available".into())
                })
        };
        let output = || named_device(&state.sinks, name);
        let input = || named_device(&state.sources, name);
        let volume_args;
        let stream_id;
        let mute = if value == 0 { "0" } else { "1" };
        let argv = match action {
            "card-profile" | "sink-port" | "source-port" => {
                selection_arguments(state, action, name, value)?
            }
            "sink-volume" | "source-volume" | "stream-volume" => {
                let (command, channels) = match action {
                    "stream-volume" => {
                        let index = name.parse().map_err(|_| {
                            AudioError::InvalidArgument("invalid stream index".into())
                        })?;
                        ("set-sink-input-volume", stream(index)?.channels.as_slice())
                    }
                    "source-volume" => ("set-source-volume", input()?.channels.as_slice()),
                    _ => ("set-sink-volume", output()?.channels.as_slice()),
                };
                volume_args = scaled_volume(channels, value);
                let mut argv = vec![command, name];
                argv.extend(volume_args.iter().map(String::as_str));
                argv
            }
            "sink-balance" => {
                let output = output()?;
                let channels = output
                    .stereo
                    .and_then(|order| order.set_balance(&output.channels, value))
                    .ok_or_else(|| {
                        AudioError::InvalidArgument("active stereo volume required".into())
                    })?;
                volume_args = channels
                    .into_iter()
                    .map(|value| value.to_string())
                    .collect();
                let mut argv = vec!["set-sink-volume", name];
                argv.extend(volume_args.iter().map(String::as_str));
                argv
            }
            "stream-output" => {
                stream(value)?;
                output()?;
                stream_id = value.to_string();
                vec!["move-sink-input", &stream_id, name]
            }
            "stream-mute" => {
                let index = name
                    .parse()
                    .map_err(|_| AudioError::InvalidArgument("invalid stream index".into()))?;
                stream(index)?;
                vec!["set-sink-input-mute", name, mute]
            }
            "sink-mute" | "default-sink" => {
                output()?;
                if action == "sink-mute" {
                    vec!["set-sink-mute", name, mute]
                } else {
                    vec!["set-default-sink", name]
                }
            }
            "source-mute" | "default-source" => {
                input()?;
                if action == "source-mute" {
                    vec!["set-source-mute", name, mute]
                } else {
                    vec!["set-default-source", name]
                }
            }
            _ => return Err(AudioError::InvalidArgument("unknown audio action".into())),
        };
        self.execute(&argv, deadline).map(|_| ()).map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                AudioError::Timeout("audio command timed out".into())
            } else {
                AudioError::Failed("audio server rejected the operation".into())
            }
        })
    }

    fn subscribe(&self, tx: async_channel::Sender<Message>) -> io::Result<Subscriber> {
        let mut child = Command::new(option_env!("MOSAIC_SETPRIV").unwrap_or("setpriv"))
            .args([
                "--pdeathsig",
                "TERM",
                "--",
                self.program,
                "--server",
                &self.server,
                "subscribe",
            ])
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let flags = Arc::new(Flags::default());
        let signal = flags.clone();
        let reader = thread::spawn(move || {
            let mut stream = BufReader::new(stdout);
            let mut line = Vec::with_capacity(160);
            loop {
                line.clear();
                match (&mut stream).take(512).read_until(b'\n', &mut line) {
                    Ok(n) if n > 0 && line.ends_with(b"\n") => {
                        if [
                            &b" on sink "[..],
                            b" on source ",
                            b" on server ",
                            b" on sink-input ",
                            b" on card ",
                        ]
                        .iter()
                        .any(|event| line.windows(event.len()).any(|part| part == *event))
                            && !signal.dirty.swap(true, Ordering::AcqRel)
                        {
                            let _ = tx.try_send(Message::Event);
                        }
                    }
                    _ => break,
                }
            }
            signal.dead.store(true, Ordering::Release);
            let _ = tx.try_send(Message::Event);
        });
        Ok(Subscriber {
            child,
            flags,
            reader: Some(reader),
        })
    }
}

#[derive(Default)]
struct Flags {
    dirty: AtomicBool,
    dead: AtomicBool,
}
struct Subscriber {
    child: Child,
    flags: Arc<Flags>,
    reader: Option<JoinHandle<()>>,
}
impl Drop for Subscriber {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn publish(
    connection: &zbus::blocking::Connection,
    state: &Mutex<AudioState>,
    mut next: AudioState,
) {
    let mut current = state.lock().unwrap();
    next.revision = current.revision;
    if next == *current {
        return;
    }
    next.revision += 1;
    *current = next;
    drop(current);
    let _ = connection.emit_signal(None::<&str>, PATH, BUS, "AudioChanged", &());
}

fn run(
    connection: &zbus::blocking::Connection,
    state: &Mutex<AudioState>,
    tx: &async_channel::Sender<Message>,
    rx: &async_channel::Receiver<Message>,
) {
    let Some(pulse) = Pulse::new() else { return };
    let mut delay = 1;
    let refresh_deadline = || Instant::now() + Duration::from_secs(3);
    while !rx.is_closed() {
        if let Ok(subscriber) = pulse.subscribe(tx.clone())
            && let Ok(snapshot) = pulse.snapshot(refresh_deadline())
        {
            publish(connection, state, snapshot);
            delay = 1;
            while !rx.is_closed() && !subscriber.flags.dead.load(Ordering::Acquire) {
                if subscriber.flags.dirty.swap(false, Ordering::AcqRel) {
                    thread::sleep(Duration::from_millis(65));
                    // One refresh for all events received during the debounce.
                    subscriber.flags.dirty.store(false, Ordering::Release);
                    match pulse.snapshot(refresh_deadline()) {
                        Ok(snapshot) => publish(connection, state, snapshot),
                        Err(_) => break,
                    }
                }
                match rx.recv_blocking() {
                    Ok(Message::Event) => (),
                    Ok(Message::Action {
                        revision,
                        action,
                        device,
                        value,
                        deadline,
                        reply,
                    }) => {
                        let result = match pulse.snapshot(deadline) {
                            Ok(snapshot) => {
                                publish(connection, state, snapshot);
                                let current = state.lock().unwrap().clone();
                                let result = pulse
                                    .apply(&current, revision, &action, &device, value, deadline);
                                if result.is_ok() {
                                    if let Ok(snapshot) = pulse.snapshot(deadline) {
                                        publish(connection, state, snapshot);
                                    } else {
                                        subscriber.flags.dirty.store(true, Ordering::Release);
                                    }
                                }
                                result
                            }
                            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                                Err(AudioError::Timeout("audio state query timed out".into()))
                            }
                            Err(_) => {
                                publish(connection, state, AudioState::default());
                                Err(AudioError::Unavailable("audio state unavailable".into()))
                            }
                        };
                        let _ = reply.send(result);
                    }
                    Err(_) => break,
                }
            }
        }
        publish(connection, state, AudioState::default());
        if !rx.is_closed() {
            thread::park_timeout(Duration::from_secs(delay));
            delay = (delay * 2).min(30);
        }
    }
}
