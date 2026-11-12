//! How long the ESP32-S31's ECC and SHA blocks take, beside the software that embedded-tls runs through
//! embassy-crypto's rustcrypto drivers. Each test checks the hardware's result against the software's, then logs
//! the cycles of each operation as `timing: <what> min <cycles> max <cycles> cycles, <µs> us`. The timings are
//! readings, not limits.

#![no_std]
#![no_main]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use crypto_timing as _;
use embassy_crypto_rustcrypto as _;
use esp_hal::rng::Trng;

/// embassy-crypto's random bytes, from esp-hal's `Trng`, which the test holds a `TrngSource` for.
struct TrngRng;

impl embassy_crypto::driver::Rng for TrngRng {
    fn fill_bytes(buf: &mut [u8]) {
        Trng::try_new().unwrap().read(buf);
    }
}

embassy_crypto::rng_impl!(TrngRng);

/// How many times each operation runs.
const RUNS: usize = 5;

/// The CPU's cycle counter.
fn cycles() -> u32 {
    let count: u32;
    // SAFETY: reading `mcycle` has no side effect.
    #[expect(unsafe_code)]
    unsafe {
        core::arch::asm!("csrr {}, mcycle", out(reg) count, options(nomem, nostack));
    }
    count
}

/// Runs `op` [`RUNS`] times and logs the fewest and the most cycles one run took, the fewest in µs, and the
/// fastest run's time by esp-hal's timer, which checks that `mcycle` counts at the CPU clock. `op`'s result goes
/// through `black_box`, so the compiler cannot drop the work.
fn time<R>(what: &str, mut op: impl FnMut() -> R) {
    let (mut min, mut max, mut timer_us) = (u32::MAX, 0, 0);
    for _ in 0..RUNS {
        let started = esp_hal::time::Instant::now();
        let start = cycles();
        core::hint::black_box(op());
        let taken = cycles().wrapping_sub(start);
        let elapsed = started.elapsed().as_micros();
        if taken < min {
            min = taken;
            timer_us = elapsed;
        }
        max = max.max(taken);
    }
    let tenths = u64::from(min) * 10 / u64::from(esp_hal::clock::cpu_clock().as_mhz());
    defmt::info!(
        "timing: {} min {} max {} cycles, {}.{} us, timer {} us",
        what,
        min,
        max,
        tenths / 10,
        tenths % 10,
        timer_us
    );
}

/// `bytes` in the ECC block's byte order, least significant first.
fn reversed<const N: usize>(mut bytes: [u8; N]) -> [u8; N] {
    bytes.reverse();
    bytes
}

/// Hardware point multiplication and the software's for one curve: `$curve` in esp-hal, `$module` in
/// embassy-crypto, `$n` bytes per coordinate.
macro_rules! point_multiplication {
    ($name:ident, $curve:ident, $module:ident, $n:literal) => {
        fn $name(ecc: &mut esp_hal::ecc::Ecc<'_, esp_hal::Blocking>) {
            use embassy_crypto::$module::{Point, SecretKey};
            use esp_hal::ecc::EllipticCurve;

            let k = SecretKey::generate().unwrap().to_scalar();
            let p = Point::mul_base(&SecretKey::generate().unwrap().to_scalar())
                .to_affine()
                .unwrap();
            let g = Point::generator().to_affine().unwrap();

            for (what, base) in [("k*G", &g), ("k*P", &p)] {
                let software = Point::from_affine(base)
                    .unwrap()
                    .mul(&k)
                    .to_affine()
                    .unwrap();
                let hardware =
                    hardware_mul::<$n>(ecc, EllipticCurve::$curve, k.as_bytes(), &base.x, &base.y);
                assert_eq!(hardware, (software.x, software.y), "{}", what);
            }

            let point = Point::from_affine(&p).unwrap();
            let k_times_p = point.mul(&k).to_affine().unwrap();
            let (k_le, x_le, y_le) = (reversed(k.to_bytes()), reversed(p.x), reversed(p.y));
            let mut verify_and_multiply = || {
                let result = ecc
                    .affine_point_verification_multiplication(
                        EllipticCurve::$curve,
                        &k_le,
                        &x_le,
                        &y_le,
                    )
                    .unwrap();
                assert!(result.success());
                let (mut x, mut y) = ([0u8; $n], [0u8; $n]);
                result.read_affine_point_result(&mut x, &mut y).unwrap();
                (reversed(x), reversed(y))
            };
            assert_eq!(
                verify_and_multiply(),
                (k_times_p.x, k_times_p.y),
                "verify-and-multiply"
            );

            time(concat!(stringify!($curve), " software k*P"), || {
                core::hint::black_box(&point).mul(core::hint::black_box(&k))
            });
            time(
                concat!(stringify!($curve), " hardware verify-and-multiply"),
                verify_and_multiply,
            );
            time(concat!(stringify!($curve), " hardware k*P"), || {
                hardware_mul::<$n>(ecc, EllipticCurve::$curve, k.as_bytes(), &p.x, &p.y)
            });
        }
    };
}

