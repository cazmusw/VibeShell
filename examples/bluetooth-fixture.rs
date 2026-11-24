use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use zbus::{
    fdo::ManagedObjects,
    message::Header,
    names::OwnedInterfaceName,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

const HCI0: &str = "/org/bluez/hci0";
const HCI1: &str = "/org/bluez/hci1";
const KEYBOARD: &str = "/org/bluez/hci0/dev_00_11_22_33_44_66";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const AGENT: &str = "org.bluez.Agent1";
type Properties = HashMap<String, OwnedValue>;
type Shared = Arc<Mutex<State>>;

#[derive(Default)]
struct State {
    powered: bool,
    clients: HashSet<String>,
    external: bool,
    found: bool,
    starts: u32,
    stops: u32,
    agents: HashMap<String, OwnedObjectPath>,
    pairing: Option<String>,
    cancel_requested: bool,
    paired: bool,
    connected: bool,
    trusted: bool,
    registrations: u32,
    unregistrations: u32,
    pairs: u32,
    confirmations: u32,
    rejections: u32,
    cancellations: u32,
    connects: u32,
    concurrent_connect: Option<bool>,
    disconnects: u32,
    removals: u32,
    default_requests: u32,
}

fn path(value: &str) -> OwnedObjectPath {
    ObjectPath::try_from(value).unwrap().into()
}

fn value<'a>(field: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(field.into()).unwrap()
}

fn props(items: impl IntoIterator<Item = (&'static str, OwnedValue)>) -> Properties {
    items
        .into_iter()
        .map(|(key, val)| (key.to_owned(), val))
        .collect()
}

struct Manager(Shared);
#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    fn get_managed_objects(&self) -> ManagedObjects {
        let state = self.0.lock().unwrap();
        let mut objects = ManagedObjects::new();
        let mut attach = |object, interface, properties| {
            objects
                .entry(path(object))
                .or_default()
                .insert(OwnedInterfaceName::try_from(interface).unwrap(), properties);
        };
        attach(
            HCI0,
            ADAPTER,
            props([
                ("Alias", value("Fixture Radio")),
                ("Powered", value(state.powered)),
                (
                    "Discovering",
                    value(state.powered && (state.external || !state.clients.is_empty())),
                ),
            ]),
        );
        attach(
            HCI1,
            ADAPTER,
            props([
                ("Alias", value("Second Radio")),
                ("Powered", value(true)),
                ("Discovering", value(false)),
            ]),
        );
        let headphones = "/org/bluez/hci0/dev_00_11_22_33_44_55";
        attach(
            headphones,
            DEVICE,
            props([
                ("Adapter", value(path(HCI0))),
                ("Alias", value("Fixture Headphones")),
                ("Paired", value(true)),
                ("Connected", value(state.powered)),
                ("Address", value("00:11:22:33:44:55")),
            ]),
        );
        attach(headphones, BATTERY, props([("Percentage", value(0_u8))]));
        if state.found {
            attach(
                KEYBOARD,
                DEVICE,
                props([
                    ("Adapter", value(path(HCI0))),
                    ("Alias", value("Discovered Keyboard")),
                    ("Address", value("00:11:22:33:44:66")),
                    ("Paired", value(state.paired)),
                    ("Connected", value(state.powered && state.connected)),
                    ("Trusted", value(state.trusted)),
                ]),
            );
        }
        attach(
            "/org/bluez/hci1/dev_00_11_22_33_44_77",
            DEVICE,
            props([
                ("Adapter", value(path(HCI1))),
                ("Alias", value("Second Radio Speaker")),
                ("Address", value("00:11:22:33:44:77")),
            ]),
        );
        objects
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum FixtureError {
    #[zbus(error)]
    ZBus(zbus::Error),
    NotReady(String),
    InProgress(String),
    NotAuthorized(String),
    InvalidArguments(String),
    DoesNotExist(String),
    AlreadyExists(String),
    AuthenticationRejected(String),
    AuthenticationCanceled(String),
}

struct FirstAdapter(Shared);
#[zbus::interface(name = "org.bluez.Adapter1")]
impl FirstAdapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.0.lock().unwrap().powered
    }

    #[zbus(property)]
    fn set_powered(&self, powered: bool) {
        let mut state = self.0.lock().unwrap();
        state.powered = powered;
        if !powered {
            state.connected = false;
        }
    }

    fn start_discovery(&self, #[zbus(header)] header: Header<'_>) -> Result<(), FixtureError> {
        let mut state = self.0.lock().unwrap();
        if !state.powered {
            return Err(FixtureError::NotReady("fixture powered off".into()));
        }
        let sender = header.sender().unwrap().to_string();
        if !state.clients.insert(sender) {
            return Err(FixtureError::InProgress("already scanning".into()));
        }
        state.found = true;
        state.starts += 1;
        Ok(())
    }

    fn stop_discovery(&self, #[zbus(header)] header: Header<'_>) -> Result<(), FixtureError> {
        let mut state = self.0.lock().unwrap();
        if !state.clients.remove(header.sender().unwrap().as_str()) {
            return Err(FixtureError::NotAuthorized("not the scan owner".into()));
        }
        state.stops += 1;
        Ok(())
    }

    fn remove_device(&self, device: ObjectPath<'_>) -> Result<(), FixtureError> {
        let mut state = self.0.lock().unwrap();
        if device.as_str() != KEYBOARD || !state.found {
            return Err(FixtureError::DoesNotExist("unknown fixture device".into()));
        }
        if state.pairing.is_some() {
            return Err(FixtureError::InProgress("fixture is pairing".into()));
        }
        state.found = false;
        state.paired = false;
        state.connected = false;
        state.trusted = false;
        state.removals += 1;
        Ok(())
    }
}

