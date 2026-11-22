use mosaic_shell_tools::wayland::{parse_global, read_event};
use std::io::Cursor;

fn global_body(name: &[u8], version: u32) -> Vec<u8> {
    let mut body = 17_u32.to_ne_bytes().to_vec();
    body.extend_from_slice(&(name.len() as u32).to_ne_bytes());
    body.extend_from_slice(name);
    while !body.len().is_multiple_of(4) {
        body.push(0);
    }
    body.extend_from_slice(&version.to_ne_bytes());
    body
}

#[test]
fn registry_preserves_native_endian_and_global_identity() {
    let g = parse_global(&global_body(b"wl_compositor\0", 6)).unwrap();
    assert_eq!(
        (g.name, g.interface.as_str(), g.version),
        (17, "wl_compositor", 6)
    );
}

#[test]
fn malformed_globals_are_errors_not_panics() {
    for body in [
        vec![],
        vec![0; 8],
        global_body(b"bad", 1),
        global_body(b"a\0b\0", 1),
        global_body(b"\xff\0", 1),
        global_body(b"ok\0", 0),
    ] {
        assert!(parse_global(&body).is_err(), "{body:?}");
    }
    let mut body = global_body(b"wl_shm\0", 1);
    body[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(parse_global(&body).is_err());
}

#[test]
fn every_truncation_of_global_is_rejected() {
    let body = global_body(b"wl_shm\0", 1);
    for n in 0..body.len() {
        assert!(parse_global(&body[..n]).is_err());
    }
}

#[test]
fn header_frames_exactly_one_message() {
    let mut wire = 2_u32.to_ne_bytes().to_vec();
    wire.extend_from_slice(&((12_u32 << 16) | 7).to_ne_bytes());
    wire.extend_from_slice(&99_u32.to_ne_bytes());
    wire.extend_from_slice(b"next");
    let mut cursor = Cursor::new(wire);
    assert_eq!(
        read_event(&mut cursor).unwrap(),
        (2, 7, 99_u32.to_ne_bytes().to_vec())
    );
    assert_eq!(cursor.position(), 12);
}

#[test]
fn invalid_headers_and_eof_are_errors() {
    for size in [0_u32, 4, 7, 9, 65535] {
        let mut wire = 2_u32.to_ne_bytes().to_vec();
        wire.extend_from_slice(&(size << 16).to_ne_bytes());
        assert!(read_event(&mut Cursor::new(wire)).is_err());
    }
    assert!(read_event(&mut Cursor::new(vec![0; 7])).is_err());
}
