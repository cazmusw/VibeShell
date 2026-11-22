use mosaic_shell_tools::diagnostics::Journal;
use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

fn journal_command(path_only: bool) -> io::Result<()> {
    let journal =
        Journal::local().ok_or_else(|| io::Error::other("Не найден каталог состояния"))?;
    if path_only {
        println!("{}", journal.directory().join("events.log").display());
    } else {
        let mut out = io::stdout().lock();
        for line in journal.recent(80)? {
            writeln!(out, "{line}")?;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["temperatures", "status"] => {
            let snapshot = mosaic_shell_tools::temperatures::read_system();
            match toml::to_string(&snapshot) {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("shellctl temperatures: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        #[cfg(feature = "desktop")]
        [
            "launcher" | "applications" | "settings" | "quit" | "windows" | "outputs"
            | "window-outputs" | "control-center",
        ]
        | ["launcher" | "applications" | "control-center", _]
        | ["dialog", "state" | "close"]
        | ["tray", "snapshot" | "state"]
        | ["trash", "status"]
        | ["audio", "status"]
        | ["audio", "set", _, _, _, _]
        | ["wifi", "show" | "status"]
        | ["bluetooth", "show" | "status"]
        | ["bluetooth", "show", _]
        | ["brightness", "show" | "status"]
        | ["brightness", "show", _]
        | ["temperatures", "show"]
        | ["temperatures", "show", _]
        | ["power", "show" | "status"]
        | ["power", "show", _]
        | ["power", "set", _, _, _]
        | ["wifi", "show" | "scan", _]
        | ["wifi", "connect", _, _, _]
        | ["wifi", "disconnect", _, _]
        | ["tray", "menu", _]
        | ["tray", "menu-event", _, _, _]
        | ["dock", "show" | "state" | "cards"]
        | ["dock", "show" | "state" | "cards", _]
        | ["launch" | "pin" | "unpin", _]
        | ["config", "get"]
        | ["config", "set", _]
        | ["reorder", _, _]
        | ["reload"]
        | ["window", _, _, _] => match mosaic_shell_tools::ipc::cli(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("shellctl: {e}");
                ExitCode::FAILURE
            }
        },
        ["version"] | ["--version"] => {
            println!("shellctl {} (stage-4.11)", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["logs"] | ["logs", "--path"] => match journal_command(args.len() == 2) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("shellctl logs: {error}");
                ExitCode::FAILURE
            }
        },
        [
            "__session-event",
            event @ ("start" | "launcher_failed" | "ui_exit"),
            code,
        ] => match code.parse::<u8>() {
            Ok(code) => {
                mosaic_shell_tools::diagnostics::event("session", event, i32::from(code));
                ExitCode::SUCCESS
            }
            Err(_) => ExitCode::from(2),
        },
        ["doctor"] => {
            let (report, healthy) = mosaic_shell_tools::doctor::run();
            if io::stdout().lock().write_all(report.as_bytes()).is_err() {
                return ExitCode::FAILURE;
            }
            if healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        ["__wayland-probe"] => {
            let (report, healthy) = mosaic_shell_tools::doctor::wayland_report();
            let prefix = if healthy { "OK\n" } else { "ERROR\n" };
            let mut stdout = io::stdout().lock();
            if stdout
                .write_all(prefix.as_bytes())
                .and_then(|_| stdout.write_all(report.as_bytes()))
                .is_err()
            {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        ["help"] | ["--help"] | [] => {
            println!(
                "Использование: shellctl doctor | version | launcher [OUTPUT] | applications [OUTPUT] | control-center [OUTPUT] | settings | windows | quit\nКоманды: dialog state|close; dock show|state|cards [OUTPUT]; config get|set TOML; reorder SOURCE BEFORE; reload; outputs; window-outputs; tray state|snapshot; trash status; audio status; audio set REVISION ACTION DEVICE VALUE; wifi show [OUTPUT]; wifi status; wifi scan ADAPTER; wifi connect ADAPTER open|secured SSID; wifi disconnect ADAPTER ACTIVE; bluetooth show [OUTPUT]; bluetooth status; brightness show [OUTPUT]; brightness status."
            );
            println!("Диагностика: shellctl logs [--path]");
            println!("Температуры: shellctl temperatures status | temperatures show [OUTPUT]");
            println!(
                "Питание: shellctl power status | power show [OUTPUT] | power set OWNER EXPECTED PROFILE"
            );
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "Неизвестная команда. Использование: shellctl doctor | version | launcher | applications | control-center | settings | windows | quit"
            );
            ExitCode::from(2)
        }
    }
}
