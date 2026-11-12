//! Which RISC-V instruction set extensions the ESP32-S31's HP core runs. A test for an extension the core runs
//! checks results that no base instruction could give.
//!
//! A test for an extension the core does not run expects the trap. esp-hal's exception handler panics on an
//! illegal instruction, with the instruction in `mtval`. Each of those tests runs one instruction, so a core with
//! part of an extension fails the test for the part it has.

#![no_std]
#![no_main]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use embedded_devkit_esp32_s31_function_coreboard_1 as _;

/// Runs `insn rd, rs1, rs2` from extension `ext` and returns `rd`.
macro_rules! rr {
    ($ext:literal, $insn:literal, $rs1:expr, $rs2:expr) => {{
        let rd: u32;
        // SAFETY: the instruction reads and writes only its operand registers.
        #[expect(unsafe_code)]
        unsafe {
            core::arch::asm!(
                ".option push",
                concat!(".option arch, +", $ext),
                concat!($insn, " {rd}, {rs1}, {rs2}"),
                ".option pop",
                rs1 = in(reg) $rs1,
                rs2 = in(reg) $rs2,
                rd = lateout(reg) rd,
                options(pure, nomem, nostack),
            );
        }
        rd
    }};
}

/// Runs `insn rd, rs1` from extension `ext` and returns `rd`.
macro_rules! r1 {
    ($ext:literal, $insn:literal, $rs1:expr) => {{
        let rd: u32;
        // SAFETY: the instruction reads and writes only its operand registers.
        #[expect(unsafe_code)]
        unsafe {
            core::arch::asm!(
                ".option push",
                concat!(".option arch, +", $ext),
                concat!($insn, " {rd}, {rs1}"),
                ".option pop",
                rs1 = in(reg) $rs1,
                rd = lateout(reg) rd,
                options(pure, nomem, nostack),
            );
        }
        rd
    }};
}

/// Runs `insn rd, rs1, imm` from extension `ext` and returns `rd`.
macro_rules! ri {
    ($ext:literal, $insn:literal, $rs1:expr, $imm:literal) => {{
        let rd: u32;
        // SAFETY: the instruction reads and writes only its operand registers.
        #[expect(unsafe_code)]
        unsafe {
            core::arch::asm!(
                ".option push",
                concat!(".option arch, +", $ext),
                concat!($insn, " {rd}, {rs1}, ", $imm),
                ".option pop",
                rs1 = in(reg) $rs1,
                rd = lateout(reg) rd,
                options(pure, nomem, nostack),
            );
        }
        rd
    }};
}

/// Reads the CSR `csr` and returns its value.
macro_rules! csr_read {
    ($csr:literal) => {{
        let value: u32;
        // SAFETY: reading the CSR has no side effect.
        #[expect(unsafe_code)]
        unsafe {
            core::arch::asm!(concat!("csrr {v}, ", $csr), v = lateout(reg) value, options(nomem, nostack));
        }
        value
    }};
}

/// Runs `insn` from extension `ext`, for a test that expects the core to trap. The instruction may write a0 to a2,
/// ft0, ft1 and the 256-byte block whose address is in a3.
macro_rules! traps {
    ($ext:literal, $insn:literal) => {{
        #[repr(C, align(256))]
        struct Scratch([u8; 256]);
        let mut scratch = Scratch([0; 256]);
        // SAFETY: a core that ran the instruction would write only the registers named here and `scratch`.
        #[expect(unsafe_code)]
        unsafe {
            core::arch::asm!(
                ".option push",
                concat!(".option arch, +", $ext),
                $insn,
                ".option pop",
                in("a3") scratch.0.as_mut_ptr(),
                out("a0") _,
                out("a1") _,
                out("a2") _,
                out("ft0") _,
                out("ft1") _,
                options(nostack),
            );
        }
        defmt::info!("ran without a trap: {=str}", $insn);
    }};
}