/// k·(x, y) on `curve` from the ECC block, with coordinates big-endian as embassy-crypto gives them.
fn hardware_mul<const N: usize>(
    ecc: &mut esp_hal::ecc::Ecc<'_, esp_hal::Blocking>,
    curve: esp_hal::ecc::EllipticCurve,
    k: &[u8; N],
    x: &[u8; N],
    y: &[u8; N],
) -> ([u8; N], [u8; N]) {
    let (k, mut x, mut y) = (reversed(*k), reversed(*x), reversed(*y));
    ecc.affine_point_multiplication(curve, &k, &x, &y)
        .unwrap()
        .read_affine_point_result(&mut x, &mut y)
        .unwrap();
    (reversed(x), reversed(y))
}

point_multiplication!(p256_point_multiplication, P256, p256, 32);
point_multiplication!(p384_point_multiplication, P384, p384, 48);

#[path = "common/wycheproof_rsa.rs"]
mod wycheproof_rsa;

/// An RSA verify's exponentiation, s^65537 mod n, in the RSA block and in the rsa crate's PKCS#1 v1.5 verify, for
/// one key size: `$op` in esp-hal, `$uint` in crypto-bigint, `$wide` limbs for the block's `r`.
macro_rules! rsa_verify {
    ($name:ident, $bits:literal, $op:ident, $uint:ident, $wide:literal, $modulus:expr, $signature:expr) => {
        fn $name(rsa: &mut esp_hal::rsa::Rsa<'_, esp_hal::Blocking>) {
            use crypto_bigint::modular::{FixedMontyForm, FixedMontyParams};
            use crypto_bigint::{Odd, Uint, $uint};
            use esp_hal::rsa::{RsaModularExponentiation, operand_sizes::$op};
            use rsa::{BigUint, Pkcs1v15Sign, RsaPublicKey};
            use sha2::{Digest, Sha256};

            let digest = Sha256::digest(wycheproof_rsa::MESSAGE);
            let e = $uint::from_u32(65537);

            // What the RSA block needs for each new key: R² mod n, where R is 2^$bits, and -n⁻¹ mod 2³².
            let setup = |modulus: &[u8; $bits / 8]| {
                let n = $uint::from_be_slice(modulus);
                let mut wide = [0u32; $wide];
                wide[$wide - 1] = 1;
                let r: $uint = Uint::<$wide>::from_words(wide)
                    .wrapping_rem_vartime(&n.resize())
                    .resize();
                let m_inv = n
                    .invert_mod2k(32)
                    .expect_ref("an RSA modulus is odd")
                    .as_words()[0];
                (n, r, m_inv.wrapping_neg())
            };
            let (n, r, m_prime) = setup(&$modulus);

            let exponentiate = |rsa: &mut esp_hal::rsa::Rsa<'_, esp_hal::Blocking>,
                                base: &$uint| {
                let mut out = [0u32; $uint::LIMBS];
                let mut modexp = RsaModularExponentiation::<$op, _>::new(
                    rsa,
                    e.as_words(),
                    n.as_words(),
                    m_prime,
                );
                modexp.start_exponentiation(base.as_words(), r.as_words());
                modexp.read_results(&mut out);
                $uint::from_words(out)
            };

            // A valid signature decrypts to EMSA-PKCS1-v1_5: 00 01 FF..FF 00, SHA-256's DigestInfo, the digest.
            let encode = |digest: &[u8]| {
                let mut encoded = [0xffu8; $bits / 8];
                let prefix = hex_literal::hex!("3031300d060960864801650304020105000420");
                let digest_at = encoded.len() - digest.len();
                let prefix_at = digest_at - prefix.len();
                encoded[0] = 0;
                encoded[1] = 1;
                encoded[prefix_at - 1] = 0;
                encoded[prefix_at..digest_at].copy_from_slice(&prefix);
                encoded[digest_at..].copy_from_slice(digest);
                $uint::from_be_slice(&encoded)
            };
            let expected = encode(&digest);
            let s = $uint::from_be_slice(&$signature);

            // A second base, checked against crypto-bigint's own exponentiation.
            let other = n.wrapping_sub(&$uint::from_u32(12345));
            let params = FixedMontyParams::new_vartime(Odd::new(n).expect("an RSA modulus is odd"));
            let other_expected = FixedMontyForm::new(&other, &params).pow(&e).retrieve();

            // Acceleration and the search are safe for a public exponent: the time and the search position reveal
            // only e.
            for (what, constant, search) in [
                (
                    concat!(
                        "rsa",
                        $bits,
                        " hardware s^e mod n, constant time, no search"
                    ),
                    true,
                    false,
                ),
                (
                    concat!("rsa", $bits, " hardware s^e mod n, constant time, search"),
                    true,
                    true,
                ),
                (
                    concat!("rsa", $bits, " hardware s^e mod n, accelerated, no search"),
                    false,
                    false,
                ),
                (
                    concat!("rsa", $bits, " hardware s^e mod n, accelerated, search"),
                    false,
                    true,
                ),
            ] {
                rsa.apply_config(
                    &esp_hal::rsa::Config::default()
                        .with_constant_time(constant)
                        .with_search_acceleration(search),
                )
                .unwrap();
                assert_eq!(exponentiate(rsa, &s), expected, "{}", what);
                assert_eq!(exponentiate(rsa, &other), other_expected, "{}", what);
                time(what, || exponentiate(rsa, &s));
            }

            // A whole hardware verify, as a TLS client runs it for a new key: the setup, the exponentiation
            // accelerated with the search, and the compare with the encoded digest.
            time(
                concat!("rsa", $bits, " hardware setup, r and m_prime"),
                || setup(core::hint::black_box(&$modulus)),
            );
            time(
                concat!("rsa", $bits, " hardware verify, accelerated, search"),
                || {
                    let (n, r, m_prime) = setup(core::hint::black_box(&$modulus));
                    let s = $uint::from_be_slice(core::hint::black_box(&$signature));
                    let mut out = [0u32; $uint::LIMBS];
                    let mut modexp = RsaModularExponentiation::<$op, _>::new(
                        rsa,
                        e.as_words(),
                        n.as_words(),
                        m_prime,
                    );
                    modexp.start_exponentiation(s.as_words(), r.as_words());
                    modexp.read_results(&mut out);
                    $uint::from_words(out) == encode(&digest)
                },
            );
            rsa.apply_config(&esp_hal::rsa::Config::default()).unwrap();

            let key = RsaPublicKey::new(BigUint::from_bytes_be(&$modulus), BigUint::from(65537u32))
                .unwrap();
            key.verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &$signature)
                .unwrap();
            time(
                concat!("rsa", $bits, " software PKCS#1 v1.5 verify"),
                || {
                    core::hint::black_box(&key).verify(
                        Pkcs1v15Sign::new::<Sha256>(),
                        core::hint::black_box(&digest),
                        core::hint::black_box(&$signature),
                    )
                },
            );
        }
    };
}