struct AgentManager(Shared);
#[zbus::interface(name = "org.bluez.AgentManager1")]
impl AgentManager {
    fn register_agent(
        &self,
        agent: OwnedObjectPath,
        capability: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), FixtureError> {
        if capability != "KeyboardDisplay" {
            return Err(FixtureError::InvalidArguments(
                "expected KeyboardDisplay".into(),
            ));
        }
        let sender = header.sender().unwrap().to_string();
        let mut state = self.0.lock().unwrap();
        if state.agents.contains_key(&sender) {
            return Err(FixtureError::AlreadyExists(
                "agent is already registered".into(),
            ));
        }
        state.agents.insert(sender, agent);
        state.registrations += 1;
        Ok(())
    }

    fn unregister_agent(
        &self,
        agent: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), FixtureError> {
        let sender = header.sender().unwrap().as_str();
        let mut state = self.0.lock().unwrap();
        if state.agents.get(sender) != Some(&agent) {
            return Err(FixtureError::DoesNotExist("agent is not registered".into()));
        }
        state.agents.remove(sender);
        state.unregistrations += 1;
        Ok(())
    }

    fn request_default_agent(&self, _agent: ObjectPath<'_>) -> Result<(), FixtureError> {
        self.0.lock().unwrap().default_requests += 1;
        Err(FixtureError::NotAuthorized(
            "default agent requests are unexpected".into(),
        ))
    }
}

