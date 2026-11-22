use super::{DeviceTarget, Service, bluez_error};
use crate::ipc::ApiError;
use futures_lite::{StreamExt, future};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zbus::{
    Connection, Proxy, fdo::DBusProxy, message::Header, proxy::MethodFlags, zvariant::ObjectPath,
};

const AGENT_PATH: &str = "/org/mosaic/BluetoothPairingAgent";
const PAIR_TIMEOUT: Duration = Duration::from_secs(85);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Confirm,
    Pin,
    Passkey,
    Display,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::Pin => "pin",
            Self::Passkey => "passkey",
            Self::Display => "display",
        }
    }

    fn accepts(self, text: &str) -> bool {
        match self {
            Self::Confirm => text.is_empty(),
            Self::Pin => {
                (1..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
            }
            Self::Passkey => {
                (1..=6).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit())
            }
            Self::Display => false,
        }
    }
}

struct Prompt {
    id: u64,
    kind: Kind,
    code: String,
    answer: Option<async_channel::Sender<Option<String>>>,
}

#[derive(Default)]
struct AgentState {
    next_prompt: u64,
    prompt: Option<Prompt>,
}

pub(super) struct Attempt {
    token: u64,
    requester: String,
    daemon: String,
    target: String,
    state: Mutex<AgentState>,
    cancel: async_channel::Sender<()>,
    active: AtomicBool,
}

struct PairingSlot<'a>(&'a Mutex<Option<Arc<Attempt>>>);
impl Drop for PairingSlot<'_> {
    fn drop(&mut self) {
        if let Some(attempt) = self.0.lock().unwrap().take() {
            attempt.cancel.close();
            attempt.clear_prompt();
        }
    }
}

impl Attempt {
    fn issued_by(&self, header: &Header<'_>) -> bool {
        header
            .sender()
            .is_some_and(|name| name.as_str() == self.daemon)
    }

    fn verify(&self, header: &Header<'_>, device: &ObjectPath<'_>) -> Result<(), AgentError> {
        if self.issued_by(header)
            && device.as_str() == self.target
            && self.active.load(Ordering::Acquire)
            && !self.cancel.is_closed()
        {
            Ok(())
        } else {
            Err(AgentError::Rejected("unexpected pairing request".into()))
        }
    }

    fn clear_prompt(&self) {
        self.state.lock().unwrap().prompt.take();
    }

    fn new_prompt(
        &self,
        kind: Kind,
        code: String,
        answer: Option<async_channel::Sender<Option<String>>>,
    ) -> Result<(), AgentError> {
        let mut state = self.state.lock().unwrap();
        if self.cancel.is_closed() {
            return Err(AgentError::Canceled("pairing was cancelled".into()));
        }
        if let Some(prompt) = state.prompt.as_ref() {
            if prompt.answer.is_some() {
                return Err(AgentError::Rejected("another prompt is pending".into()));
            }
            if prompt.kind == kind && prompt.code == code {
                return Ok(());
            }
        }
        state.next_prompt += 1;
        state.prompt = Some(Prompt {
            id: state.next_prompt,
            kind,
            code,
            answer,
        });
        Ok(())
    }
}

struct Agent(Arc<Attempt>);
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

impl Agent {
    async fn request(
        &self,
        header: &Header<'_>,
        device: &ObjectPath<'_>,
        kind: Kind,
        code: String,
    ) -> Result<String, AgentError> {
        self.0.verify(header, device)?;
        let (send, receive) = async_channel::bounded(1);
        self.0.new_prompt(kind, code, Some(send))?;
        match receive.recv().await {
            Ok(Some(text)) => Ok(text),
            Ok(None) => Err(AgentError::Rejected("user rejected pairing".into())),
            Err(_) => Err(AgentError::Canceled("pairing stopped".into())),
        }
    }

