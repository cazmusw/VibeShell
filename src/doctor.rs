use crate::wayland::Global;
use crate::{process::capture, wayland};
use std::{env, fmt::Write, path::PathBuf, time::Duration};

pub const REQUIRED: [&str; 5] = [
    "wl_compositor",
    "wl_shm",
    "xdg_wm_base",
    "zwlr_layer_shell_v1",
    "zwlr_foreign_toplevel_manager_v1",
];

pub fn missing_required(globals: &[Global]) -> Vec<&'static str> {
    REQUIRED
        .into_iter()
        .filter(|name| {
            !globals
                .iter()
                .any(|g| &g.interface == name && g.version > 0)
        })
        .collect()
}

pub fn name_owned(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().next() == Some(name))
}

fn socket_path() -> Result<PathBuf, &'static str> {
    if env::var_os("WAYLAND_SOCKET").is_some() {
        return Err(
            "WAYLAND_SOCKET задан: наследуемый fd не используется диагностикой; задайте WAYLAND_DISPLAY",
        );
    }
    let display = env::var_os("WAYLAND_DISPLAY")
        .ok_or("WAYLAND_DISPLAY не задан; запустите doctor из Labwc-сессии")?;
    let path = PathBuf::from(display);
    if path.is_absolute() {
        return Ok(path);
    }
    if path.components().count() != 1 {
        return Err("некорректный WAYLAND_DISPLAY");
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR не задан")?;
    Ok(PathBuf::from(runtime).join(path))
}

pub fn run() -> (String, bool) {
    let mut report = format!(
        "mosaic-shell {} — диагностика платформы\nПроверка только чтения. Присутствие протокола не доказывает работоспособность UI.\n\n",
        env!("CARGO_PKG_VERSION")
    );
    for (label, program, args) in [
        ("Labwc", "labwc", vec!["--version"]),
        ("Nix", "nix", vec!["--version"]),
        ("NixOS", "nixos-version", vec![]),
    ] {
        match capture(program, &args, Duration::from_secs(2)) {
            Ok(version) => {
                let _ = writeln!(report, "{label}: {}", version.trim());
            }
            Err(_) => {
                let _ = writeln!(report, "{label}: версия недоступна");
            }
        }
    }
    // Deadline includes connect(): std UnixStream::connect alone has no timeout.
    let probe = env::current_exe()
        .and_then(|exe| capture(exe, &["__wayland-probe"], Duration::from_secs(3)));
    let mut healthy = false;
    match probe {
        Ok(output) => {
            if let Some((status, body)) = output.split_once('\n') {
                healthy = status == "OK";
                report.push_str(body);
            } else {
                report.push_str("[ОШИБКА] Wayland: некорректный ответ worker\n");
            }
        }
        Err(error) => {
            let _ = writeln!(report, "[ОШИБКА] Wayland: {error}");
        }
    }
    for (scope, names) in [
        (
            "--user",
            vec![
                "org.freedesktop.Notifications",
                "org.kde.StatusNotifierWatcher",
                "org.mosaic.Shell1",
                "org.mosaic.Services1",
            ],
        ),
        (
            "--system",
            vec![
                "org.freedesktop.NetworkManager",
                "org.bluez",
                "org.freedesktop.UPower",
                "org.freedesktop.login1",
                "org.freedesktop.PolicyKit1",
            ],
        ),
    ] {
        let _ = writeln!(report, "\nD-Bus {scope} (владельцы имён, не readiness):");
        match capture(
            "busctl",
            &[
                scope,
                "--timeout=2s",
                "--no-pager",
                "--no-legend",
                "--acquired",
                "list",
            ],
            Duration::from_secs(3),
        ) {
            Ok(listing) => {
                for name in names {
                    let state = if name_owned(&listing, name) {
                        "ДОСТУПЕН"
                    } else {
                        "ВРЕМЕННО НЕДОСТУПЕН"
                    };
                    let _ = writeln!(report, "[{state}] {name}");
                    if name_owned(&listing, name)
                        && [
                            "org.freedesktop.Notifications",
                            "org.kde.StatusNotifierWatcher",
                        ]
                        .contains(&name)
                    {
                        let _ = writeln!(
                            report,
                            "[КОНФЛИКТ РОЛИ] {name} уже занят; существующая служба не изменена"
                        );
                    }
                }
            }
            Err(error) => {
                healthy = false;
                let _ = writeln!(report, "[ОШИБКА] D-Bus: {error}");
            }
        }
    }
    report.push_str("\nЭТАП 0 НЕ ЗАКРЫТ автоматически: doctor не выполняет GTK/window-control probes, приёмку масштабов, lifecycle, audio и lock readiness. Результаты отдельных проверок смотрите в отчёте этапа.\nКод 0 означает доступность обязательных globals и обоих bus, а не приёмку оболочки.\n");
    (report, healthy)
}

/// Internal subprocess worker; parent bounds even a stalled connect().
pub fn wayland_report() -> (String, bool) {
    let mut report = String::new();
    let mut healthy = true;
    let _ = writeln!(report, "\nWayland globals:");
    let registry = socket_path().map_err(str::to_owned).and_then(|path| {
        wayland::registry(&path, Duration::from_secs(2)).map_err(|e| e.to_string())
    });
    match registry {
        Ok(mut globals) => {
            for name in missing_required(&globals) {
                healthy = false;
                let _ = writeln!(report, "[НЕ ПОДДЕРЖИВАЕТСЯ] обязательный {name}");
            }
            globals.sort_by(|a, b| a.interface.cmp(&b.interface));
            for g in &globals {
                let _ = writeln!(report, "[ДОСТУПЕН] {} v{}", g.interface, g.version);
            }
            for name in [
                "wp_fractional_scale_manager_v1",
                "ext_session_lock_manager_v1",
                "xdg_activation_v1",
            ] {
                if !globals.iter().any(|g| g.interface == name) {
                    let _ = writeln!(report, "[НЕ ПОДДЕРЖИВАЕТСЯ] дополнительный {name}");
                }
            }
        }
        Err(error) => {
            healthy = false;
            let _ = writeln!(report, "[ОШИБКА] Wayland: {error}");
        }
    }
    (report, healthy)
}
