//! Flashing and running one image.

use crate::client::check_protocol;
use crate::monitor::{Monitor, Stop};
use crate::test::{TestCase, TestResult, tests_in_elf};
use crate::{Banner, Board, Client, CommandResult, Error, chip};
use defmt_decoder::{Locations, StreamDecoder, Table};
use object::{Object, ObjectSection};
use probe_rs::config::{Registry, TargetSelector};
use probe_rs::flashing::{
    DownloadOptions, ElfLoader, ElfOptions, FlashProgress, ImageLoader, ProgressEvent,
    download_file_with_options,
};
use probe_rs::probe::DebugProbeSelector;
use probe_rs::probe::list::Lister;
use probe_rs::rtt::{Rtt, RttTarget, ScanRegion};
use probe_rs::semihosting::SemihostingCommand;
use probe_rs::{BreakpointCause, Core, CoreStatus, HaltReason, Permissions, Session, Target};
use probe_rs_espressif::image_format::IdfLoader;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The firmware to flash and how to flash it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Image {
    /// The ELF file.
    pub elf: PathBuf,
    /// The chip's name in probe-rs, such as `esp32c5` or `STM32H7S3L8`.
    pub chip: String,
    /// A chip description in probe-rs's YAML, loaded in place of the built-in one for its family.
    pub chip_description: Option<PathBuf>,
    /// The flash algorithm that writes its range of flash. The other algorithms for that range are removed, so
    /// probe-rs cannot choose one of them. `None` lets probe-rs choose.
    pub preferred_flash_algorithm: Option<String>,
    /// The ESP-IDF partition table, a CSV file. `None` takes probe-rs's default table.
    pub partition_table: Option<PathBuf>,
    /// The partition the app goes into, by its name in the table. `None` takes the first app partition.
    pub app_partition: Option<String>,
    /// The bootloader to flash with the app. `None` takes probe-rs's built-in bootloader for the chip.
    pub bootloader: Option<PathBuf>,
    /// Skip the flash pages that already hold the image's bytes.
    pub preverify: bool,
    /// Read the flash back after the write and compare it with the image.
    pub verify: bool,
    /// The SPI flash mode an ESP-IDF image boots in. `None` takes [`FlashMode::Qio`]. A chip that does not boot
    /// an ESP-IDF image ignores it.
    pub flash_mode: Option<FlashMode>,
}

/// How an ESP32's SPI flash transfers the address and the data of a read. The bootloader sets the mode, and the
/// app keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    /// Address and data on four lines.
    Qio,
    /// Address on one line, data on four.
    Qout,
    /// Address and data on two lines.
    Dio,
    /// Address on one line, data on two.
    Dout,
}

impl std::str::FromStr for FlashMode {
    type Err = String;

    /// Reads `qio`, `qout`, `dio` or `dout`, ignoring case.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.to_ascii_lowercase().as_str() {
            "qio" => Ok(Self::Qio),
            "qout" => Ok(Self::Qout),
            "dio" => Ok(Self::Dio),
            "dout" => Ok(Self::Dout),
            _ => Err(format!(
                "{text} is not a flash mode: qio, qout, dio or dout"
            )),
        }
    }
}

/// The mode an ESP-IDF image of `image` boots in. Quad reads move twice the bits per clock of dual ones. A board
/// with a flash that cannot run them names another mode in its ELF or with `--flash-mode`.
fn idf_flash_mode(image: &Image) -> espflash::flasher::FlashMode {
    use espflash::flasher::FlashMode as Loader;
    match image.flash_mode.unwrap_or(FlashMode::Qio) {
        FlashMode::Qio => Loader::Qio,
        FlashMode::Qout => Loader::Qout,
        FlashMode::Dio => Loader::Dio,
        FlashMode::Dout => Loader::Dout,
    }
}

impl Image {
    /// The image of `elf`, with the chip from its `.probe-rs.chip` section, the preferred flash algorithm from
    /// its `.mcu-run.flash-algorithm` section and the flash mode from its `.mcu-run.flash-mode` section. Each
    /// section holds the name as UTF-8. The other fields take their defaults. Fails when the file cannot be read,
    /// is not an ELF, names no chip, or names a flash mode that is not one.
    pub fn from_elf(elf: PathBuf) -> Result<Self, Error> {
        let failed = |error: String| Error::Elf {
            path: elf.clone(),
            error,
        };
        let bytes = std::fs::read(&elf).map_err(|error| failed(error.to_string()))?;
        let file = object::File::parse(&*bytes).map_err(|error| failed(error.to_string()))?;
        let name = |section: &str| -> Result<Option<String>, Error> {
            let Some(found) = file.section_by_name(section) else {
                return Ok(None);
            };
            let data = found.data().map_err(|error| failed(error.to_string()))?;
            let text = std::str::from_utf8(data)
                .map_err(|_| failed(format!("the {section} section is not UTF-8")))?;
            Ok((!text.is_empty()).then(|| text.to_owned()))
        };
        let chip = name(".probe-rs.chip")?.ok_or_else(|| Error::NoChip { elf: elf.clone() })?;
        let preferred_flash_algorithm = name(".mcu-run.flash-algorithm")?;
        let flash_mode = name(".mcu-run.flash-mode")?
            .map(|mode| mode.parse())
            .transpose()
            .map_err(|error| failed(format!("the .mcu-run.flash-mode section: {error}")))?;
        Ok(Self {
            elf,
            chip,
            preferred_flash_algorithm,
            flash_mode,
            ..Self::default()
        })
    }