#[repr(C, align(64))]
struct JumpTable([Option<extern "C" fn() -> u32>; 33]);

extern "C" fn table_target() -> u32 {
    0x5A5A_0001
}

/// Zcmt's jump table: entry 0 for `cm.jt`, entry 32 for `cm.jalt`, both `table_target`.
static JUMP_TABLE: JumpTable = {
    let mut entries: [Option<extern "C" fn() -> u32>; 33] = [None; 33];
    entries[0] = Some(table_target);
    entries[32] = Some(table_target);
    JumpTable(entries)
};

#[cfg(test)]
#[embedded_test::tests]
mod tests {
    use super::JUMP_TABLE;
    use core::arch::asm;
    use core::ptr::addr_of;

    #[test]
    fn zba_address_generation() {
        defmt::assert_eq!(rr!("zba", "sh1add", 0x10_u32, 3_u32), 0x23);
        defmt::assert_eq!(rr!("zba", "sh2add", 0x10_u32, 3_u32), 0x43);
        defmt::assert_eq!(rr!("zba", "sh3add", 0x10_u32, 3_u32), 0x83);
    }

    #[test]
    fn zbb_basic_bit_manipulation() {
        let (a, b) = (0x0F0F_00F0_u32, 0x00FF_0F0F_u32);
        defmt::assert_eq!(rr!("zbb", "andn", a, b), 0x0F00_00F0);
        defmt::assert_eq!(rr!("zbb", "orn", a, b), 0xFF0F_F0F0);
        defmt::assert_eq!(rr!("zbb", "xnor", a, b), 0xF00F_F000);
        defmt::assert_eq!(r1!("zbb", "clz", a), 4);
        defmt::assert_eq!(r1!("zbb", "ctz", a), 4);
        defmt::assert_eq!(r1!("zbb", "cpop", a), 12);
        let minus_five = (-5_i32) as u32;
        defmt::assert_eq!(rr!("zbb", "max", minus_five, 3_u32), 3);
        defmt::assert_eq!(rr!("zbb", "min", minus_five, 3_u32), minus_five);
        defmt::assert_eq!(rr!("zbb", "maxu", minus_five, 3_u32), minus_five);
        defmt::assert_eq!(rr!("zbb", "minu", minus_five, 3_u32), 3);
        defmt::assert_eq!(r1!("zbb", "sext.b", 0x80_u32), 0xFFFF_FF80);
        defmt::assert_eq!(r1!("zbb", "sext.h", 0x8000_u32), 0xFFFF_8000);
        defmt::assert_eq!(r1!("zbb", "zext.h", 0xFFFF_1234_u32), 0x1234);
        defmt::assert_eq!(rr!("zbb", "rol", 0x8000_0001_u32, 1_u32), 0x0000_0003);
        defmt::assert_eq!(rr!("zbb", "ror", 0x8000_0001_u32, 1_u32), 0xC000_0000);
        defmt::assert_eq!(ri!("zbb", "rori", 0x1234_5678_u32, "4"), 0x8123_4567);
        defmt::assert_eq!(r1!("zbb", "orc.b", 0x0001_0080_u32), 0x00FF_00FF);
        defmt::assert_eq!(r1!("zbb", "rev8", 0x1234_5678_u32), 0x7856_3412);
    }

    #[test]
    fn zbc_carry_less_multiplication() {
        let a = 0x8000_0001_u32;
        defmt::assert_eq!(rr!("zbc", "clmul", a, a), 0x0000_0001);
        defmt::assert_eq!(rr!("zbc", "clmulh", a, a), 0x4000_0000);
        defmt::assert_eq!(rr!("zbc", "clmulr", a, a), 0x8000_0000);
    }

