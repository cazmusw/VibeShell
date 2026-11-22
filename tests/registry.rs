use mosaic_shell_tools::wayland::query_stream;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

#[test]
fn real_socket_roundtrip_and_remove_event() {
    let (client, mut server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        let mut requests = [0; 24];
        server.read_exact(&mut requests).unwrap();
        let expected: Vec<u8> = [1_u32, (12 << 16) | 1, 2, 1, 12 << 16, 3]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        assert_eq!(requests.as_slice(), expected);
        let body: Vec<u8> = [
            9_u32,
            7,
            u32::from_ne_bytes(*b"wl_s"),
            u32::from_ne_bytes(*b"hm\0\0"),
            1,
        ]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
        let mut events: Vec<u8> = [2_u32, 28 << 16]
            .into_iter()
            .flat_map(u32::to_ne_bytes)
            .collect();
        events.extend(body);
        events.extend(
            [2_u32, (12 << 16) | 1, 9, 3, 12 << 16, 1]
                .into_iter()
                .flat_map(u32::to_ne_bytes),
        );
        for byte in events {
            server.write_all(&[byte]).unwrap();
        }
    });
    assert!(
        query_stream(client, Duration::from_secs(1))
            .unwrap()
            .is_empty()
    );
    worker.join().unwrap();
}

#[test]
fn silent_compositor_has_deadline() {
    let (client, _server) = UnixStream::pair().unwrap();
    let start = Instant::now();
    assert!(query_stream(client, Duration::from_millis(30)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn disconnected_compositor_is_reported() {
    let (client, server) = UnixStream::pair().unwrap();
    drop(server);
    assert!(query_stream(client, Duration::from_millis(30)).is_err());
}