    /// The image's ID in lower-case hex, as the console's banner names it. An ESP-IDF image, which has a
    /// `.flash.appdesc` section, has the SHA-256 of its ELF file, which espflash writes into the app descriptor.
    /// Any other image has the GNU build ID in its notes. `None` when neither is there. Fails when the file
    /// cannot be read or is not an ELF.
    pub fn id(&self) -> Result<Option<String>, Error> {
        let failed = |error: String| Error::Elf {
            path: self.elf.clone(),
            error,
        };
        let bytes = std::fs::read(&self.elf).map_err(|error| failed(error.to_string()))?;
        let file = object::File::parse(&*bytes).map_err(|error| failed(error.to_string()))?;
        let hex = |id: &[u8]| id.iter().map(|byte| format!("{byte:02x}")).collect();
        if file.section_by_name(".flash.appdesc").is_some() {
            return Ok(Some(hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes))));
        }
        let id = file.build_id().map_err(|error| failed(error.to_string()))?;
        Ok(id.map(hex))
    }
}

/// Whether `banner` names the image with the ID `elf`. Only an ID on both sides that is the same says that the
/// board runs the image.
fn runs_image(banner: &Banner, elf: Option<&str>) -> bool {
    matches!((banner.image.as_deref(), elf), (Some(running), Some(built)) if running == built)
}

/// Writes one line to `to` that says how a console's image started: flashed, or already running and reset. A test
/// harness captures `eprintln!` and shows it only for a failed test. A write to the stderr handle goes past that
/// capture, so a passing test shows whether it flashed. A failed write is ignored.
fn started(elf: &Path, flashed: bool, mut to: impl Write) {
    let name = elf.file_name().unwrap_or(elf.as_os_str()).to_string_lossy();
    let _ = if flashed {
        writeln!(to, "mcu: flashed {name}")
    } else {
        writeln!(to, "mcu: {name} already runs, reset")
    };
}

/// Sends `reset` to the console and waits for the next banner. Returns the banner when the console answered `ok`
/// within `answer` and the banner came within `boot` after that.
fn reset_through<T: Read + Write>(
    client: &mut Client<T>,
    answer: Duration,
    boot: Duration,
) -> Option<Banner> {
    let answered = client
        .send("reset", answer)
        .is_ok_and(|reply| reply.result == CommandResult::Ok);
    if !answered {
        return None;
    }
    client.wait_for_banner(boot).ok()
}

/// How long a console may take to answer `reset` before [`Runner::console`] flashes instead. A console writes `ok`
/// before it resets, so the answer is quick, and firmware without a console costs no more than this.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a started image may take to write its banner.
const CONSOLE_TIMEOUT: Duration = Duration::from_secs(10);

/// The transport of a console that [`Runner::console`] or [`Runner::fresh_console`] returns. It reads and writes
/// the board's console port. A test names `Client<Transport>` where it keeps or passes the console.
pub struct Transport(Box<dyn Stream>);

/// What a [`Transport`] holds. Each function that returns `impl Read + Write` has a type of its own, so the
/// transport boxes it.
trait Stream: Read + Write {}

impl<T: Read + Write> Stream for T {}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// The board's console port, opened.
fn port_transport(board: &Board) -> Result<Transport, Error> {
    Ok(Transport(Box::new(crate::open_port(&crate::port(board)?)?)))
}

/// Flashes and runs one [`Image`] on one board.
pub struct Runner {
    image: Image,
    registry: Registry,
    target: Target,
}

/// How long the core may take to reset and halt.
const RESET_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the run sleeps when the board had nothing to say.
const IDLE: Duration = Duration::from_millis(10);

/// embedded-test's timeout for a test without its own.
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

impl Runner {
    /// A runner for `image`. Loads the chip description, resolves the chip and applies the preferred flash
    /// algorithm, and fails when any of them is wrong.
    pub fn new(image: Image) -> Result<Self, Error> {
        let mut registry = chip::registry(image.chip_description.as_deref())?;
        if let Some(preferred) = &image.preferred_flash_algorithm {
            chip::prefer_flash_algorithm(&mut registry, &image.chip, preferred)?;
        }
        let target = chip::target(&registry, &image.chip)?;
        Ok(Self {
            image,
            registry,
            target,
        })
    }

    /// The board to run the image on. With `device`, or with fewer than two boards attached, the board that
    /// [`select`](crate::select) finds. Otherwise the one board with a probe that can debug the chip's
    /// architecture and, when more than one can, with a JTAG IDCODE that names the chip or names none. Fails as `select` does, with
    /// only the boards that are left.
    pub fn board(&self, device: Option<&str>) -> Result<Board, Error> {
        crate::detect::choose(
            &crate::attached(),
            device,
            self.target.architecture(),
            &self.target.name,
            crate::detect::debugs,
            |board| crate::detect::with_chip(board, &self.registry),
        )
    }