    #[test]
    fn zbs_single_bit_instructions() {
        defmt::assert_eq!(rr!("zbs", "bclr", 0xFF_u32, 3_u32), 0xF7);
        defmt::assert_eq!(ri!("zbs", "bclri", 0xFF_u32, "3"), 0xF7);
        defmt::assert_eq!(rr!("zbs", "bext", 0b1000_u32, 3_u32), 1);
        defmt::assert_eq!(ri!("zbs", "bexti", 0b1000_u32, "3"), 1);
        defmt::assert_eq!(rr!("zbs", "binv", 0_u32, 5_u32), 0x20);
        defmt::assert_eq!(ri!("zbs", "binvi", 0_u32, "5"), 0x20);
        defmt::assert_eq!(rr!("zbs", "bset", 0_u32, 31_u32), 0x8000_0000);
        defmt::assert_eq!(ri!("zbs", "bseti", 0_u32, "31"), 0x8000_0000);
    }

    #[test]
    fn zcb_compressed_basic_instructions() {
        let mut bytes = [0x80_u8, 0x81, 0x00, 0x80];
        let (lbu, lh, lhu): (u32, u32, u32);
        let (zext_b, sext_b, zext_h, sext_h, not, mul): (u32, u32, u32, u32, u32, u32);
        // SAFETY: the loads and stores reach only `bytes`, whose address is in a0. The other instructions use
        // only the named registers. Zcb's operands must be in x8 to x15, so the registers are named.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                ".option push",
                ".option arch, +zcb",
                "c.lbu a1, 1(a0)",
                "c.lh a2, 2(a0)",
                "c.lhu a3, 2(a0)",
                ".option pop",
                in("a0") bytes.as_mut_ptr(),
                out("a1") lbu,
                out("a2") lh,
                out("a3") lhu,
                options(readonly, nostack),
            );
            asm!(
                ".option push",
                ".option arch, +zcb, +zbb, +m",
                "li a0, 0x1FF",
                "c.zext.b a0",
                "li a1, 0x80",
                "c.sext.b a1",
                "li a2, 0x12348000",
                "c.zext.h a2",
                "li a3, 0x8000",
                "c.sext.h a3",
                "li a4, 0x0F0F0F0F",
                "c.not a4",
                ".option pop",
                out("a0") zext_b,
                out("a1") sext_b,
                out("a2") zext_h,
                out("a3") sext_h,
                out("a4") not,
                options(nomem, nostack),
            );
            asm!(
                ".option push",
                ".option arch, +zcb, +m",
                "li a1, 6",
                "li a2, 7",
                "c.mul a1, a2",
                "li a3, 0x5A",
                "c.sb a3, 0(a0)",
                "li a4, 0x1234",
                "c.sh a4, 2(a0)",
                ".option pop",
                in("a0") bytes.as_mut_ptr(),
                out("a1") mul,
                out("a2") _,
                out("a3") _,
                out("a4") _,
                options(nostack),
            );
        }
        defmt::assert_eq!((lbu, lh, lhu), (0x81, 0xFFFF_8000, 0x8000));
        defmt::assert_eq!(
            (zext_b, sext_b, zext_h, sext_h),
            (0xFF, 0xFFFF_FF80, 0x8000, 0xFFFF_8000)
        );
        defmt::assert_eq!((not, mul), (0xF0F0_F0F0, 42));
        defmt::assert_eq!(bytes, [0x5A, 0x81, 0x34, 0x12]);
    }

    /// The RISC-V Zc spec stores ra at the highest address of the frame `cm.push` makes. The S31 stores the list
    /// the other way round, with the last register at the highest address.
    #[test]
    fn zcmp_push_pop_and_moves() {
        let (at_12, at_8, s0_before, popretz_a0, moved_a0, moved_a1): (
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
        );
        // SAFETY: `cm.push` lowers sp by 16 and `cm.pop` raises it by 16 again, so sp is the same at the end. The
        // block saves and restores ra and s0 itself, and reads only the 16 bytes it pushed.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                ".option push",
                ".option arch, +zcmp",
                "li ra, 0x1234",
                "mv {s0_before}, s0",
                "cm.push {{ra, s0}}, -16",
                "lw {at_12}, 12(sp)",
                "lw {at_8}, 8(sp)",
                "cm.pop {{ra, s0}}, 16",
                ".option pop",
                s0_before = out(reg) s0_before,
                at_12 = out(reg) at_12,
                at_8 = out(reg) at_8,
                out("ra") _,
            );
            asm!(
                ".option push",
                ".option arch, +zcmp",
                "la ra, 1f",
                "li a0, 9",
                "cm.push {{ra}}, -16",
                "cm.popretz {{ra}}, 16",
                "1:",
                ".option pop",
                out("a0") popretz_a0,
                out("ra") _,
            );
            asm!(
                ".option push",
                ".option arch, +zcmp",
                "li a0, 0x11",
                "li a1, 0x22",
                "cm.mvsa01 s2, s3",
                "li a0, 0",
                "li a1, 0",
                "cm.mva01s s3, s2",
                ".option pop",
                out("a0") moved_a0,
                out("a1") moved_a1,
                out("s2") _,
                out("s3") _,
                options(nomem, nostack),
            );
        }
        defmt::assert_eq!((at_12, at_8), (s0_before, 0x1234));
        defmt::assert_eq!(popretz_a0, 0);
        defmt::assert_eq!((moved_a0, moved_a1), (0x22, 0x11));
    }

    #[test]
    fn zcmt_table_jumps() {
        let (via_jalt, via_jt, jvt_read): (u32, u32, u32);
        // SAFETY: the block sets the jump vector table CSR (0x017) to `JUMP_TABLE` and restores it before it
        // ends. Both table entries call `table_target`, an `extern "C"` function, which returns to ra. The values
        // kept across the calls are in callee-saved registers, which the calls preserve.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                ".option push",
                ".option arch, +zcmt",
                "csrr s4, 0x017",
                "csrw 0x017, {table}",
                "csrr s6, 0x017",
                "cm.jalt 32",
                "mv s5, a0",
                "la ra, 1f",
                "cm.jt 0",
                "1:",
                "csrw 0x017, s4",
                ".option pop",
                table = in(reg) addr_of!(JUMP_TABLE),
                out("s4") _,
                out("s5") via_jalt,
                out("s6") jvt_read,
                out("a0") via_jt,
                clobber_abi("C"),
            );
        }
        defmt::assert_eq!(jvt_read, addr_of!(JUMP_TABLE) as u32);
        defmt::assert_eq!((via_jalt, via_jt), (0x5A5A_0001, 0x5A5A_0001));
    }

    #[test]
    fn zicntr_cycle_and_instructions_retired() {
        let first = csr_read!("0xc00");
        let second = csr_read!("0xc00");
        let first_retired = csr_read!("0xc02");
        let second_retired = csr_read!("0xc02");
        defmt::assert!(second > first);
        defmt::assert!(second_retired > first_retired);
    }

    #[test]
    fn zicntr_time() {
        let first = csr_read!("0xc01");
        let second = csr_read!("0xc01");
        defmt::assert!(second >= first);
    }

    #[test]
    fn xesploop_loop_registers() {
        let count: u32;
        // SAFETY: the block turns on the hardware loop state and writes loop 0's count register. No loop is
        // set up, so the count has no effect.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                "csrwi 0x7f1, 1",
                "csrw 0x7c8, {seven}",
                "csrr {count}, 0x7c8",
                seven = in(reg) 7_u32,
                count = lateout(reg) count,
                options(nomem, nostack),
            );
        }
        defmt::assert_eq!(count, 7);
    }

    /// `esp.lp.setupi 0, 4, 1f` and `esp.lp.setup 0, t6, 1f`, written as words because upstream LLVM has no
    /// mnemonics for them. The label is the last instruction of the loop body, 8 bytes after the setup.
    #[test]
    fn xesploop_hardware_loops() {
        let (fixed_a0, fixed_a1, counted_a0, counted_a1): (u32, u32, u32, u32);
        // SAFETY: each loop runs two `addi` instructions on the named registers. The loops end after their counts,
        // and the block turns the hardware loop state on first.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                ".option push",
                ".option norvc",
                "csrwi 0x7f1, 1",
                "li a0, 0",
                "li a1, 0",
                ".word 0x0042502b",
                "addi a0, a0, 1",
                "addi a1, a1, 2",
                ".option pop",
                out("a0") fixed_a0,
                out("a1") fixed_a1,
                options(nomem, nostack),
            );
            asm!(
                ".option push",
                ".option norvc",
                "csrwi 0x7f1, 1",
                "li a0, 0",
                "li a1, 0",
                ".word 0x004fc02b",
                "addi a0, a0, 1",
                "addi a1, a1, 2",
                ".option pop",
                in("t6") 3_u32,
                out("a0") counted_a0,
                out("a1") counted_a1,
                options(nomem, nostack),
            );
        }
        defmt::assert_eq!((fixed_a0, fixed_a1), (4, 8));
        defmt::assert_eq!((counted_a0, counted_a1), (3, 6));
    }

    /// ESP-IDF turns PIE on by writing 1, its initial state, to the PIE state CSR (0x7F2). On the board under test the CSR
    /// exists and reads back 0, its off state.
    #[test]
    fn xespv_state_register_reads_back_off() {
        let state: u32;
        // SAFETY: the block writes the PIE state CSR and reads it back.
        #[expect(unsafe_code)]
        unsafe {
            asm!(
                "csrwi 0x7f2, 1",
                "csrr {state}, 0x7f2",
                state = lateout(reg) state,
                options(nomem, nostack),
            );
        }
        defmt::assert_eq!(state, 0);
    }

    /// `esp.zero.q q0` in PIE 2.2's encoding, which Espressif's LLVM gives the S31, written as a word because
    /// upstream LLVM has no mnemonic for it.
    #[test]
    #[should_panic]
    fn xespv_traps_on_zero_q_in_the_2_2_encoding() {
        traps!("zbb", "csrwi 0x7f2, 1\n.word 0x0000101b");
    }

    /// `esp.zero.q q1` in PIE 2.1's encoding, which Espressif's LLVM gives the ESP32-P4 before v3.
    #[test]
    #[should_panic]
    fn xespv_traps_on_zero_q_in_the_2_1_encoding() {
        traps!("zbb", "csrwi 0x7f2, 1\n.word 0x004000db");
    }

    #[test]
    #[should_panic]
    fn v_traps_on_vsetvli() {
        traps!(
            "zve32x",
            "li a0, 0x200\ncsrs mstatus, a0\nvsetvli a1, a2, e32, m1, ta, ma"
        );
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_fmv_h_x() {
        traps!("zfhmin", "fmv.h.x ft0, a1");
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_fmv_x_h() {
        traps!("zfhmin", "fmv.x.h a0, ft0");
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_fcvt_s_h() {
        traps!("zfhmin", "fcvt.s.h ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_fcvt_h_s() {
        traps!("zfhmin", "fcvt.h.s ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_flh() {
        traps!("zfhmin", "flh ft0, 0(a3)");
    }

    #[test]
    #[should_panic]
    fn zfhmin_traps_on_fsh() {
        traps!("zfhmin", "fsh ft0, 0(a3)");
    }

    #[test]
    #[should_panic]
    fn zfh_traps_on_fadd_h() {
        traps!("zfh", "fadd.h ft0, ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zfa_traps_on_fli_s() {
        traps!("zfa", "fli.s ft0, 0.5");
    }

    #[test]
    #[should_panic]
    fn zfa_traps_on_fminm_s() {
        traps!("zfa", "fminm.s ft0, ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zfa_traps_on_fround_s() {
        traps!("zfa", "fround.s ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zfa_traps_on_fleq_s() {
        traps!("zfa", "fleq.s a0, ft0, ft1");
    }

    #[test]
    #[should_panic]
    fn zicond_traps_on_czero_eqz() {
        traps!("zicond", "czero.eqz a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zicond_traps_on_czero_nez() {
        traps!("zicond", "czero.nez a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zbkb_traps_on_pack() {
        traps!("zbkb", "pack a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zbkb_traps_on_packh() {
        traps!("zbkb", "packh a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zbkb_traps_on_brev8() {
        traps!("zbkb", "brev8 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zbkb_traps_on_zip() {
        traps!("zbkb", "zip a0, a1");
    }

    #[test]
    #[should_panic]
    fn zbkb_traps_on_unzip() {
        traps!("zbkb", "unzip a0, a1");
    }

    #[test]
    #[should_panic]
    fn zbkx_traps_on_xperm4() {
        traps!("zbkx", "xperm4 a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zbkx_traps_on_xperm8() {
        traps!("zbkx", "xperm8 a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha256sum0() {
        traps!("zknh", "sha256sum0 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha256sum1() {
        traps!("zknh", "sha256sum1 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha256sig0() {
        traps!("zknh", "sha256sig0 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha256sig1() {
        traps!("zknh", "sha256sig1 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sig0h() {
        traps!("zknh", "sha512sig0h a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sig0l() {
        traps!("zknh", "sha512sig0l a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sig1h() {
        traps!("zknh", "sha512sig1h a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sig1l() {
        traps!("zknh", "sha512sig1l a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sum0r() {
        traps!("zknh", "sha512sum0r a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zknh_traps_on_sha512sum1r() {
        traps!("zknh", "sha512sum1r a0, a1, a2");
    }

    #[test]
    #[should_panic]
    fn zkne_traps_on_aes32esi() {
        traps!("zkne", "aes32esi a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zkne_traps_on_aes32esmi() {
        traps!("zkne", "aes32esmi a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zknd_traps_on_aes32dsi() {
        traps!("zknd", "aes32dsi a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zknd_traps_on_aes32dsmi() {
        traps!("zknd", "aes32dsmi a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zksh_traps_on_sm3p0() {
        traps!("zksh", "sm3p0 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zksh_traps_on_sm3p1() {
        traps!("zksh", "sm3p1 a0, a1");
    }

    #[test]
    #[should_panic]
    fn zksed_traps_on_sm4ed() {
        traps!("zksed", "sm4ed a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zksed_traps_on_sm4ks() {
        traps!("zksed", "sm4ks a0, a1, a2, 0");
    }

    #[test]
    #[should_panic]
    fn zkr_traps_on_the_seed_csr() {
        traps!("zkr", "csrrw a0, seed, zero");
    }

    #[test]
    #[should_panic]
    fn zacas_traps_on_amocas_w() {
        traps!("zacas", "amocas.w a0, a1, (a3)");
    }

    #[test]
    #[should_panic]
    fn zabha_traps_on_amoadd_b() {
        traps!("zabha", "amoadd.b a0, a1, (a3)");
    }

    #[test]
    #[should_panic]
    fn zabha_traps_on_amoadd_h() {
        traps!("zabha", "amoadd.h a0, a1, (a3)");
    }

    #[test]
    #[should_panic]
    fn zawrs_traps_on_wrs_nto() {
        traps!("zawrs", "wrs.nto");
    }

    #[test]
    #[should_panic]
    fn zicboz_traps_on_cbo_zero() {
        traps!("zicboz", "cbo.zero (a3)");
    }

    #[test]
    #[should_panic]
    fn zicbom_traps_on_cbo_clean() {
        traps!("zicbom", "cbo.clean (a3)");
    }

    #[test]
    #[should_panic]
    fn zimop_traps_on_mop_r_0() {
        traps!("zimop", "mop.r.0 a0, a1");
    }
}
