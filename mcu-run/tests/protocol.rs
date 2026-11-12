//! The host and the console speak one protocol version while both are built from this repo (K-9).

#[test]
fn the_host_speaks_the_console_s_protocol_version() {
    assert_eq!(mcu_run::PROTOCOL, mcu_console::PROTOCOL);
}