    /// The image this runner flashes.
    pub fn image(&self) -> &Image {
        &self.image
    }

    /// The chip's name as the chip description spells it.
    pub fn chip(&self) -> &str {
        &self.target.name
    }

    /// Whether the image is an embedded-test binary, which [`test`](Self::test) runs.
    pub fn is_test_binary(&self) -> Result<bool, Error> {
        Ok(tests_in_elf(&self.read_elf()?)?.is_some())
    }

    /// Flashes the image to `board`, resets and halts the chip, blanks the RTT control block, runs, and
    /// follows the board's output until the run stops.
    ///
    /// The output goes to stdout, and to `log` when given. The run stops at a line that contains `until`, at
    /// `timeout` after the firmware starts, when the firmware exits through semihosting, or when the core halts
    /// for another reason. Fails when the probe, the flash or the core cannot be used. Fails with
    /// [`Error::PortInUse`], before the flash, when another process holds the board's console port.
    pub fn run(
        &self,
        board: &Board,
        until: Option<&str>,
        timeout: Option<Duration>,
        log: Option<&Path>,
    ) -> Result<Stop, Error> {
        let (elf, table, mut session) = self.start(board)?;
        let mut output = Output::new(&elf, table.as_ref(), &self.image.elf, until, timeout, log)?;
        let mut on_command = |command, core: &mut Core<'_>, output: &mut Output<'_>| {
            Ok(match command {
                SemihostingCommand::ExitSuccess => Some(Stop::Exited(0)),
                SemihostingCommand::ExitError(details) => {
                    let code = details.exit_status.unwrap_or(1);
                    Some(Stop::Exited(u8::try_from(code).unwrap_or(u8::MAX)))
                }
                SemihostingCommand::GetCommandLine(request) => {
                    request
                        .write_command_line_to_target(core, "")
                        .map_err(|error| Error::Run(error.to_string()))?;
                    None
                }
                other => {
                    output.answer(other, core)?;
                    None
                }
            })
        };
        let mut followed = {
            let mut core = session
                .core(0)
                .map_err(|error| Error::Run(error.to_string()))?;
            output.restart(&mut core)?;
            unless_gone(board, output.follow(&mut core, None, &mut on_command))?
        };
        let mut last_lost: Option<Instant> = None;
        let stop = loop {
            let error = match followed {
                Followed::Stop(stop) => break stop,
                Followed::CoreLost(error) => error,
            };
            // Losing the core twice in a row means the reset was not what lost it.
            if last_lost.is_some_and(|lost| lost.elapsed() < CORE_LOST_AGAIN) {
                return Err(Error::Run(error));
            }
            last_lost = Some(Instant::now());
            // A reset can take the chip's debug transport down with it, after which probe-rs reads the debug
            // module as all zeroes, and a reset button takes the probe off the USB bus. A new session sets the
            // transport up again. Attaching does not reset the chip.
            drop(session);
            session = self.reattach(board)?;
            if let Some(stop) = output.after_reset()? {
                break stop;
            }
            let mut core = session
                .core(0)
                .map_err(|error| Error::Run(error.to_string()))?;
            followed = unless_gone(board, output.follow(&mut core, None, &mut on_command))?;
        };
        output.monitor.finish()?;
        Ok(stop)
    }

    /// A client on the image's console on `board`, just after the image starts, with its banner read.
    ///
    /// It first sends `reset` to whatever firmware the board runs. When the console answers `ok` within 2 s and
    /// its next banner names this image's ID ([`Image::id`]), the board already runs the image, and the reset has
    /// restarted it. A software reset keeps the USB link, so the port stays open. Otherwise the image is flashed
    /// and started. The port opens before the flash, so the client reads the banner of the new image and nothing
    /// that the board ran before answers it.
    ///
    /// Fails when the port, the probe or the flash cannot be used, or when no banner arrives. Fails with
    /// [`Error::Protocol`] when the banner names another protocol version than [`PROTOCOL`](crate::PROTOCOL). Fails
    /// with [`Error::PortInUse`], before the reset or the flash, when another process holds the board's console
    /// port.
    pub fn console(&self, board: &Board) -> Result<Client<Transport>, Error> {
        crate::serial::check_free(board)?;
        // An ELF that cannot be read gives no ID, so the board is flashed, and the flash reports why the ELF cannot
        // be read.
        let id = self.image.id().ok().flatten();
        if let Ok(transport) = port_transport(board) {
            let mut client = Client::new(transport);
            let banner = reset_through(&mut client, ANSWER_TIMEOUT, CONSOLE_TIMEOUT);
            if let Some(banner) = banner.filter(|banner| runs_image(banner, id.as_deref())) {
                check_protocol(&banner)?;
                started(&self.image.elf, false, std::io::stderr());
                return Ok(client);
            }
        }
        self.flashed(board)
    }

