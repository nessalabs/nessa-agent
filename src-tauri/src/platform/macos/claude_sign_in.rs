//! `claude setup-token` run in the background on a terminal of its own.
//!
//! The CLI draws a terminal interface and opens the person's browser itself,
//! so it is given a pseudo-terminal, wide enough that the token it prints is
//! never wrapped, and read until a token shows or it ends. It runs in the
//! person's login shell, where the `claude` their terminal finds is found.

use std::{
    io::Read,
    os::fd::{FromRawFd, OwnedFd},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use crate::provider_authentication::{minted::token_in, LoginFailure};

/// Columns the token's line is drawn in: wider than any token.
const COLUMNS: u16 = 400;
/// How long a CLI that showed its token is given to end by itself.
const SETTLE: Duration = Duration::from_secs(2);

pub(super) fn mint(timeout: Duration) -> Result<String, LoginFailure> {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    mint_with(
        Command::new(shell).args(["-l", "-i", "-c", "exec claude setup-token"]),
        timeout,
    )
}

/// Run `command` on a fresh terminal and answer the token it shows.
pub(super) fn mint_with(command: &mut Command, timeout: Duration) -> Result<String, LoginFailure> {
    let (controller, terminal) = terminal().map_err(|_| LoginFailure::LaunchFailed)?;
    let input = terminal.try_clone().map_err(|_| LoginFailure::LaunchFailed)?;
    let output = terminal.try_clone().map_err(|_| LoginFailure::LaunchFailed)?;
    // SAFETY: setsid and TIOCSCTTY are async-signal-safe and touch only the
    // child, which takes the terminal on its stdin as its controlling one.
    let mut child = unsafe {
        use std::os::unix::process::CommandExt;
        command
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(terminal))
            .pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            })
            .spawn()
    }
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => LoginFailure::Unavailable,
        _ => LoginFailure::LaunchFailed,
    })?;
    // Only the child holds the terminal now: its end is the controller's.
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let result = read_token(controller, timeout);
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Read the controller until a token has shown and the CLI ended or
/// settled, the terminal closed, or `timeout` passed.
fn read_token(controller: OwnedFd, timeout: Duration) -> Result<String, LoginFailure> {
    use std::os::fd::AsRawFd;
    let deadline = Instant::now() + timeout;
    let mut screen = Vec::new();
    let mut shown: Option<Instant> = None;
    let mut file = std::fs::File::from(controller);
    let mut chunk = [0u8; 8192];
    loop {
        let now = Instant::now();
        let until = match shown {
            Some(at) => (at + SETTLE).min(deadline),
            None => deadline,
        };
        if now >= until {
            break;
        }
        let wait = (until - now).as_millis().min(i32::MAX as u128) as i32;
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, wait) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if ready == 0 {
            continue;
        }
        match file.read(&mut chunk) {
            // The CLI ended: on macOS the terminal reads as closed.
            Ok(0) | Err(_) => break,
            Ok(read) => {
                screen.extend_from_slice(&chunk[..read]);
                if shown.is_none() && token_in(&screen).is_some() {
                    shown = Some(Instant::now());
                }
            }
        }
    }
    let token = token_in(&screen).ok_or(LoginFailure::NotCompleted);
    // What was shown held the token; it is not kept past this answer.
    screen.fill(0);
    token
}

/// A new pseudo-terminal, `COLUMNS` wide: its controller and its terminal.
fn terminal() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut controller = 0;
    let mut terminal = 0;
    let mut size = libc::winsize {
        ws_row: 50,
        ws_col: COLUMNS,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two descriptors it opened into the two ints.
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut terminal,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if opened != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both were just opened and are owned by nothing else.
    let owned = unsafe { (OwnedFd::from_raw_fd(controller), OwnedFd::from_raw_fd(terminal)) };
    Ok(owned)
}

#[cfg(test)]
#[path = "../../../tests/platform/macos/claude_sign_in.rs"]
mod tests;
