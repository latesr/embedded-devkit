//! The async driver, over `embedded-io-async`.

use crate::{Action, CancelSignal, Commands, Config, Input, Reply, Stop, execute, masked};
use core::fmt::Write as _;
use embassy_futures::select::{Either, select};
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};

/// The console over an async transport. `LINE` is the longest command line it takes.
pub struct Console<R, W, D, C, const LINE: usize = 128> {
    rx: R,
    out: Output<W, D>,
    commands: C,
    config: Config,
    input: Input<LINE>,
    cancel: CancelSignal,
}

impl<R, W, D, C, const LINE: usize> Console<R, W, D, C, LINE>
where
    R: Read,
    W: Write,
    D: DelayNs,
    C: Commands,
{
    /// A console over `rx` and `tx`, with `delay` bounding each write.
    pub fn new(rx: R, tx: W, delay: D, commands: C, config: Config) -> Self {
        Self {
            rx,
            out: Output {
                tx,
                delay,
                host: true,
                timeout_ms: config.write_timeout_ms,
                at_line_start: true,
            },
            commands,
            config,
            input: Input::new(),
            cancel: CancelSignal::new(),
        }
    }

    /// Writes the banner and the prompt, then handles every byte until the transport ends or fails.
    pub async fn run(&mut self) -> Stop<R::Error> {
        self.start().await;
        loop {
            if let Err(stop) = self.step().await {
                return stop;
            }
        }
    }

    /// Writes the banner and the first prompt.
    pub async fn start(&mut self) {
        let banner = crate::banner(&self.config);
        self.out.send(banner.as_str().as_bytes()).await;
        self.prompt().await;
    }

    /// Reads once and handles what arrived.
    pub async fn step(&mut self) -> Result<(), Stop<R::Error>> {
        let mut buf = [0u8; 32];
        let n = self.rx.read(&mut buf).await.map_err(Stop::Read)?;
        if n == 0 {
            return Err(Stop::Eof);
        }
        // Input shows a host is there, so output resumes.
        self.out.host = true;
        for &byte in buf.get(..n).unwrap_or(&[]) {
            match self.input.push(byte) {
                Action::None => {}
                Action::Echo(byte) => {
                    let echo = if masked(&self.commands, &self.input) {
                        b'*'
                    } else {
                        byte
                    };
                    self.out.send(&[echo]).await;
                }
                Action::Erase => self.out.send(b"\x08 \x08").await,
                Action::Cancel => {
                    self.out.send(b"\r\n").await;
                    self.prompt().await;
                }
                Action::Line => {
                    self.out.send(b"\r\n").await;
                    let line = self.input.take();
                    self.cancel.clear();
                    let run = execute(
                        &mut self.commands,
                        line,
                        &mut self.out,
                        self.config.reset,
                        self.cancel.cancel(),
                    );
                    match select(run, watch(&mut self.rx, &self.cancel)).await {
                        Either::First(()) => {}
                        Either::Second(Ok(())) => {
                            if !self.out.at_line_start {
                                self.out.send(b"\r\n").await;
                            }
                            self.out.send(b"err interrupted\r\n").await;
                        }
                        Either::Second(Err(stop)) => return Err(stop),
                    }
                    self.prompt().await;
                }
            }
        }
        self.out.flush().await;
        Ok(())
    }

    async fn prompt(&mut self) {
        self.out.send(self.config.name.as_bytes()).await;
        self.out.send(b"> ").await;
        self.out.flush().await;
    }
}

/// Reads the host's input while a command runs. The first Ctrl-C raises `cancel`. A second returns, so the driver
/// drops the command. Every other byte is dropped.
async fn watch<R: Read>(rx: &mut R, cancel: &CancelSignal) -> Result<(), Stop<R::Error>> {
    let mut buf = [0u8; 32];
    loop {
        let n = rx.read(&mut buf).await.map_err(Stop::Read)?;
        if n == 0 {
            return Err(Stop::Eof);
        }
        for &byte in buf.get(..n).unwrap_or(&[]) {
            if byte == 0x03 {
                if cancel.cancel().is_requested() {
                    return Ok(());
                }
                cancel.raise();
            }
        }
    }
}

/// The console's writer: every write bounded, and output dropped while no host reads.
struct Output<W, D> {
    tx: W,
    delay: D,
    /// False after a write timed out, until input arrives.
    host: bool,
    timeout_ms: u32,
    /// False from the start of a write until a write that ends a line completes. A command dropped part way
    /// through a line leaves it false, so the driver ends that line first.
    at_line_start: bool,
}

impl<W: Write, D: DelayNs> Output<W, D> {
    async fn send(&mut self, bytes: &[u8]) {
        if !self.host || bytes.is_empty() {
            return;
        }
        self.at_line_start = false;
        let ends_line = bytes.ends_with(b"\n");
        match select(
            self.tx.write_all(bytes),
            self.delay.delay_ms(self.timeout_ms),
        )
        .await
        {
            Either::First(Ok(())) => self.at_line_start = ends_line,
            Either::First(Err(_)) | Either::Second(()) => self.host = false,
        }
    }

    async fn flush(&mut self) {
        if !self.host {
            return;
        }
        match select(self.tx.flush(), self.delay.delay_ms(self.timeout_ms)).await {
            Either::First(Ok(())) => {}
            Either::First(Err(_)) | Either::Second(()) => self.host = false,
        }
    }
}

impl<W: Write, D: DelayNs> Reply for Output<W, D> {
    async fn line(&mut self, text: &str) {
        // A line that fits, such as every field line, goes out without a copy.
        if text.len() <= crate::REPLY_LEN {
            self.send(text.as_bytes()).await;
        } else {
            let mut cut = crate::LineBuf::<{ crate::REPLY_LEN }>::new();
            let _ = write!(cut, "{text}");
            self.send(cut.as_str().as_bytes()).await;
        }
        self.send(b"\r\n").await;
    }

    async fn flush(&mut self) {
        Output::flush(self).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::{Future, pending};
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};
    use embedded_io_async::ErrorType;

    /// A transport and a delay that complete at once, or never while `stall` is set.
    struct Link {
        stall: bool,
    }

    impl ErrorType for Link {
        type Error = embedded_io::ErrorKind;
    }

    impl Write for Link {
        async fn write(&mut self, bytes: &[u8]) -> Result<usize, Self::Error> {
            if self.stall {
                pending::<()>().await;
            }
            Ok(bytes.len())
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    impl DelayNs for Link {
        async fn delay_ns(&mut self, _: u32) {
            if self.stall {
                pending::<()>().await;
            }
        }
    }

    fn poll_once<F: Future>(future: F) -> Poll<F::Output> {
        pin!(future).poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn output_is_at_a_line_start_only_after_a_completed_write_that_ends_a_line() {
        let mut out = Output {
            tx: Link { stall: false },
            delay: Link { stall: false },
            host: true,
            timeout_ms: 500,
            at_line_start: true,
        };
        assert_eq!(poll_once(out.send(b"busy")), Poll::Ready(()));
        assert!(!out.at_line_start);
        assert_eq!(poll_once(out.send(b"\r\n")), Poll::Ready(()));
        assert!(out.at_line_start);
        out.tx.stall = true;
        out.delay.stall = true;
        assert_eq!(poll_once(out.send(b"done\r\n")), Poll::Pending);
        assert!(!out.at_line_start);
    }
}
