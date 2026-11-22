use std::{
    io::{self, Read},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const LIMIT: usize = 256 * 1024;

/// Structured argv, bounded output and a wall-clock deadline. Never uses a shell.
pub fn capture(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    timeout: Duration,
) -> io::Result<String> {
    capture_command(Command::new(program).args(args), timeout)
}

pub(crate) fn capture_command(command: &mut Command, timeout: Duration) -> io::Result<String> {
    String::from_utf8(capture_bytes(command, timeout, LIMIT)?)
        .map_err(|_| io::Error::other("command output is not UTF-8"))
}

pub(crate) fn capture_bytes(
    command: &mut Command,
    timeout: Duration,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .and_then(|_| {
                if bytes.len() > limit {
                    Err(io::Error::other("command output exceeds the size limit"))
                } else {
                    Ok(bytes)
                }
            });
        let _ = tx.send(result);
    });
    let result = (|| {
        let mut output = None;
        loop {
            if output.is_none() {
                match rx.try_recv() {
                    Ok(result) => output = Some(result?),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(io::Error::other("output reader disconnected"));
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(io::Error::other(format!("command exited with {status}")));
                }
                if let Some(output) = output {
                    return Ok(output);
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "command deadline exceeded",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}