    /// A client on the image's console on `board`, just after the image is flashed and started, with its banner
    /// read. It flashes even when the board already runs the image, so a test starts from a first boot. Fails as
    /// [`console`](Self::console) does.
    pub fn fresh_console(&self, board: &Board) -> Result<Client<Transport>, Error> {
        self.flashed(board)
    }

    /// Flashes and starts the image, with the port open first, and reads its banner.
    fn flashed(&self, board: &Board) -> Result<Client<Transport>, Error> {
        let mut client = Client::new(port_transport(board)?);
        let (_, _, mut session) = self.start(board)?;
        {
            let mut core = session
                .core(0)
                .map_err(|error| Error::Run(error.to_string()))?;
            core.reset_and_halt(RESET_TIMEOUT)
                .map_err(|error| Error::Run(error.to_string()))?;
            core.run().map_err(|error| Error::Run(error.to_string()))?;
        }
        drop(session);
        let banner = client.wait_for_banner(CONSOLE_TIMEOUT)?;
        check_protocol(&banner)?;
        started(&self.image.elf, true, std::io::stderr());
        Ok(client)
    }

    /// Flashes an embedded-test binary to `board` and runs each of its tests, with a reset before each one.
    /// Prints one line per test and returns every result. The board's output goes to stdout, and to `log` when
    /// given. Fails when the image is not an embedded-test binary. Fails with [`Error::PortInUse`], before the
    /// flash, when another process holds the board's console port.
    pub fn test(&self, board: &Board, log: Option<&Path>) -> Result<Vec<TestResult>, Error> {
        let (elf, table, mut session) = self.start(board)?;
        let tests = tests_in_elf(&elf)?.ok_or_else(|| {
            Error::TestList(format!(
                "{} holds no embedded-test tests",
                self.image.elf.display()
            ))
        })?;
        let mut output = Output::new(&elf, table.as_ref(), &self.image.elf, None, None, log)?;
        let mut core = session
            .core(0)
            .map_err(|error| Error::Run(error.to_string()))?;
        println!("running {} tests", tests.len());
        let mut results = Vec::with_capacity(tests.len());
        for test in &tests {
            let result = if test.ignored {
                TestResult::Ignored {
                    name: test.name.clone(),
                }
            } else {
                output.run_test(&mut core, test)?
            };
            println!(
                "test {} ... {}",
                result.name(),
                match &result {
                    TestResult::Passed { .. } => "ok".to_owned(),
                    TestResult::Failed { reason, .. } => format!("FAILED: {reason}"),
                    TestResult::Ignored { .. } => "ignored".to_owned(),
                }
            );
            results.push(result);
        }
        output.monitor.finish()?;
        let passed = results
            .iter()
            .filter(|result| matches!(result, TestResult::Passed { .. }))
            .count();
        let failed = results
            .iter()
            .filter(|result| matches!(result, TestResult::Failed { .. }))
            .count();
        let ignored = results.len().saturating_sub(passed).saturating_sub(failed);
        println!(
            "test result: {}. {passed} passed; {failed} failed; {ignored} ignored",
            if failed == 0 { "ok" } else { "FAILED" }
        );
        Ok(results)
    }

    fn read_elf(&self) -> Result<Vec<u8>, Error> {
        std::fs::read(&self.image.elf).map_err(|error| Error::Elf {
            path: self.image.elf.clone(),
            error: error.to_string(),
        })
    }

    /// Reads the ELF and its defmt table, attaches to the board and flashes. What every run starts with.
    fn start(&self, board: &Board) -> Result<(Vec<u8>, Option<Table>, Session), Error> {
        crate::serial::check_free(board)?;
        let elf = self.read_elf()?;
        let table = Table::parse(&elf).map_err(|error| Error::Defmt(error.to_string()))?;
        let mut session = self.attach(board)?;
        self.flash(&mut session)?;
        Ok((elf, table, session))
    }

    /// Attaches to `board` again after a reset, trying until [`BOARD_RETURN`] has passed. A reset button takes the
    /// probe off the USB bus a moment after the chip resets, so an attempt can fail while the board is still
    /// listed, and the next waits for it to return.
    fn reattach(&self, board: &Board) -> Result<Session, Error> {
        let start = Instant::now();
        loop {
            match self.attach(board) {
                Ok(session) => return Ok(session),
                Err(error) => {
                    let left = BOARD_RETURN.saturating_sub(start.elapsed());
                    if left.is_zero() {
                        return Err(error);
                    }
                    crate::board::wait_for_board(board, left)?;
                    std::thread::sleep(IDLE);
                }
            }
        }
    }

    fn attach(&self, board: &Board) -> Result<Session, Error> {
        let selector: DebugProbeSelector = board
            .selector
            .parse()
            .map_err(|error| Error::Probe(format!("selector {}: {error}", board.selector)))?;
        let probe = crate::probe_thread::on_probe_thread(move || Lister::new().open(selector))
            .map_err(|error| Error::Probe(error.to_string()))?;
        probe
            .attach_with_registry(
                TargetSelector::Specified(Box::new(self.target.clone())),
                Permissions::default(),
                &self.registry,
            )
            .map_err(|error| Error::Probe(error.to_string()))
    }