rsa_verify!(
    rsa2048_verify,
    2048,
    Op2048,
    U2048,
    129,
    wycheproof_rsa::MODULUS_2048,
    wycheproof_rsa::SIGNATURE_2048
);
rsa_verify!(
    rsa4096_verify,
    4096,
    Op4096,
    U4096,
    257,
    wycheproof_rsa::MODULUS_4096,
    wycheproof_rsa::SIGNATURE_4096
);

#[cfg(test)]
#[embedded_test::tests(default_timeout = 60)]
mod tests {
    use super::*;
    use core::hint::black_box;
    use esp_hal::ecc::{Config, Ecc, EccBackend, EccModBase, EllipticCurve};
    use esp_hal::rng::TrngSource;
    use esp_hal::sha::{Sha256Context, ShaBackend};

    /// Hardware point multiplication on both curves, with enhanced security mode on, gives the software's point
    /// for the same scalar.
    #[test]
    fn hardware_point_multiplication_gives_the_software_point() {
        let peripherals = crypto_timing::init();
        let _source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
        let mut ecc = Ecc::new(
            peripherals.ECC,
            Config::default().with_enhanced_security(true),
        )
        .unwrap();
        p256_point_multiplication(&mut ecc);
        p384_point_multiplication(&mut ecc);
    }

