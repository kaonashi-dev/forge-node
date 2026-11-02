//! Bounded subprocess execution for the host's own CLI calls.
//!
//! Every child gets its own process group, a deadline, and a kill aimed at the
//! negative pgid: killing only the direct child leaves a grandchild holding the
//! pipes and the thread waiting on them.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn run_bounded(mut command: Command, timeout: Duration) -> Result<Output, String> {
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    });

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                let stderr = stderr
                    .and_then(|handle| handle.join().ok())
                    .unwrap_or_default();
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_group(&mut child);
                let _ = child.wait();
                if let Some(handle) = stdout {
                    let _ = handle.join();
                }
                if let Some(handle) = stderr {
                    let _ = handle.join();
                }
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                kill_group(&mut child);
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
}

fn kill_group(child: &mut std::process::Child) {
    let pid = nix::unistd::Pid::from_raw(-(child.id() as i32));
    let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
    let _ = child.kill();
}
