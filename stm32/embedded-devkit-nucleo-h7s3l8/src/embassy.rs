//! Embassy's interrupt executors, and the console's async transport (B-3, B-4).
//!
//! The application writes the executors into itself with [`interrupt_executors!`](crate::interrupt_executors),
//! because an interrupt executor's handler calls `InterruptExecutor::on_interrupt`, which is `unsafe`. The
//! executors run on the UART4 and UART5 interrupts, so an application with this feature cannot use those UARTs'
//! interrupts.

use embassy_executor::{InterruptExecutor, SendSpawner};
use embassy_stm32::interrupt::{self, InterruptExt, Priority};
use embassy_stm32::peripherals::{PD8, PD9, USART3};
use embassy_stm32::usart::{self, BufferedUart, BufferedUartRx, BufferedUartTx, ConfigError};
use embassy_stm32::{Peri, bind_interrupts};

#[doc(hidden)]
pub use embassy_executor::InterruptExecutor as __InterruptExecutor;
#[doc(hidden)]
pub use embassy_executor::SendSpawner as __SendSpawner;
#[doc(hidden)]
pub use embassy_stm32::interrupt as __interrupt;

/// The console's receive and transmit buffers. The application keeps them in a `static`, so they outlive the
/// console.
pub struct ConsoleBuffers {
    rx: [u8; 256],
    tx: [u8; 256],
}

impl ConsoleBuffers {
    /// Empty buffers.
    pub const fn new() -> Self {
        Self {
            rx: [0; 256],
            tx: [0; 256],
        }
    }
}

impl Default for ConsoleBuffers {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the two executors that [`interrupt_executors!`](crate::interrupt_executors) writes, and returns
/// their spawners as `(high, low)`. A task on `high` preempts every task on `low`, and both preempt `main`.
pub fn start(
    high: &'static InterruptExecutor,
    low: &'static InterruptExecutor,
) -> (SendSpawner, SendSpawner) {
    // The interrupts here are the ones whose handlers interrupt_executors! writes. A lower number is a higher
    // priority.
    interrupt::UART4.set_priority(Priority::P6);
    interrupt::UART5.set_priority(Priority::P7);
    (high.start(interrupt::UART4), low.start(interrupt::UART5))
}

/// The console's async transport: USART3 at 115200 bps on PD9 (RX) and PD8 (TX), driven by its interrupt.
///
/// # Errors
///
/// Returns embassy-stm32's `ConfigError` when USART3's clock cannot make 115200 bps.
pub fn console(
    usart: Peri<'static, USART3>,
    rx: Peri<'static, PD9>,
    tx: Peri<'static, PD8>,
    buffers: &'static mut ConsoleBuffers,
) -> Result<(BufferedUartRx<'static>, BufferedUartTx<'static>), ConfigError> {
    let uart = BufferedUart::new(
        usart,
        rx,
        tx,
        &mut buffers.tx,
        &mut buffers.rx,
        Irqs,
        usart::Config::default(),
    )?;
    let (tx, rx) = uart.split();
    Ok((rx, tx))
}

bind_interrupts!(struct Irqs {
    USART3 => usart::BufferedInterruptHandler<USART3>;
});

/// Writes the board's two interrupt executors into the application, and a `start_executors()` function that
/// starts them through [`embassy::start`](crate::embassy::start) and returns their spawners as `(high, low)`.
/// Call it once, at the application's top level.
#[macro_export]
macro_rules! interrupt_executors {
    () => {
        mod __board_executors {
            use $crate::embassy::__interrupt as interrupt;

            pub static HIGH: $crate::embassy::__InterruptExecutor =
                $crate::embassy::__InterruptExecutor::new();
            pub static LOW: $crate::embassy::__InterruptExecutor =
                $crate::embassy::__InterruptExecutor::new();

            #[interrupt]
            fn UART4() {
                // SAFETY: UART4 is the interrupt that embassy::start starts HIGH on, and start is what enables it, so
                // this runs only for a started executor. Another handler for UART4, from bind_interrupts! or
                // #[interrupt], would be a second definition of the same symbol, which the linker refuses.
                unsafe { HIGH.on_interrupt() }
            }

            #[interrupt]
            fn UART5() {
                // SAFETY: as for UART4, with LOW.
                unsafe { LOW.on_interrupt() }
            }
        }

        /// Starts the board's interrupt executors and returns their spawners as `(high, low)`.
        fn start_executors() -> (
            $crate::embassy::__SendSpawner,
            $crate::embassy::__SendSpawner,
        ) {
            $crate::embassy::start(&__board_executors::HIGH, &__board_executors::LOW)
        }
    };
}
