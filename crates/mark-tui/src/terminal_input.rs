use std::io::{self, Write};

use crossterm::{event::DisableMouseCapture, execute};

/// Disables terminal mouse reporting and discards reports that were already in
/// flight. Mouse reports are escape sequences on Unix terminals, so restoring
/// cooked mode before this settles can leak those sequences into the shell's
/// next prompt.
pub(crate) fn disable_mouse_capture_and_discard_reports(writer: &mut impl Write) -> io::Result<()> {
    execute!(writer, DisableMouseCapture)?;
    writer.flush()?;
    discard_in_flight_input()
}

pub(crate) fn discard_pending_input() -> io::Result<()> {
    discard_input(false)
}

#[cfg(unix)]
fn discard_in_flight_input() -> io::Result<()> {
    discard_input(true)
}

#[cfg(unix)]
fn discard_input(settle_in_flight: bool) -> io::Result<()> {
    use std::{
        fs::OpenOptions,
        os::fd::{AsFd, BorrowedFd},
        thread,
        time::{Duration, Instant},
    };

    use rustix::{
        io::Errno,
        termios::{QueueSelector, isatty, tcflush},
    };

    const SETTLE_TIME: Duration = Duration::from_millis(20);
    const FLUSH_INTERVAL: Duration = Duration::from_millis(2);

    fn flush(fd: BorrowedFd<'_>) -> io::Result<()> {
        match tcflush(fd, QueueSelector::IFlush) {
            Ok(()) | Err(Errno::NOTTY) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn settle(fd: BorrowedFd<'_>) -> io::Result<()> {
        let deadline = Instant::now() + SETTLE_TIME;
        flush(fd)?;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(());
            }
            thread::sleep(FLUSH_INTERVAL.min(deadline.saturating_duration_since(now)));
            flush(fd)?;
        }
    }

    fn discard(fd: BorrowedFd<'_>, settle_in_flight: bool) -> io::Result<()> {
        if settle_in_flight {
            settle(fd)
        } else {
            flush(fd)
        }
    }

    // Crossterm uses fd 0 when it is a tty and the controlling terminal when
    // stdin is redirected. Flush whichever input source crossterm used.
    let stdin = io::stdin();
    if isatty(&stdin) {
        discard(stdin.as_fd(), settle_in_flight)
    } else {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        discard(tty.as_fd(), settle_in_flight)
    }
}

#[cfg(not(unix))]
fn discard_in_flight_input() -> io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn discard_input(_settle_in_flight: bool) -> io::Result<()> {
    Ok(())
}
