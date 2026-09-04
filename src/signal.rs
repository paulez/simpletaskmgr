use anyhow::{Context, Result};
use log::debug;

/// Signals the user can send to a process from the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Hangup (SIGHUP, signal 1)
    Sighup,
    /// Forceful termination (SIGKILL, signal 9)
    Sigkill,
}

impl Signal {
    /// The POSIX signal number for this signal.
    pub fn as_c_int(self) -> libc::c_int {
        match self {
            Signal::Sighup => libc::SIGHUP,
            Signal::Sigkill => libc::SIGKILL,
        }
    }

    /// The conventional name, e.g. `"SIGHUP"`.
    pub fn name(self) -> &'static str {
        match self {
            Signal::Sighup => "SIGHUP",
            Signal::Sigkill => "SIGKILL",
        }
    }
}

/// Sends `signal` to the process with `pid`.
///
/// This is a direct `kill(2)` call; sending a signal to a process owned by
/// another user fails with `EPERM` and is surfaced as an error. A pid that no
/// longer exists fails with `ESRCH`.
pub fn send_signal(pid: i32, signal: Signal) -> Result<()> {
    debug!("Sending {} to pid {pid}", signal.name());
    let ret = unsafe { libc::kill(pid, signal.as_c_int()) };
    if ret == 0 {
        debug!("{} delivered to pid {pid}", signal.name());
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        log::error!("Failed to send {} to pid {pid}: {err:?}", signal.name());
        Err(err).with_context(|| format!("failed to send {} to pid {pid}", signal.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use std::time::Duration;

    /// Spawns a `sleep` process and returns it (still running).
    fn spawn_sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep")
    }

    #[test]
    fn test_signal_names_and_numbers() {
        assert_eq!(Signal::Sighup.as_c_int(), libc::SIGHUP);
        assert_eq!(Signal::Sigkill.as_c_int(), libc::SIGKILL);
        assert_eq!(Signal::Sighup.name(), "SIGHUP");
        assert_eq!(Signal::Sigkill.name(), "SIGKILL");
    }

    /// SIGHUP (default disposition: terminate) and SIGKILL must both
    /// terminate a live process, so the `kill(2)` delivery path is exercised
    /// without depending on the target's handler.
    #[test]
    fn test_signal_delivered_to_process() {
        for (signal, expected) in [(Signal::Sigkill, 9), (Signal::Sighup, 1)] {
            let mut child = spawn_sleeper();
            let pid = child.id() as i32;
            assert!(
                send_signal(pid, signal).is_ok(),
                "sending {signal:?} must succeed"
            );
            let status = child.wait().expect("failed to wait on child");
            assert_eq!(status.code(), None, "should die by signal, not exit");
            #[cfg(unix)]
            assert_eq!(
                status.signal(),
                Some(expected),
                "wrong signal for {signal:?}"
            );
        }
    }

    /// Sending a signal to an unknown pid must fail, not panic.
    #[test]
    fn test_send_signal_unknown_pid_fails() {
        // A pid above the current pid_max is guaranteed to be unused.
        let huge_pid = i32::MAX;
        let err = send_signal(huge_pid, Signal::Sigkill).expect_err("must fail");
        assert!(err.to_string().contains("SIGKILL"), "{err}");
    }

    /// A signal must be delivered promptly; wait on the child with a timeout
    /// guard so the test cannot hang if the signal were never delivered.
    #[test]
    fn test_sigkill_is_prompt() {
        let mut child = spawn_sleeper();
        let pid = child.id() as i32;
        assert!(send_signal(pid, Signal::Sigkill).is_ok());

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "process still alive after 5s"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("wait failed: {e}"),
            }
        }
    }
}