    /// Flashes the image with the loader the chip description names. An `idf` chip takes the ESP-IDF image
    /// with its bootloader and partition table, and any other chip takes the ELF as it is.
    fn flash(&self, session: &mut Session) -> Result<(), Error> {
        if self.target.default_format.as_deref() == Some("idf") {
            let loader = IdfLoader {
                bootloader: self.image.bootloader.clone(),
                partition_table: self.image.partition_table.clone(),
                target_app_partition: self.image.app_partition.clone(),
                flash_mode: Some(idf_flash_mode(&self.image)),
                flash_frequency: None,
            };
            self.download(session, loader)
        } else {
            self.download(session, ElfLoader(ElfOptions::default()))
        }
    }

    fn download(&self, session: &mut Session, loader: impl ImageLoader) -> Result<(), Error> {
        let started = Instant::now();
        let mut options = DownloadOptions::default();
        options.preverify = self.image.preverify;
        options.verify = self.image.verify;
        options.progress = FlashProgress::new(|event| match event {
            ProgressEvent::Started(operation) => eprintln!("mcu: {operation:?}"),
            ProgressEvent::Finished(operation) => eprintln!(
                "mcu: {operation:?} done at {:.1} s",
                started.elapsed().as_secs_f32()
            ),
            ProgressEvent::Failed(operation) => eprintln!("mcu: {operation:?} failed"),
            ProgressEvent::DiagnosticMessage { message } => eprint!("mcu: {message}"),
            _ => {}
        });
        download_file_with_options(session, &self.image.elf, loader, options)
            .map_err(|error| Error::Flash(error.to_string()))
    }
}

/// What the run decides on a semihosting command: stop with this, or let the core run on.
type OnCommand<'a> = dyn FnMut(SemihostingCommand, &mut Core<'_>, &mut Output<'_>) -> Result<Option<Stop>, Error>
    + 'a;

/// The board's output during a run: RTT with defmt decoding, and the monitor that prints, logs and stops.
struct Output<'e> {
    monitor: Monitor<std::io::Stdout>,
    /// The ELF's defmt table, when it has one.
    table: Option<&'e Table>,
    locations: Locations,
    /// Where the RTT control block is, from the ELF, or all of RAM when the ELF has no `_SEGGER_RTT`.
    region: ScanRegion,
    rtt: Option<Rtt>,
    /// The defmt stream decoder. It holds a frame in progress, so it lives from one RTT attach to the next.
    decoder: Option<Box<dyn StreamDecoder + Send + Sync + 'e>>,
}

impl<'e> Output<'e> {
    fn new(
        elf: &[u8],
        table: Option<&'e Table>,
        path: &Path,
        until: Option<&str>,
        timeout: Option<Duration>,
        log: Option<&Path>,
    ) -> Result<Self, Error> {
        let rtt_address =
            probe_rs::rtt::find_rtt_control_block_in_raw_file(elf).map_err(|error| Error::Elf {
                path: path.to_path_buf(),
                error: error.to_string(),
            })?;
        let locations = match table {
            Some(table) => table
                .get_locations(elf)
                .map_err(|error| Error::Defmt(error.to_string()))?,
            None => Locations::default(),
        };
        Ok(Self {
            monitor: Monitor::new(std::io::stdout(), until, timeout, log, Instant::now())?,
            table,
            locations,
            region: rtt_address.map_or(ScanRegion::Ram, ScanRegion::Exact),
            rtt: None,
            decoder: None,
        })
    }

    /// Resets and halts the core, blanks the RTT control block, forgets the last run's RTT state and lets
    /// the core run.
    fn restart(&mut self, core: &mut Core<'_>) -> Result<(), Error> {
        core.reset_and_halt(RESET_TIMEOUT)
            .map_err(|error| Error::Run(error.to_string()))?;
        if let ScanRegion::Exact(_) = self.region {
            let target = RttTarget::from(&*core);
            Rtt::clear_control_block(core, &target, &self.region)
                .map_err(|error| Error::Run(error.to_string()))?;
        }
        self.rtt = None;
        self.decoder = None;
        core.run().map_err(|error| Error::Run(error.to_string()))
    }

    /// Follows the core until the output, the deadline, a halt or `on_command` stops the run, or until the
    /// core's state can no longer be read.
    fn follow(
        &mut self,
        core: &mut Core<'_>,
        deadline: Option<Instant>,
        on_command: &mut OnCommand<'_>,
    ) -> Result<Followed, Error> {
        loop {
            let (followed, busy) = self.poll(core)?;
            if let Some(followed) = followed {
                return Ok(followed);
            }
            match self.halt(core, on_command)? {
                Some(Followed::Stop(stop)) => {
                    // The firmware can write its last lines and exit between the poll and the halt check. They
                    // are still in the RTT buffer, and the halted core lets them be read.
                    self.poll(core)?;
                    return Ok(Followed::Stop(stop));
                }
                Some(lost) => return Ok(lost),
                None => {}
            }
            let now = Instant::now();
            if let Some(stop) = self.monitor.tick(now) {
                return Ok(Followed::Stop(stop));
            }
            if deadline.is_some_and(|deadline| now >= deadline) {
                return Ok(Followed::Stop(Stop::Deadline));
            }
            if !busy {
                std::thread::sleep(IDLE);
            }
        }
    }