struct Keyboard(Shared);
#[zbus::interface(name = "org.bluez.Device1")]
impl Keyboard {
    async fn pair(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> Result<(), FixtureError> {
        let sender = header.sender().unwrap().to_string();
        let agent = {
            let mut state = self.0.lock().unwrap();
            if !state.powered || !state.found {
                return Err(FixtureError::NotReady("keyboard unavailable".into()));
            }
            if state.paired {
                return Err(FixtureError::AlreadyExists(
                    "keyboard already paired".into(),
                ));
            }
            if state.pairing.is_some() {
                return Err(FixtureError::InProgress("fixture is pairing".into()));
            }
            let agent = state.agents.get(&sender).cloned().ok_or_else(|| {
                FixtureError::NotAuthorized("Pair caller has not registered an agent".into())
            })?;
            state.pairing = Some(sender.clone());
            state.pairs += 1;
            agent
        };
        let incoming = bus
            .call_method(
                Some(sender.as_str()),
                &agent,
                Some(AGENT),
                "RequestAuthorization",
                &(path(KEYBOARD),),
            )
            .await;
        assert!(matches!(incoming, Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.bluez.Error.Rejected"));
        let result = bus
            .call_method(
                Some(sender.as_str()),
                agent,
                Some(AGENT),
                "RequestConfirmation",
                &(path(KEYBOARD), 4207_u32),
            )
            .await;
        let mut state = self.0.lock().unwrap();
        state.pairing = None;
        let canceled = std::mem::take(&mut state.cancel_requested);
        match result {
            Ok(_) if !canceled => {
                state.paired = true;
                state.confirmations += 1;
                Ok(())
            }
            Err(zbus::Error::MethodError(name, _, _))
                if !canceled && name.as_str() == "org.bluez.Error.Rejected" =>
            {
                state.rejections += 1;
                Err(FixtureError::AuthenticationRejected(
                    "confirmation rejected".into(),
                ))
            }
            _ => {
                state.cancellations += 1;
                Err(FixtureError::AuthenticationCanceled(
                    "confirmation canceled".into(),
                ))
            }
        }
    }

    async fn cancel_pairing(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] bus: &zbus::Connection,
    ) -> Result<(), FixtureError> {
        let sender = header.sender().unwrap().as_str();
        let agent = {
            let mut state = self.0.lock().unwrap();
            if state.pairing.as_deref() != Some(sender) {
                return Err(FixtureError::DoesNotExist("no pairing for caller".into()));
            }
            let agent = state.agents.get(sender).cloned().ok_or_else(|| {
                FixtureError::DoesNotExist("pairing agent has unregistered".into())
            })?;
            state.cancel_requested = true;
            agent
        };
        bus.call_method(Some(sender), agent, Some(AGENT), "Cancel", &())
            .await
            .map_err(FixtureError::ZBus)?;
        Ok(())
    }

    fn connect(&self) -> Result<(), FixtureError> {
        let mut state = self.0.lock().unwrap();
        if !state.powered || !state.found || !state.paired {
            return Err(FixtureError::NotReady(
                "keyboard is not paired or ready".into(),
            ));
        }
        if let Some(completes) = state.concurrent_connect.take() {
            if completes {
                let shared = self.0.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(350));
                    let mut state = shared.lock().unwrap();
                    state.connected = true;
                    state.connects += 1;
                });
            }
            return Err(FixtureError::InProgress(
                "another client is connecting".into(),
            ));
        }
        state.connected = true;
        state.connects += 1;
        Ok(())
    }

    fn disconnect(&self) -> Result<(), FixtureError> {
        let mut state = self.0.lock().unwrap();
        if !state.connected {
            return Err(FixtureError::NotReady("keyboard is not connected".into()));
        }
        state.connected = false;
        state.disconnects += 1;
        Ok(())
    }

    #[zbus(property)]
    fn trusted(&self) -> bool {
        self.0.lock().unwrap().trusted
    }

    #[zbus(property)]
    fn set_trusted(&self, trusted: bool) {
        self.0.lock().unwrap().trusted = trusted;
    }
}

struct TestControl(Shared);
#[zbus::interface(name = "org.mosaic.BluetoothFixture")]
impl TestControl {
    fn set_concurrent_connect(&self, completes: bool) {
        self.0.lock().unwrap().concurrent_connect = Some(completes);
    }

    fn set_external_search(&self, active: bool) {
        self.0.lock().unwrap().external = active;
    }

    fn set_external_power(&self, powered: bool) {
        let mut state = self.0.lock().unwrap();
        state.powered = powered;
        if !powered {
            state.clients.clear();
            state.connected = false;
        }
    }

    fn counters(&self) -> (u32, u32, u32) {
        let state = self.0.lock().unwrap();
        (state.starts, state.stops, state.clients.len() as u32)
    }