    /// An RSA verify's exponentiation in the RSA block gives a valid signature's encoded message, for a 2048-bit and
    /// a 4096-bit Wycheproof key, and the rsa crate verifies the same signatures.
    #[test]
    fn hardware_rsa_exponentiation_gives_the_encoded_message() {
        let peripherals = crypto_timing::init();
        esp_alloc::heap_allocator!(size: 96 * 1024);
        let mut rsa =
            esp_hal::rsa::Rsa::new(peripherals.RSA, esp_hal::rsa::Config::default()).unwrap();
        rsa2048_verify(&mut rsa);
        rsa4096_verify(&mut rsa);
        defmt::info!(
            "heap: max usage {} bytes",
            esp_alloc::HEAP.stats().max_usage
        );
    }

    /// One trip through the ECC work queue: the smallest operation, a secp256r1 modular addition, through the
    /// blocking driver and through the work-queue backend.
    #[test]
    fn a_work_queue_trip_costs_the_difference_over_the_blocking_driver() {
        let mut peripherals = crypto_timing::init();
        let _source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
        let (a, b) = ([1u8; 32], [2u8; 32]);
        let mut blocking_sum = [0u8; 32];
        {
            let mut ecc = Ecc::new(peripherals.ECC.reborrow(), Config::default()).unwrap();
            ecc.modular_addition(EllipticCurve::P256, EccModBase::OrderOfCurve, &a, &b)
                .unwrap()
                .read_scalar_result(&mut blocking_sum)
                .unwrap();
            time("P256 blocking modular addition", || {
                let _sum = ecc
                    .modular_addition(EllipticCurve::P256, EccModBase::OrderOfCurve, &a, &b)
                    .unwrap();
            });
        }
        let mut backend = EccBackend::new(peripherals.ECC, Config::default()).unwrap();
        let _driver = backend.start();
        let mut queued_sum = [0u8; 32];
        let add = |out: &mut [u8; 32]| {
            EllipticCurve::P256
                .modular_addition(EccModBase::OrderOfCurve, &a, &b)
                .and_then(|op| op.with_scalar_result(out))
                .unwrap()
                .process()
                .wait_blocking();
        };
        add(&mut queued_sum);
        assert_eq!(queued_sum, blocking_sum);
        assert_eq!(queued_sum, [3u8; 32]);
        time("P256 work-queue modular addition", || add(&mut queued_sum));
    }