    fn display(
        &self,
        header: &Header<'_>,
        device: &ObjectPath<'_>,
        code: String,
    ) -> Result<(), AgentError> {
        self.0.verify(header, device)?;
        self.0.new_prompt(Kind::Display, code, None)
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn request_confirmation(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        if passkey > 999_999 {
            return Err(AgentError::Rejected("invalid passkey".into()));
        }
        self.request(&header, &device, Kind::Confirm, format!("{passkey:06}"))
            .await
            .map(|_| ())
    }

    fn request_authorization(&self, _device: ObjectPath<'_>) -> Result<(), AgentError> {
        Err(AgentError::Rejected(
            "incoming pairing is outside this agent's scope".into(),
        ))
    }

    async fn request_pin_code(
        &self,
        device: ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<String, AgentError> {
        self.request(&header, &device, Kind::Pin, String::new())
            .await
    }

    async fn request_passkey(
        &self,
        device: ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<u32, AgentError> {
        self.request(&header, &device, Kind::Passkey, String::new())
            .await?
            .parse()
            .map_err(|_| AgentError::Rejected("invalid passkey".into()))
    }

    fn display_pin_code(
        &self,
        device: ObjectPath<'_>,
        pincode: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        if pincode.len() != 6 || !pincode.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AgentError::Rejected("invalid display PIN".into()));
        }
        self.display(&header, &device, pincode.into())
    }

    fn display_passkey(
        &self,
        device: ObjectPath<'_>,
        passkey: u32,
        _entered: u16,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        if passkey > 999_999 {
            return Err(AgentError::Rejected("invalid passkey".into()));
        }
        self.display(&header, &device, format!("{passkey:06}"))
    }

    fn authorize_service(&self, _device: ObjectPath<'_>, _uuid: &str) -> Result<(), AgentError> {
        Err(AgentError::Rejected(
            "this agent is scoped to explicit pairing".into(),
        ))
    }

    fn release(&self, #[zbus(header)] header: Header<'_>) {
        if self.0.issued_by(&header) {
            self.0.clear_prompt();
        }
    }

    fn cancel(&self, #[zbus(header)] header: Header<'_>) {
        if self.0.issued_by(&header) {
            self.0.clear_prompt();
        }
    }
}

impl Service {
    pub async fn pair(
        &self,
        session: &Connection,
        requester: &str,
        token: u64,
        target: DeviceTarget<'_>,
    ) -> Result<(), ApiError> {
        if !self.pairing_enabled {
            return Err(ApiError::PermissionDenied(
                "Mosaic Bluetooth pairing agent is disabled".into(),
            ));
        }
        if token == 0 {
            return Err(ApiError::InvalidArgument("invalid pairing token".into()));
        }
        let (cancel, cancelled) = async_channel::bounded(1);
        let attempt = Arc::new(Attempt {
            token,
            requester: requester.into(),
            daemon: target.owner.into(),
            target: target.path.into(),
            state: Mutex::new(AgentState::default()),
            cancel,
            active: AtomicBool::new(false),
        });
        {
            let mut slot = self.pairing.lock().unwrap();
            if slot.is_some() {
                return Err(ApiError::Conflict(
                    "Bluetooth pairing is already in progress".into(),
                ));
            }
            *slot = Some(attempt.clone());
        }
        let _slot = PairingSlot(&self.pairing);
        let _operation = self.lock_device(&target)?;
        let system = self.bus().await?;
        let (_, adapter, device) = self.device_target(&system, &target).await?;
        if !adapter.powered || device.paired {
            return Err(ApiError::Conflict(
                "Bluetooth device cannot be paired in its current state".into(),
            ));
        }

        let broker = DBusProxy::new(session).await?;
        let mut owners = broker
            .receive_name_owner_changed_with_args(&[(0, requester)])
            .await?;
        if !broker
            .name_has_owner(requester.try_into().map_err(zbus::Error::from)?)
            .await
            .map_err(zbus::Error::from)?
        {
            return Err(cancelled_error());
        }

        let bus = zbus::connection::Builder::system()?
            .method_timeout(PAIR_TIMEOUT)
            .serve_at(AGENT_PATH, Agent(attempt.clone()))?
            .build()
            .await?;
        let registration =
            Proxy::new(&bus, target.owner, "/org/bluez", "org.bluez.AgentManager1").await?;
        let peer = Proxy::new(&bus, target.owner, target.path, super::DEVICE).await?;
        let agent_path = ObjectPath::try_from(AGENT_PATH).map_err(zbus::Error::from)?;
        let work = async {
            let setup = bounded(registration.call_with_flags::<_, _, ()>(
                "RegisterAgent",
                MethodFlags::NoAutoStart.into(),
                &(&agent_path, "KeyboardDisplay"),
            ))
            .await;
            if let Err(error) = setup {
                return (Err(error), false);
            }
            if attempt.cancel.is_closed() {
                return (Err(cancelled_error()), false);
            }
            // Pair and registration deliberately use the same unique connection.
            attempt.active.store(true, Ordering::Release);
            let reply = peer
                .call_with_flags::<_, _, ()>("Pair", MethodFlags::NoAutoStart.into(), &())
                .await
                .map_err(bluez_error)
                .and_then(|value| value.ok_or(zbus::Error::InvalidReply.into()));
            (reply, true)
        };
        let stop = async {
            future::race(
                async {
                    let _ = cancelled.recv().await;
                },
                async {
                    let _ = owners.next().await;
                },
            )
            .await;
            (Err(cancelled_error()), false)
        };
        let (result, completed) = future::or(future::or(work, stop), async {
            async_io::Timer::after(PAIR_TIMEOUT).await;
            (
                Err(ApiError::Timeout("Bluetooth pairing timed out".into())),
                false,
            )
        })
        .await;
        attempt.cancel.close();
        attempt.clear_prompt();
        if completed {
            let _ = bounded(registration.call_with_flags::<_, _, ()>(
                "UnregisterAgent",
                MethodFlags::NoAutoStart.into(),
                &(&agent_path,),
            ))
            .await;
        }
        drop(peer);
        drop(registration);
        // BlueZ cancels pairing associated with a disconnected requester. Closing
        // this dedicated connection never cancels another client's request.
        let _ = bus.close().await;
        result
    }