    /// Runs one test from a reset and says how it ended.
    fn run_test(&mut self, core: &mut Core<'_>, test: &TestCase) -> Result<TestResult, Error> {
        self.restart(core)?;
        let timeout = test
            .timeout
            .map_or(TEST_TIMEOUT, |seconds| Duration::from_secs(seconds.into()));
        let deadline = Instant::now().checked_add(timeout);
        let command_line = format!("run_addr {}", test.address);
        let followed = self.follow(core, deadline, &mut |command, core, output| {
            Ok(match command {
                SemihostingCommand::ExitSuccess => Some(Stop::Exited(0)),
                SemihostingCommand::ExitError(_) => Some(Stop::Exited(1)),
                SemihostingCommand::GetCommandLine(request) => {
                    request
                        .write_command_line_to_target(core, &command_line)
                        .map_err(|error| Error::Run(error.to_string()))?;
                    None
                }
                other => {
                    output.answer(other, core)?;
                    None
                }
            })
        })?;
        // A test starts from a reset the probe makes, so it does not follow the core across one of its own.
        let stop = match followed {
            Followed::Stop(stop) => stop,
            Followed::CoreLost(error) => return Err(Error::Run(error)),
        };
        let name = test.name.clone();
        let panicked = match stop {
            Stop::Exited(0) => false,
            Stop::Exited(_) => true,
            Stop::Deadline => {
                return Ok(TestResult::Failed {
                    name,
                    reason: format!("timed out after {timeout:?}"),
                });
            }
            Stop::Halted(reason) => {
                return Ok(TestResult::Failed {
                    name,
                    reason: format!("the core halted: {reason}"),
                });
            }
            Stop::Matched => {
                return Ok(TestResult::Failed {
                    name,
                    reason: "stopped on a matching line".to_owned(),
                });
            }
        };
        Ok(if panicked == test.should_panic {
            TestResult::Passed { name }
        } else if panicked {
            TestResult::Failed {
                name,
                reason: "panicked".to_owned(),
            }
        } else {
            TestResult::Failed {
                name,
                reason: "should have panicked and did not".to_owned(),
            }
        })
    }

    /// Reads what the board wrote since the last poll. Returns what ended the follow, if anything: a stop, or a
    /// probe transfer that failed and lost the core. Also returns whether there was anything to read.
    fn poll(&mut self, core: &mut Core<'_>) -> Result<(Option<Followed>, bool), Error> {
        if self.rtt.is_none() {
            let target = RttTarget::from(&*core);
            self.rtt = Rtt::attach_region(core, &target, &self.region).ok();
            if self.rtt.is_some() {
                self.decoder = self.table.map(Table::new_stream_decoder);
            }
        }
        let Some(rtt) = &mut self.rtt else {
            return Ok((None, false));
        };
        let mut buf = [0u8; 4096];
        let mut busy = false;
        let mut stop = None;
        for channel in rtt.up_channels() {
            let n = match channel.read(core, &mut buf) {
                Ok(n) => n,
                Err(error) if board_reset(&error) => {
                    return Ok((self.after_reset()?.map(Followed::Stop), true));
                }
                Err(error) if transport_lost(&error) => {
                    return Ok((Some(Followed::CoreLost(error.to_string())), true));
                }
                Err(error) => return Err(Error::Run(error.to_string())),
            };
            let bytes = buf.get(..n).unwrap_or(&[]);
            if bytes.is_empty() {
                continue;
            }
            busy = true;
            let found = match (&mut self.decoder, channel.number()) {
                (Some(decoder), 0) => {
                    decoder.received(bytes);
                    let mut found = None;
                    loop {
                        match decoder.decode() {
                            Ok(frame) => {
                                let text = defmt_line(&frame, &self.locations);
                                if let Some(stop) = self.monitor.line(&text)? {
                                    found = Some(stop);
                                }
                            }
                            Err(defmt_decoder::DecodeError::UnexpectedEof) => break,
                            Err(defmt_decoder::DecodeError::Malformed) => {
                                self.monitor.line("(malformed defmt frame)")?;
                                // Only the rzcobs encoding finds the next frame after a bad one. On the raw
                                // encoding the decoder would report the same bytes for ever.
                                if !self
                                    .table
                                    .is_some_and(|table| table.encoding().can_recover())
                                {
                                    self.monitor.line(
                                        "(the defmt stream cannot recover, the rest of channel 0 is dropped)",
                                    )?;
                                    self.decoder = None;
                                    break;
                                }
                            }
                        }
                    }
                    found
                }
                // Channel 0 carries defmt frames, and once the decoder is gone they are not text either.
                (None, 0) if self.table.is_some() => None,
                _ => self.monitor.text(bytes)?,
            };
            if found.is_some() {
                stop = found;
            }
        }
        Ok((stop.map(Followed::Stop), busy))
    }