    /// What embedded-tls's handshake runs in software: ECDH and ECDSA on both curves, X25519 and Ed25519.
    #[test]
    fn software_handshake_operations() {
        let peripherals = crypto_timing::init();
        let _source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
        macro_rules! nist {
            ($module:ident, $n:literal) => {{
                use embassy_crypto::$module::{SecretKey, SigningKey};
                let ours = SecretKey::generate().unwrap();
                let theirs = SecretKey::generate().unwrap().public_key().unwrap();
                time(concat!(stringify!($module), " ECDH shared secret"), || {
                    black_box(&ours).diffie_hellman(black_box(&theirs)).unwrap()
                });
                let signing = SigningKey::generate().unwrap();
                let verifying = signing.verifying_key().unwrap();
                let digest = [0x5au8; $n];
                let signature = signing.sign_prehash(&digest).unwrap();
                verifying.verify_prehash(&digest, &signature).unwrap();
                time(concat!(stringify!($module), " ECDSA sign"), || {
                    black_box(&signing)
                        .sign_prehash(black_box(&digest))
                        .unwrap()
                });
                time(concat!(stringify!($module), " ECDSA verify"), || {
                    black_box(&verifying).verify_prehash(black_box(&digest), black_box(&signature))
                });
            }};
        }
        nist!(p256, 32);
        nist!(p384, 48);

        let ours = embassy_crypto::x25519::SecretKey::generate().unwrap();
        let theirs = embassy_crypto::x25519::SecretKey::generate()
            .unwrap()
            .public_key()
            .unwrap();
        time("x25519 shared secret", || {
            black_box(&ours).diffie_hellman(black_box(&theirs)).unwrap()
        });
        let signing = embassy_crypto::ed25519::SigningKey::generate().unwrap();
        let verifying = signing.verifying_key().unwrap();
        let message = [0x5au8; 64];
        let signature = signing.sign(&message).unwrap();
        verifying.verify(&message, &signature).unwrap();
        time("ed25519 sign", || {
            black_box(&signing).sign(black_box(&message)).unwrap()
        });
        time("ed25519 verify", || {
            black_box(&verifying).verify(black_box(&message), black_box(&signature))
        });
    }

    /// Hardware SHA-256 through the work queue gives the software's digest, at the sizes a TLS handshake hashes.
    #[test]
    fn hardware_sha256_gives_the_software_digest() {
        let peripherals = crypto_timing::init();
        let _source = TrngSource::new(peripherals.RNG, peripherals.ADC1);
        let mut backend = ShaBackend::new(peripherals.SHA);
        let _driver = backend.start();
        // Distinct bytes, so that a byte or word swap in the driver changes the digest.
        let data: [u8; 4096] = core::array::from_fn(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8);

        let hardware = |chunks: &[&[u8]]| {
            let mut context = Sha256Context::new();
            for chunk in chunks {
                context.update(chunk).wait_blocking();
            }
            let mut digest = [0u8; 32];
            context.finalize(&mut digest).wait_blocking();
            digest
        };
        let software = |chunks: &[&[u8]]| {
            let mut context = embassy_crypto::Sha256::new();
            for chunk in chunks {
                context.update(chunk);
            }
            context.finalize()
        };

        // Padding, the 55 and 56 byte boundary of the length field, a partial last block, and the timed sizes.
        for len in [0, 1, 55, 56, 63, 64, 65, 1000, 1024, 4096] {
            let chunks = [&data[..len]];
            assert_eq!(hardware(&chunks), software(&chunks), "{}", len);
        }
        // Updates that straddle blocks, as a transcript of uneven handshake messages does.
        let uneven = [&data[..3], &data[3..64], &data[64..264], &data[264..271]];
        assert_eq!(hardware(&uneven), software(&uneven), "uneven chunks");
        let transcript: [&[u8]; 50] = core::array::from_fn(|i| &data[i * 64..(i + 1) * 64]);
        assert_eq!(hardware(&transcript), software(&transcript), "transcript");

        for (what_hw, what_sw, chunks) in [
            (
                "sha256 hardware 64 B",
                "sha256 software 64 B",
                &[&data[..64]][..],
            ),
            (
                "sha256 hardware 1 KB",
                "sha256 software 1 KB",
                &[&data[..1024]][..],
            ),
            (
                "sha256 hardware 4 KB",
                "sha256 software 4 KB",
                &[&data[..]][..],
            ),
            (
                "sha256 hardware 50 updates of 64 B",
                "sha256 software 50 updates of 64 B",
                &transcript[..],
            ),
        ] {
            time(what_hw, || hardware(black_box(chunks)));
            time(what_sw, || software(black_box(chunks)));
        }
    }
}