    fn pairing_for(&self, requester: &str, token: u64) -> Option<Arc<Attempt>> {
        self.pairing
            .lock()
            .unwrap()
            .as_ref()
            .filter(|attempt| {
                attempt.token == token
                    && attempt.requester == requester
                    && !attempt.cancel.is_closed()
            })
            .cloned()
    }

    pub fn pairing_prompt(&self, requester: &str, token: u64) -> (u64, String, String) {
        self.pairing_for(requester, token)
            .and_then(|attempt| {
                let state = attempt.state.lock().unwrap();
                state
                    .prompt
                    .as_ref()
                    .map(|prompt| (prompt.id, prompt.kind.name().into(), prompt.code.clone()))
            })
            .unwrap_or_default()
    }

    pub fn respond_pairing(
        &self,
        requester: &str,
        token: u64,
        id: u64,
        accept: bool,
        text: &str,
    ) -> Result<(), ApiError> {
        let attempt = self
            .pairing_for(requester, token)
            .ok_or_else(prompt_changed)?;
        let mut state = attempt.state.lock().unwrap();
        if attempt.cancel.is_closed() {
            return Err(prompt_changed());
        }
        let prompt = state
            .prompt
            .as_ref()
            .filter(|prompt| prompt.id == id && prompt.answer.is_some())
            .ok_or_else(prompt_changed)?;
        if accept && !prompt.kind.accepts(text) {
            return Err(ApiError::InvalidArgument(
                "invalid Bluetooth pairing input".into(),
            ));
        }
        let prompt = state.prompt.take().unwrap();
        prompt
            .answer
            .unwrap()
            .try_send(accept.then(|| text.to_owned()))
            .map_err(|_| prompt_changed())
    }

    pub fn cancel_pairing(&self, requester: &str, token: u64) {
        if let Some(attempt) = self.pairing_for(requester, token) {
            attempt.cancel.close();
            attempt.clear_prompt();
        }
    }
}

async fn bounded(
    request: impl std::future::Future<Output = zbus::Result<Option<()>>>,
) -> Result<(), ApiError> {
    future::race(
        async {
            request
                .await
                .map_err(bluez_error)?
                .ok_or(zbus::Error::InvalidReply.into())
        },
        async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            Err(ApiError::Timeout(
                "Bluetooth pairing agent did not respond".into(),
            ))
        },
    )
    .await
}

fn cancelled_error() -> ApiError {
    ApiError::Cancelled("Bluetooth pairing cancelled".into())
}

fn prompt_changed() -> ApiError {
    ApiError::Conflict("Bluetooth pairing prompt changed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_agent_requires_explicit_opt_in() {
        assert!(!Service::default().pairing_enabled);
        assert!(Service::with_pairing(true).pairing_enabled);
    }

    #[test]
    fn pairing_input_matches_bluez_constraints() {
        assert!(Kind::Pin.accepts("0123Ab"));
        assert!(!Kind::Pin.accepts(""));
        assert!(!Kind::Pin.accepts("a b"));
        assert!(!Kind::Pin.accepts("12345678901234567"));
        assert!(Kind::Passkey.accepts("000042"));
        assert!(!Kind::Passkey.accepts("1000000"));
        assert!(!Kind::Passkey.accepts("+42"));
        assert!(!Kind::Confirm.accepts("unexpected"));
        assert!(!Kind::Display.accepts(""));
    }
}
