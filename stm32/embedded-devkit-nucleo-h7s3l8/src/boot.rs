//! The start-up code: maps the external flash into memory, so that the program stored there can run (B-14).
//!
//! cortex-m-rt's `Reset` calls `__pre_init` before it initialises RAM and calls `main`. `__pre_init` lives in
//! internal flash, in `.boot`, because nothing in external flash can run until it returns. It therefore writes the
//! registers itself through stm32-metapac, whose accessors are `#[inline(always)]`, and calls nothing. It must not
//! read `.rodata` or touch a static, because those are in external flash and in RAM that is not initialised yet.
//!
//! The flash is the board's Macronix MX25UW25645G on XSPI2, through port N of the XSPI I/O manager. The start-up
//! code reads it in one-line SPI mode with `FAST READ4B` (`0x0C`): a 4-byte address and 8 dummy cycles. A chip
//! reset does not reset the flash, so the flash must be in SPI mode whenever the chip resets. Code that switches it
//! to an octal mode must return it to SPI mode first, or send the octal software reset here. probe-rs's
//! `mx25uw25645g_stm32h7s38-nucleo` algorithm leaves it in SPI mode.
//!
//! Only `Reset`, the start-up code and `HardFault` are in internal flash. An exception before `__pre_init`
//! returns, such as an NMI, vectors to a handler in external flash. That fetch faults, and the chip stops in
//! `HardFault`.
//!
//! XSPI2's kernel clock is HCLK, so the flash clock follows the bus clock: 8 MHz from the 64 MHz HSI at reset, and
//! 37.5 MHz once `init` has raised HCLK to 300 MHz. Both are well within the flash's 133 MHz for SPI.

use embassy_stm32::pac;
use pac::gpio::vals::{Moder, Ospeedr};
use pac::xspi::vals::{
    CcrAdmode, CcrAdsize, CcrDmode, CcrImode, CcrIsize, Csht, Cssel, Fmode, Mtyp,
};

/// The XSPI clock divides HCLK by this value plus one.
const PRESCALER: u8 = 7;
/// 2^(DEVSIZE + 1) bytes: 32 MB.
const DEVSIZE: u8 = 24;
const FAST_READ_4B: u32 = 0x0C;
const DUMMY_CYCLES: u8 = 8;
/// The XSPIM port 2 alternate function on port N.
const AF_XSPIM_P2: u8 = 9;

// SAFETY: `__pre_init` is the symbol cortex-m-rt's `Reset` calls, and `.boot` is the section `memory.x` places in
// internal flash. The function reads and writes only peripheral registers, through stm32-metapac, so it is sound to
// run before RAM is initialised and before the external flash is mapped.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot")]
extern "C" fn __pre_init() {
    pac::RCC.ahb4enr().modify(|w| w.set_gpionen(true));
    pac::RCC.ahb5enr().modify(|w| w.set_iomngren(true));
    // The supply configuration in CSR2 can be written once after a power-on. A later write that differs leaves
    // PWR's ACTVOSRDY low for good, and embassy-stm32's init waits on it for ever. This is the first write after a
    // power-on, so it sets the LDO configuration that init asks embassy-stm32 for, and enables XSPI port 2's I/O.
    pac::PWR.csr2().modify(|w| {
        w.set_sden(false);
        w.set_ldoen(true);
        w.set_bypass(false);
        w.set_en_xspim2(true);
    });

    // NCS, CLK, IO0 and IO1, one call each. A loop over an array could read the array from .rodata.
    xspi_pin(1);
    xspi_pin(6);
    xspi_pin(2);
    xspi_pin(3);

    pac::XSPIM.cr().modify(|w| {
        w.set_muxen(false);
        w.set_req2ack_time(1);
    });
    pac::RCC.ahb5enr().modify(|w| w.set_xspi2en(true));
    pac::RCC.ahb5rstr().modify(|w| w.set_xspi2rst(true));
    pac::RCC.ahb5rstr().modify(|w| w.set_xspi2rst(false));

    let xspi = pac::XSPI2;
    while xspi.sr().read().busy() {}
    xspi.dcr1().modify(|w| {
        w.set_devsize(DEVSIZE);
        w.set_mtyp(Mtyp::B_0X1);
        w.set_csht(Csht::B_0X1);
    });
    xspi.dcr2().modify(|w| w.set_prescaler(PRESCALER));
    while xspi.sr().read().busy() {}
    xspi.tcr().modify(|w| {
        w.set_dhqc(true);
        w.set_dcyc(DUMMY_CYCLES);
    });
    // CSSEL resets to NCS2. The flash's chip select is PN1, NCS1.
    xspi.cr().modify(|w| {
        w.set_cssel(Cssel::B_0X0);
        w.set_en(true);
    });

    xspi.ccr().write(|w| {
        w.set_imode(CcrImode::B_0X1);
        w.set_isize(CcrIsize::B_0X0);
        w.set_admode(CcrAdmode::B_0X1);
        w.set_adsize(CcrAdsize::B_0X3);
        w.set_dmode(CcrDmode::B_0X1);
    });
    xspi.ir().write(|w| w.set_instruction(FAST_READ_4B));
    xspi.cr().modify(|w| w.set_fmode(Fmode::B_0X3));
}

#[inline(always)]
fn xspi_pin(pin: usize) {
    pac::GPION
        .ospeedr()
        .modify(|w| w.set_ospeedr(pin, Ospeedr::VERY_HIGH_SPEED));
    pac::GPION
        .afr(pin / 8)
        .modify(|w| w.set_afr(pin % 8, AF_XSPIM_P2));
    pac::GPION
        .moder()
        .modify(|w| w.set_moder(pin, Moder::ALTERNATE));
}
