#![forbid(unsafe_code)]
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    time::Instant,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_pointer, wl_registry},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1 as manager, zwlr_virtual_pointer_v1 as pointer,
};
struct State {
    manager: Option<manager::ZwlrVirtualPointerManagerV1>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "zwlr_virtual_pointer_manager_v1" {
                state.manager = Some(registry.bind(name, 1, qh, ()));
            }
        }
    }
}
delegate_noop!(State:ignore manager::ZwlrVirtualPointerManagerV1);
delegate_noop!(State:ignore pointer::ZwlrVirtualPointerV1);
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("missing runtime")?);
    if runtime.starts_with("/run/user")
        || !runtime
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with("mosaic-"))
    {
        return Err("input fixture only supports private mosaic test runtimes".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let width = args.get(1).ok_or("width required")?.parse::<u32>()?;
    let height = args.get(2).ok_or("height required")?.parse::<u32>()?;
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut state = State { manager: None };
    queue.roundtrip(&mut state)?;
    let pointer = state
        .manager
        .as_ref()
        .ok_or("virtual pointer unsupported")?
        .create_virtual_pointer(None, &qh, ());
    queue.roundtrip(&mut state)?;
    let started = Instant::now();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.len() > 128 {
            return Err("oversized command".into());
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let time = started.elapsed().as_millis() as u32;
        match fields.as_slice() {
            ["move", x, y] => pointer.motion_absolute(
                time,
                x.parse::<u32>()?.min(width),
                y.parse::<u32>()?.min(height),
                width,
                height,
            ),
            ["button", code, value] => pointer.button(
                time,
                code.parse()?,
                if *value == "1" {
                    wl_pointer::ButtonState::Pressed
                } else {
                    wl_pointer::ButtonState::Released
                },
            ),
            ["axis", axis, value] => {
                let axis = if *axis == "v" {
                    wl_pointer::Axis::VerticalScroll
                } else {
                    wl_pointer::Axis::HorizontalScroll
                };
                let value: f64 = value.parse()?;
                pointer.axis_source(wl_pointer::AxisSource::Wheel);
                pointer.axis_discrete(time, axis, value, if value < 0. { -1 } else { 1 });
            }
            ["quit"] => break,
            _ => return Err("unknown command".into()),
        }
        pointer.frame();
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        println!("ok");
        io::stdout().flush()?;
    }
    pointer.destroy();
    connection.flush()?;
    Ok(())
}