    /// Forgets the RTT state of the boot before a reset, so the next poll attaches to the new boot's control
    /// block with a new decoder, and says the board reset. One reset can show both as a moved read pointer and
    /// as a lost core, and the line comes only from the first.
    fn after_reset(&mut self) -> Result<Option<Stop>, Error> {
        self.decoder = None;
        if self.rtt.take().is_none() {
            return Ok(None);
        }
        self.monitor.line("(the board reset)")
    }

    /// Handles a halted core. A semihosting command goes to `on_command`, and the core runs on unless it says
    /// to stop. Any other halt stops the run. A status that cannot be read is a lost core.
    fn halt(
        &mut self,
        core: &mut Core<'_>,
        on_command: &mut OnCommand<'_>,
    ) -> Result<Option<Followed>, Error> {
        let status = match core.status() {
            Ok(status) => status,
            Err(error) => return Ok(Some(Followed::CoreLost(error.to_string()))),
        };
        let CoreStatus::Halted(reason) = status else {
            return Ok(None);
        };
        let HaltReason::Breakpoint(BreakpointCause::Semihosting(command)) = reason else {
            return Ok(Some(Followed::Stop(Stop::Halted(format!("{reason:?}")))));
        };
        if let Some(stop) = on_command(command, core, self)? {
            return Ok(Some(Followed::Stop(stop)));
        }
        core.run().map_err(|error| Error::Run(error.to_string()))?;
        Ok(None)
    }

    /// Answers a semihosting request the firmware makes on the way, such as a console write, and ignores
    /// the ones that need no answer.
    fn answer(&mut self, command: SemihostingCommand, core: &mut Core<'_>) -> Result<(), Error> {
        if let SemihostingCommand::WriteConsole(request) = command {
            let text = request
                .read(core)
                .map_err(|error| Error::Run(error.to_string()))?;
            self.monitor.text(text.as_bytes())?;
        }
        Ok(())
    }
}

/// One defmt frame as a line: the level, the message and the source location when the ELF has it.
fn defmt_line(frame: &defmt_decoder::Frame<'_>, locations: &Locations) -> String {
    let location = locations.get(&frame.index());
    match (frame.level(), location) {
        (Some(level), Some(at)) => format!(
            "{:<5} {} ({}:{})",
            level.as_str(),
            frame.display_message(),
            at.file.display(),
            at.line
        ),
        (Some(level), None) => format!("{:<5} {}", level.as_str(), frame.display_message()),
        (None, _) => frame.display_message().to_string(),
    }
}

/// Whether an RTT read failed because the firmware set up its RTT control block again, which it does only when
/// it boots. probe-rs reports it as a read pointer it did not write, or as a block it cannot read while the
/// firmware fills it in.
fn board_reset(error: &probe_rs::rtt::Error) -> bool {
    matches!(
        error,
        probe_rs::rtt::Error::ReadPointerChanged | probe_rs::rtt::Error::ControlBlockCorrupted(_)
    )
}

/// Whether an RTT read failed in the probe's own transfer. A reset can fail the transfer that lands during it, as
/// the NUCLEO-H7S3L8's ST-LINK does, so the run treats the core as lost and attaches again (M-18).
fn transport_lost(error: &probe_rs::rtt::Error) -> bool {
    matches!(error, probe_rs::rtt::Error::Probe(_))
}

/// How a follow ended: with a stop, or with a core whose state could no longer be read.
enum Followed {
    Stop(Stop),
    /// probe-rs's error from reading the core's status.
    CoreLost(String),
}

/// A core lost again this soon after the last re-attach was not lost to a reset.
const CORE_LOST_AGAIN: Duration = Duration::from_secs(2);

/// How long a run waits for a board that a reset took off the USB bus (M-5).
const BOARD_RETURN: Duration = Duration::from_secs(10);

