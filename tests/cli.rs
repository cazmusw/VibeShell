use std::process::Command;

#[test]
fn version_is_available_without_wayland() {
    let result = Command::new(env!("CARGO_BIN_EXE_shellctl"))
        .arg("version")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_command_is_rejected() {
    assert_eq!(
        Command::new(env!("CARGO_BIN_EXE_shellctl"))
            .arg("unknown")
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn doctor_without_compositor_reports_failure() {
    let result = Command::new(env!("CARGO_BIN_EXE_shellctl"))
        .arg("doctor")
        .env("WAYLAND_DISPLAY", "/nonexistent/mosaic-test")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stdout).contains("Wayland"));
}

#[test]
fn isolated_wayland_worker_reports_errors_for_parent() {
    let result = Command::new(env!("CARGO_BIN_EXE_shellctl"))
        .arg("__wayland-probe")
        .env("WAYLAND_DISPLAY", "/nonexistent/mosaic-test")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(result.stdout.starts_with(b"ERROR\n"));
}

#[test]
fn full_compositor_accept_queue_cannot_hang_doctor() {
    use std::{
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };
    let directory = std::env::temp_dir().join(format!("mosaic-backlog-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.join("wl");
    let listener = UnixListener::bind(&socket).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let count_worker = Arc::clone(&count);
    let connect_path = socket.clone();
    let filler = std::thread::spawn(move || {
        let mut held_connections = Vec::new();
        while let Ok(stream) = UnixStream::connect(&connect_path) {
            held_connections.push(stream);
            count_worker.fetch_add(1, Ordering::SeqCst);
        }
    });
    let start = Instant::now();
    while count.load(Ordering::SeqCst) < 100 && start.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(50));
    let mut child = Command::new(env!("CARGO_BIN_EXE_shellctl"))
        .arg("doctor")
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let completed = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code() == Some(1);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break false;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(listener);
    filler.join().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        count.load(Ordering::SeqCst) >= 100,
        "test failed to fill the queue"
    );
    assert!(completed, "doctor hung on a full compositor accept queue");
}
