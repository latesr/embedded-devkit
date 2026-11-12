//! The blocking driver, over `embedded-io`.

use crate::{Action, CancelSignal, Commands, Config, Input, Reply, Stop, execute, masked};
use core::fmt::Write as _;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use embedded_hal::delay::DelayNs;
use embedded_io::{Read, Write, WriteReady};

/// The console over a blocking transport. `LINE` is the longest command line it takes.
///
/// A command runs as a future that the driver polls until it completes. Under this driver a command must not
/// await anything that needs another task to run, because nothing else runs while it is polled. The driver
/// does not read while a command runs, so Ctrl-C cannot stop one, and [`Cancel::requested`] never completes.
///
/// [`Cancel::requested`]: crate::Cancel::requested
///
/// The write timeout holds through `WriteReady`. The transport's `write_ready` must be true only when a write
/// of at least one byte returns without waiting for the host, and its `flush` must return once the bytes are
/// handed to the hardware, because the driver cannot bound a blocking `flush`. An RTT channel in a blocking
/// mode, or a `flush` that waits for the host to read, blocks the console when no host is there.
pub struct Console<R, W, D, C, const LINE: usize = 128> {
    rx: R,
    out: Output<W, D>,
    commands: C,
    config: Config,
    input: Input<LINE>,
}

impl<R, W, D, C, const LINE: usize> Console<R, W, D, C, LINE>
where
    R: Read,
    W: Write + WriteReady,
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
            },
            commands,
            config,
            input: Input::new(),
        }
    }

    /// Writes the banner and the prompt, then handles every byte until the transport ends or fails.
    pub fn run(&mut self) -> Stop<R::Error> {
        self.start();
        loop {
            if let Err(stop) = self.step() {
                return stop;
            }
        }
    }

    /// Writes the banner and the first prompt.
    pub fn start(&mut self) {
        let banner = crate::banner(&self.config);
        self.out.send(banner.as_str().as_bytes());
        self.prompt();
    }

    /// Reads once and handles what arrived.
    pub fn step(&mut self) -> Result<(), Stop<R::Error>> {
        let mut buf = [0u8; 32];
        let n = self.rx.read(&mut buf).map_err(Stop::Read)?;
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
                    self.out.send(&[echo]);
                }
                Action::Erase => self.out.send(b"\x08 \x08"),
                Action::Cancel => {
                    self.out.send(b"\r\n");
                    self.prompt();
                }
                Action::Line => {
                    self.out.send(b"\r\n");
                    let line = self.input.take();
                    // This driver does not read while a command runs, so nothing raises the signal.
                    let signal = CancelSignal::new();
                    block_on(execute(
                        &mut self.commands,
                        line,
                        &mut self.out,
                        self.config.reset,
                        signal.cancel(),
                    ));
                    self.prompt();
                }
            }
        }
        self.out.flush();
        Ok(())
    }

    fn prompt(&mut self) {
        self.out.send(self.config.name.as_bytes());
        self.out.send(b"> ");
        self.out.flush();
    }
}

/// Polls `future` until it completes. The driver's [`Reply`] never returns `Pending`, so a command that awaits
/// only its reply completes on the first poll.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
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
}

impl<W: Write + WriteReady, D: DelayNs> Output<W, D> {
    fn send(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if !self.host || !self.wait_ready() {
                return;
            }
            match self.tx.write(bytes) {
                Ok(0) | Err(_) => {
                    self.host = false;
                    return;
                }
                Ok(n) => bytes = bytes.get(n..).unwrap_or(&[]),
            }
        }
    }

    fn flush(&mut self) {
        if self.host && self.wait_ready() && self.tx.flush().is_err() {
            self.host = false;
        }
    }

    /// Waits up to the timeout for the transport to take a write. Marks the host absent when it does not.
    fn wait_ready(&mut self) -> bool {
        let mut waited_ms = 0;
        loop {
            match self.tx.write_ready() {
                Ok(true) => return true,
                Ok(false) if waited_ms < self.timeout_ms => {
                    self.delay.delay_ms(1);
                    waited_ms = waited_ms.saturating_add(1);
                }
                _ => {
                    self.host = false;
                    return false;
                }
            }
        }
    }
}

impl<W: Write + WriteReady, D: DelayNs> Reply for Output<W, D> {
    async fn line(&mut self, text: &str) {
        // A line that fits, such as every field line, goes out without a copy.
        if text.len() <= crate::REPLY_LEN {
            self.send(text.as_bytes());
        } else {
            let mut cut = crate::LineBuf::<{ crate::REPLY_LEN }>::new();
            let _ = write!(cut, "{text}");
            self.send(cut.as_str().as_bytes());
        }
        self.send(b"\r\n");
    }

    async fn flush(&mut self) {
        Output::flush(self);
    }
}