/// Turns a follow's error into a lost core when the board has gone from the USB bus, as a reset button takes it,
/// so the run waits for it to return. Any other error stays an error.
fn unless_gone(board: &Board, followed: Result<Followed, Error>) -> Result<Followed, Error> {
    match followed {
        Err(error) if !crate::board::is_attached(board) => {
            Ok(Followed::CoreLost(error.to_string()))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that hands out `input`, keeps what is written, and counts flushes.
    struct Wire {
        input: std::io::Cursor<Vec<u8>>,
        written: std::rc::Rc<std::cell::RefCell<(Vec<u8>, u32)>>,
    }

    impl Read for Wire {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let n = buf.len().min(3);
            self.written.borrow_mut().0.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            let mut written = self.written.borrow_mut();
            written.1 = written.1.saturating_add(1);
            Ok(())
        }
    }

    #[test]
    fn a_transport_reads_writes_and_flushes_through_to_its_stream() {
        let written = std::rc::Rc::default();
        let mut transport = Transport(Box::new(Wire {
            input: std::io::Cursor::new(b"ok\r\n".to_vec()),
            written: std::rc::Rc::clone(&written),
        }));
        let mut read = [0; 8];
        assert_eq!(transport.read(&mut read).unwrap(), 4);
        assert_eq!(&read[..4], b"ok\r\n");
        // The stream takes three bytes at a time, and the transport reports the short write as it is.
        assert_eq!(transport.write(b"reset").unwrap(), 3);
        transport.flush().unwrap();
        assert_eq!(*written.borrow(), (b"res".to_vec(), 1));
    }

    #[test]
    fn a_moved_read_pointer_or_a_block_being_set_up_is_a_reset_and_nothing_else_is() {
        use probe_rs::rtt::Error;
        assert!(board_reset(&Error::ReadPointerChanged));
        assert!(board_reset(&Error::ControlBlockCorrupted(
            "pointer".to_owned()
        )));
        assert!(!board_reset(&Error::ControlBlockNotFound));
        assert!(!board_reset(&Error::MissingChannel(1)));
    }

    #[test]
    fn only_a_failed_probe_transfer_loses_the_transport() {
        use probe_rs::rtt::Error;
        assert!(transport_lost(&Error::Probe(probe_rs::Error::Timeout)));
        assert!(!transport_lost(&Error::ReadPointerChanged));
        assert!(!transport_lost(&Error::ControlBlockNotFound));
        assert!(!transport_lost(&Error::MissingChannel(1)));
    }

    #[test]
    fn a_flash_mode_is_read_by_its_name_in_any_case() {
        assert_eq!("qio".parse(), Ok(FlashMode::Qio));
        assert_eq!("QOUT".parse(), Ok(FlashMode::Qout));
        assert_eq!("Dio".parse(), Ok(FlashMode::Dio));
        assert_eq!("dout".parse(), Ok(FlashMode::Dout));
        assert_eq!(
            "fast".parse::<FlashMode>(),
            Err("fast is not a flash mode: qio, qout, dio or dout".to_owned())
        );
    }

    #[test]
    fn an_esp_idf_image_boots_in_quad_unless_it_names_another_mode() {
        use espflash::flasher::FlashMode as Loader;
        let mode = |flash_mode| {
            idf_flash_mode(&Image {
                flash_mode,
                ..Image::default()
            })
        };
        assert_eq!(mode(None), Loader::Qio);
        assert_eq!(mode(Some(FlashMode::Qio)), Loader::Qio);
        assert_eq!(mode(Some(FlashMode::Qout)), Loader::Qout);
        assert_eq!(mode(Some(FlashMode::Dio)), Loader::Dio);
        assert_eq!(mode(Some(FlashMode::Dout)), Loader::Dout);
    }

    #[test]
    fn a_started_console_says_whether_its_image_was_flashed_or_reset() {
        let elf = Path::new("/t/release/examples/console");
        let mut flashed = Vec::new();
        started(elf, true, &mut flashed);
        assert_eq!(
            String::from_utf8(flashed).unwrap(),
            "mcu: flashed console\n"
        );
        let mut reset = Vec::new();
        started(elf, false, &mut reset);
        assert_eq!(
            String::from_utf8(reset).unwrap(),
            "mcu: console already runs, reset\n"
        );
    }

    #[test]
    fn a_board_runs_the_image_only_when_its_banner_and_the_elf_name_the_same_id() {
        let banner = |image: Option<&str>| Banner {
            name: "c5".to_owned(),
            boot: 3,
            protocol: Some(1),
            image: image.map(str::to_owned),
        };
        assert!(runs_image(&banner(Some("0a1b")), Some("0a1b")));
        assert!(!runs_image(&banner(Some("0a1b")), Some("0a1c")));
        assert!(!runs_image(&banner(Some("0a1b")), None));
        assert!(!runs_image(&banner(None), Some("0a1b")));
        assert!(!runs_image(&banner(None), None));
    }

    /// A console transport that answers with `script` and then reports that the console closed it.
    struct Script {
        input: std::io::Cursor<Vec<u8>>,
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn console_answering(script: &str) -> Client<Script> {
        Client::new(Script {
            input: std::io::Cursor::new(script.as_bytes().to_vec()),
        })
    }

    #[test]
    fn a_reset_through_the_console_needs_ok_and_then_the_banner() {
        let wait = Duration::from_secs(1);
        let banner = "c5> reset\r\nok\r\nESP-ROM:esp32c5\r\nc5 boot 3 protocol 1 image 0a1b\r\n";
        assert_eq!(
            reset_through(&mut console_answering(banner), wait, wait),
            Some(Banner {
                name: "c5".to_owned(),
                boot: 3,
                protocol: Some(1),
                image: Some("0a1b".to_owned()),
            })
        );
        let refused = "c5> reset\r\nerr busy\r\nc5 boot 3 protocol 1\r\n";
        assert_eq!(
            reset_through(&mut console_answering(refused), wait, wait),
            None
        );
        let silent = "c5> reset\r\nok\r\nESP-ROM:esp32c5\r\n";
        assert_eq!(
            reset_through(&mut console_answering(silent), wait, wait),
            None
        );
    }
}