    fn device_counters(&self) -> (u32, u32, u32, u32, u32, u32, u32, u32, u32, u32) {
        let state = self.0.lock().unwrap();
        (
            state.registrations,
            state.unregistrations,
            state.pairs,
            state.confirmations,
            state.rejections,
            state.cancellations,
            state.connects,
            state.disconnects,
            state.removals,
            state.default_requests,
        )
    }

    fn pairing_state(&self) -> (bool, u32) {
        let state = self.0.lock().unwrap();
        (state.pairing.is_some(), state.agents.len() as u32)
    }
}

fn pairing_client(args: &[String]) -> zbus::Result<()> {
    const SERVICE: &str = "org.mosaic.Services1";
    const SERVICE_PATH: &str = "/org/mosaic/Services1";
    let [owner, adapter, device, address, token] = args else {
        panic!("expected owner, adapter, device, address, and token");
    };
    let token: u64 = token.parse().unwrap();
    let bus = zbus::blocking::Connection::session()?;
    let pairing_bus = bus.clone();
    let target = (
        owner.clone(),
        adapter.clone(),
        device.clone(),
        address.clone(),
        token,
    );
    let attempt = thread::spawn(move || {
        zbus::blocking::Proxy::new(&pairing_bus, SERVICE, SERVICE_PATH, SERVICE)?
            .call::<_, _, ()>("PairBluetooth", &target)
    });
    let proxy = zbus::blocking::Proxy::new(&bus, SERVICE, SERVICE_PATH, SERVICE)?;
    let deadline = Instant::now() + Duration::from_secs(7);
    let (prompt, kind, code) = loop {
        let reply: (u64, String, String) = proxy.call("GetBluetoothPairing", &(token,))?;
        if reply.0 != 0 {
            break reply;
        }
        assert!(
            Instant::now() < deadline && !attempt.is_finished(),
            "no pairing prompt"
        );
        thread::sleep(Duration::from_millis(20));
    };
    println!("{prompt} {kind} {code}");
    io::stdout().flush().unwrap();
    let mut response = String::new();
    io::stdin().read_line(&mut response).unwrap();
    assert_eq!(response.trim(), "reject");
    assert!(
        proxy
            .call::<_, _, ()>("RespondBluetoothPairing", &(token, prompt + 1, true, ""))
            .is_err()
    );
    assert!(
        proxy
            .call::<_, _, ()>("RespondBluetoothPairing", &(token, prompt, true, "invalid"))
            .is_err()
    );
    proxy.call::<_, _, ()>("RespondBluetoothPairing", &(token, prompt, false, ""))?;
    assert!(
        attempt.join().unwrap().is_err(),
        "rejected pairing succeeded"
    );
    let fresh: (u64, String, String) = proxy.call("GetBluetoothPairing", &(token,))?;
    assert_eq!(fresh, (0, String::new(), String::new()));
    Ok(())
}

fn main() -> zbus::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|mode| mode == "--pair-client") {
        return pairing_client(&args[1..]);
    }
    let state = Shared::default();
    let bus = zbus::blocking::connection::Builder::session()?
        .name("org.bluez")?
        .serve_at("/", Manager(state.clone()))?
        .serve_at(HCI0, FirstAdapter(state.clone()))?
        .serve_at(KEYBOARD, Keyboard(state.clone()))?
        .serve_at("/org/bluez", AgentManager(state.clone()))?
        .serve_at("/org/mosaic/BluetoothFixture", TestControl(state.clone()))?
        .build()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus)?;
    for change in dbus.receive_name_owner_changed()? {
        let args = change.args()?;
        if args.name().as_str().starts_with(':') && args.new_owner().is_none() {
            let sender = args.name().as_str();
            let mut state = state.lock().unwrap();
            state.agents.remove(sender);
            if state.pairing.as_deref() == Some(sender) {
                state.cancel_requested = true;
            }
        }
    }
    Ok(())
}
