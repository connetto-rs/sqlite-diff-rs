//! Ports of the numeric conversions SQLite 3.53 applies when it stores a value
//! under a column affinity: `sqlite3AtoF`, `sqlite3Atoi64`, the integer
//! affinity checks, and the `"%!.17g"` rendering built on `sqlite3FpDecode`.
//!
//! Each function mirrors its C counterpart step for step, because byte parity
//! with the session extension depends on SQLite's own rounding, which differs
//! from Rust's correctly rounded conversions in the last digits.

#![expect(
    clippy::many_single_char_names,
    reason = "names follow SQLite's C source so the port can be checked against it line by line"
)]

const POWERS_OF_10_FIRST: i32 = -348;
const POWERS_OF_10_LAST: i32 = 347;

/// `1.0e+0 << 63` through `1.0e+26 >> 23`, the leading 64 bits of `10^p`.
const BASE: [u64; 27] = [
    0x8000_0000_0000_0000,
    0xa000_0000_0000_0000,
    0xc800_0000_0000_0000,
    0xfa00_0000_0000_0000,
    0x9c40_0000_0000_0000,
    0xc350_0000_0000_0000,
    0xf424_0000_0000_0000,
    0x9896_8000_0000_0000,
    0xbebc_2000_0000_0000,
    0xee6b_2800_0000_0000,
    0x9502_f900_0000_0000,
    0xba43_b740_0000_0000,
    0xe8d4_a510_0000_0000,
    0x9184_e72a_0000_0000,
    0xb5e6_20f4_8000_0000,
    0xe35f_a931_a000_0000,
    0x8e1b_c9bf_0400_0000,
    0xb1a2_bc2e_c500_0000,
    0xde0b_6b3a_7640_0000,
    0x8ac7_2304_89e8_0000,
    0xad78_ebc5_ac62_0000,
    0xd8d7_26b7_177a_8000,
    0x8786_7832_6eac_9000,
    0xa968_163f_0a57_b400,
    0xd3c2_1bce_cced_a100,
    0x8459_5161_4014_84a0,
    0xa56f_a5b9_9019_a5c8,
];

/// Leading 64 bits of `10^(27 * (i - 13))`, with `10^-1` at index 13.
const SCALE: [u64; 26] = [
    0x8049_a4ac_0c58_11ae,
    0xcf42_894a_5dce_35ea,
    0xa76c_5823_38ed_2621,
    0x873e_4f75_e222_4e68,
    0xda7f_5bf5_9096_6848,
    0xb080_392c_c434_9dec,
    0x8e93_8662_882a_f53e,
    0xe658_29b3_046b_0afa,
    0xba12_1a46_50e4_ddeb,
    0x964e_858c_91ba_2655,
    0xf2d5_6790_ab41_c2a2,
    0xc428_d05a_a475_1e4c,
    0x9e74_d1b7_91e0_7e48,
    0xcccc_cccc_cccc_cccc,
    0xcecb_8f27_f420_0f3a,
    0xa70c_3c40_a64e_6c51,
    0x86f0_ac99_b4e8_dafd,
    0xda01_ee64_1a70_8de9,
    0xb01a_e745_b101_e9e4,
    0x8e41_ade9_fbeb_c27d,
    0xe5d3_ef28_2a24_2e81,
    0xb9a7_4a06_37ce_2ee1,
    0x95f8_3d0a_1fb6_9cd9,
    0xf24a_01a7_3cf2_dccf,
    0xc3b8_3581_09e8_4f07,
    0x9e19_db92_b4e3_1ba9,
];

/// The next 32 bits of each `SCALE` entry.
const SCALE_LO: [u32; 26] = [
    0x205b_896d,
    0x5206_4cad,
    0xaf2a_f2b8,
    0x5a77_44a7,
    0xaf39_a475,
    0xbd8d_794e,
    0x547e_b47b,
    0x0cb4_a5a3,
    0x92f3_4d62,
    0x3a6a_07f9,
    0xfae2_7299,
    0xaa97_e14c,
    0x775e_a265,
    0xcccc_cccc,
    0x0000_0000,
    0x9990_90b6,
    0x69a0_28bb,
    0xe80e_6f48,
    0x5ec0_5dd0,
    0x1458_8f14,
    0x8f16_68c9,
    0x6d95_3e2c,
    0x4abd_af10,
    0xbc63_3b39,
    0x0a86_2f81,
    0x6c07_a2c2,
];

const fn bit(n: u32) -> u64 {
    1 << n
}

/// High and low 64 bits of `a * b`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "splitting a 128-bit product into its two halves"
)]
fn multiply128(a: u64, b: u64) -> (u64, u64) {
    let r = u128::from(a) * u128::from(b);
    ((r >> 64) as u64, r as u64)
}

/// Upper 64 bits and the next 32 bits of `((a << 32) + a_lo) * b`, from 96 by 64 bits.
#[expect(
    clippy::cast_possible_truncation,
    reason = "extracting fixed-width slices of a 128-bit product"
)]
fn multiply160(a: u64, a_lo: u32, b: u64) -> (u64, u32) {
    let mut r = u128::from(a) * u128::from(b);
    r += (u128::from(a_lo) * u128::from(b)) >> 32;
    ((r >> 64) as u64, (r >> 32) as u32)
}

/// `floor(log2(10^p))`.
const fn pwr10to2(p: i32) -> i32 {
    (p * 108_853) >> 15
}

/// `floor(log10(2^p))`.
const fn pwr2to10(p: i32) -> i32 {
    (p * 78_913) >> 18
}

/// `i32` to `u32` for a shift amount or index that the algorithm keeps non-negative.
#[expect(
    clippy::cast_sign_loss,
    reason = "callers pass values the algorithm keeps non-negative"
)]
fn unsigned(v: i32) -> u32 {
    debug_assert!(v >= 0, "negative shift or index {v}");
    v as u32
}

/// Leading 64 bits of `10^p` and the next 32 bits, for `p` in `-348..=347`.
fn power_of_ten(p: i32) -> (u64, u32) {
    debug_assert!((POWERS_OF_10_FIRST..=POWERS_OF_10_LAST).contains(&p));
    let (g, n) = if p < 0 {
        if p == -1 {
            return (SCALE[13], SCALE_LO[13]);
        }
        let (g, n) = (p / 27, p % 27);
        if n == 0 { (g, n) } else { (g - 1, n + 27) }
    } else if p < 27 {
        return (BASE[unsigned(p) as usize], 0);
    } else {
        (p / 27, p % 27)
    };
    let index = unsigned(g + 13) as usize;
    let s = SCALE[index];
    if n == 0 {
        return (s, SCALE_LO[index]);
    }
    let (mut x, mut lo) = multiply160(s, SCALE_LO[index], BASE[unsigned(n) as usize]);
    if x & bit(63) == 0 {
        x = (x << 1) | u64::from((lo >> 31) & 1);
        lo = (lo << 1) | 1;
    }
    (x, lo)
}

/// `(d, p)` with `m * 2^e ≈ d * 10^p` and `d` holding at least `n` significant digits.
///
/// `m` must have its top bit set.
fn fp2_convert10(m: u64, e: i32, n: i32) -> (u64, i32) {
    debug_assert!((1..=18).contains(&n));
    let p = n - 1 - pwr2to10(e + 63);
    let (h, _) = multiply128(m, power_of_ten(p).0);
    let d = if n == 18 {
        let h = h >> unsigned(-(e + pwr10to2(p) + 2));
        u64::midpoint(h, (h << 1) & 2)
    } else {
        h >> unsigned(-(e + pwr10to2(p) + 1))
    };
    (d, -p)
}

/// The double nearest `d * 10^p` by SQLite's rounding, for `d > 0`.
fn fp10_convert2(d: u64, p: i32) -> f64 {
    debug_assert!(d > 0);
    if p < POWERS_OF_10_FIRST {
        return 0.0;
    }
    if p > POWERS_OF_10_LAST {
        return f64::INFINITY;
    }
    let b = 64 - i32::from(u8::try_from(d.leading_zeros()).unwrap_or(64));
    let lp = pwr10to2(p);
    let mut e = 53 - b - lp;
    if e > 1074 {
        if e >= 1130 {
            return 0.0;
        }
        e = 1074;
    }
    let s = unsigned(-(e - (64 - b) + lp + 3));
    let (mut pow_high, mut pow_low) = power_of_ten(p);
    if pow_low != 0 {
        pow_high += 1;
        pow_low = !pow_low;
    }
    let x = d << unsigned(64 - b);
    let (mut hi, lo) = multiply128(x, pow_high);
    let mid1 = lo >> 32;
    let mut sticky = 1;
    if hi & (bit(s) - 1) == 0 {
        let mid2 = multiply128(x, u64::from(pow_low) << 32).0 >> 32;
        sticky = u64::from((mid1.wrapping_sub(mid2) & 0xffff_ffff) > 1);
        hi -= u64::from(mid1 < mid2);
    }
    let mut u = (hi >> s) | sticky;
    if u >= bit(55) - 2 {
        u = (u >> 1) | (u & 1);
        e -= 1;
    }
    let mut m = (u + 1 + ((u >> 2) & 1)) >> 2;
    if e <= -972 {
        return f64::INFINITY;
    }
    if m & bit(52) != 0 {
        m = (m & !bit(52)) | (u64::from(unsigned(1075 - e)) << 52);
    }
    f64::from_bits(m)
}

/// `sqlite3Isspace`: space, tab, newline, vertical tab, form feed, carriage return.
const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The byte at `i`, or NUL past the end as the C string would read it.
fn byte_at(text: &[u8], i: usize) -> u8 {
    text.get(i).copied().unwrap_or(0)
}

/// The exponent of an `e` just consumed before `i`, and where parsing resumes.
///
/// Without exponent digits, parsing resumes one byte back, as the C `z--` does.
fn exponent(text: &[u8], mut i: usize) -> (usize, Option<i32>) {
    let digit = |i: usize| byte_at(text, i).wrapping_sub(b'0');
    let sign = if byte_at(text, i) == b'-' {
        i += 1;
        -1
    } else {
        if byte_at(text, i) == b'+' {
            i += 1;
        }
        1
    };
    if digit(i) >= 10 {
        return (i - 1, None);
    }
    let mut exp = i32::from(digit(i));
    i += 1;
    while digit(i) < 10 {
        exp = if exp < 10_000 {
            exp * 10 + i32::from(digit(i))
        } else {
            10_000
        };
        i += 1;
    }
    (i, Some(sign * exp))
}

/// Whether nothing but whitespace follows `i` before the end or a NUL.
fn only_space_after(text: &[u8], mut i: usize) -> bool {
    while is_space(byte_at(text, i)) {
        i += 1;
    }
    byte_at(text, i) == 0
}

/// `sqlite3AtoF`: the double a text spells and SQLite's parse flags.
///
/// A positive flag means the whole text is a number: bit 1 marks a decimal
/// point or exponent. Zero or negative means it is not. A NUL byte ends the
/// text, as it ends the C string.
pub(super) fn ato_f(text: &[u8]) -> (i32, f64) {
    let at = |i: usize| byte_at(text, i);
    let digit = |i: usize| at(i).wrapping_sub(b'0');
    let mut i = 0;
    let mut neg = false;
    let mut s: u64 = 0;
    let mut d: i32 = 0;
    let mut state: i32 = 0;

    loop {
        let integer_part = if digit(i) < 10 {
            true
        } else if at(i) == b'-' || at(i) == b'+' {
            neg = at(i) == b'-';
            i += 1;
            digit(i) < 10
        } else if is_space(at(i)) {
            while is_space(at(i)) {
                i += 1;
            }
            continue;
        } else {
            false
        };
        if integer_part {
            state = 1;
            s = u64::from(digit(i));
            i += 1;
            while digit(i) < 10 {
                s = s * 10 + u64::from(digit(i));
                i += 1;
                if s >= (u64::MAX - 9) / 10 {
                    state = 9;
                    while digit(i) < 10 {
                        i += 1;
                        d += 1;
                    }
                    break;
                }
            }
        }
        break;
    }

    if at(i) == b'.' {
        i += 1;
        if digit(i) < 10 {
            state |= 1;
            while digit(i) < 10 {
                if s < (u64::MAX - 9) / 10 {
                    s = s * 10 + u64::from(digit(i));
                    d -= 1;
                } else {
                    state = 11;
                }
                i += 1;
            }
        } else if state == 0 {
            return (0, 0.0);
        }
        state |= 2;
    } else if state == 0 {
        return (0, 0.0);
    }

    if at(i) == b'e' || at(i) == b'E' {
        let (next, exp) = exponent(text, i + 1);
        i = next;
        if let Some(exp) = exp {
            state |= 2;
            d += exp;
        }
    }

    let mut result = if s == 0 {
        state |= 4;
        0.0
    } else {
        fp10_convert2(s, d)
    };
    if neg {
        result = -result;
    }
    if only_space_after(text, i) {
        (state, result)
    } else {
        (state | !0xf, result)
    }
}

/// `compare2pow63`: the sign of a 19-digit number minus `9223372036854775808`.
fn compare2pow63(digits: &[u8]) -> i32 {
    let pow63 = b"922337203685477580";
    for (&digit, &limit) in digits.iter().zip(pow63) {
        let c = (i32::from(digit) - i32::from(limit)) * 10;
        if c != 0 {
            return c;
        }
    }
    i32::from(digits.get(18).copied().unwrap_or(b'8')) - i32::from(b'8')
}

/// `sqlite3Atoi64`: whether the whole text is a decimal integer fitting `i64`.
///
/// Returns 0 and the value on success. The text is used to its full length,
/// NUL bytes included, as SQLite passes the value's byte count.
pub(super) fn atoi64(text: &[u8]) -> (i32, i64) {
    let end = text.len();
    let mut k = 0;
    while k < end && is_space(text[k]) {
        k += 1;
    }
    let mut neg = false;
    if k < end {
        if text[k] == b'-' {
            neg = true;
            k += 1;
        } else if text[k] == b'+' {
            k += 1;
        }
    }
    let start = k;
    while k < end && text[k] == b'0' {
        k += 1;
    }
    let mut u: u64 = 0;
    let mut i = 0;
    while k + i < end && text[k + i].is_ascii_digit() {
        u = u
            .wrapping_mul(10)
            .wrapping_add(u64::from(text[k + i] - b'0'));
        i += 1;
    }
    let value = match i64::try_from(u) {
        Ok(v) if neg => -v,
        Ok(v) => v,
        Err(_) if neg => i64::MIN,
        Err(_) => i64::MAX,
    };
    let mut rc = 0;
    if i == 0 && start == k {
        rc = -1;
    } else if text[k + i..].iter().any(|&c| !is_space(c)) {
        rc = 1;
    }
    if i < 19 {
        return (rc, value);
    }
    let j = if i > 19 { 1 } else { compare2pow63(&text[k..]) };
    if j < 0 {
        return (rc, value);
    }
    let value = if neg { i64::MIN } else { i64::MAX };
    if j > 0 {
        (2, value)
    } else {
        (if neg { rc } else { 3 }, value)
    }
}

/// `sqlite3RealToI64`: `r` truncated to an integer, saturating near the `i64` bounds.
#[expect(
    clippy::cast_possible_truncation,
    reason = "truncation toward zero is the conversion SQLite applies"
)]
fn real_to_i64(r: f64) -> i64 {
    debug_assert!(!r.is_nan(), "callers map NaN to NULL before converting");
    if r < -9_223_372_036_854_774_784.0 {
        i64::MIN
    } else if r > 9_223_372_036_854_774_784.0 {
        i64::MAX
    } else {
        r as i64
    }
}

/// `sqlite3VdbeIntegerAffinity`: the integer equal to `r`, strictly inside the `i64` range.
#[expect(
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "SQLite compares the real with the integer converted back to real"
)]
pub(super) fn integral(r: f64) -> Option<i64> {
    let ix = real_to_i64(r);
    (r == ix as f64 && ix > i64::MIN && ix < i64::MAX).then_some(ix)
}

/// `alsoAnInt`: the integer a parsed text also denotes, if any.
#[expect(
    clippy::cast_precision_loss,
    reason = "SQLite compares the bits of the real with the integer converted to real"
)]
pub(super) fn also_an_int(text: &[u8], r: f64) -> Option<i64> {
    let i = real_to_i64(r);
    let same = r == 0.0
        || (r.to_bits() == (i as f64).to_bits()
            && (-2_251_799_813_685_248..2_251_799_813_685_248).contains(&i));
    if same {
        return Some(i);
    }
    let (rc, value) = atoi64(text);
    (rc == 0).then_some(value)
}

/// Fixed buffer for a rendered number, which never exceeds 25 bytes.
pub(crate) struct NumberText {
    bytes: [u8; 32],
    len: usize,
}

impl NumberText {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; 32],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        self.bytes[self.len] = byte;
        self.len += 1;
    }

    fn extend(&mut self, bytes: &[u8]) {
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    fn zeros(&mut self, count: usize) {
        for _ in 0..count {
            self.push(b'0');
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl core::fmt::Write for NumberText {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.extend(s.as_bytes());
        Ok(())
    }
}

/// Decimal text of an integer, as `sqlite3Int64ToText` writes it.
pub(super) fn integer_text(i: i64) -> NumberText {
    use core::fmt::Write;
    let mut text = NumberText::new();
    let _ = write!(text, "{i}");
    text
}

/// The significant digits of a positive finite double, from `sqlite3FpDecode`
/// with 17 digits and the precision reduction of the `!` flag.
struct Decoded {
    buf: [u8; 24],
    /// Index of the first digit in `buf`.
    start: usize,
    /// Number of significant digits.
    n: usize,
    /// Position of the decimal point relative to the first digit.
    decimal_point: i32,
}

impl Decoded {
    fn digits(&self) -> &[u8] {
        &self.buf[self.start..self.start + self.n]
    }

    fn digit_value(&self, count: usize) -> u64 {
        self.buf[self.start..self.start + count]
            .iter()
            .fold(0, |v, &c| v * 10 + u64::from(c - b'0'))
    }
}

/// `sqlite3FpDecode(r, 17, 20)` for a positive finite nonzero `r`.
fn fp_decode(r: f64) -> Decoded {
    const I_ROUND: usize = 17;
    let bits = r.to_bits();
    let exponent = i32::from(u16::try_from((bits >> 52) & 0x7ff).unwrap_or(0));
    let mut v = bits & 0x000f_ffff_ffff_ffff;
    let e = if exponent == 0 {
        let nn = v.leading_zeros();
        v <<= nn;
        -1074 - i32::from(u8::try_from(nn).unwrap_or(0))
    } else {
        v = (v << 11) | bit(63);
        exponent - 1086
    };
    let (mut v, exp) = fp2_convert10(v, e, 18);

    let mut decoded = Decoded {
        buf: [b'0'; 24],
        start: 24,
        n: 0,
        decimal_point: 0,
    };
    while v > 0 {
        decoded.start -= 1;
        decoded.buf[decoded.start] = b'0' + u8::try_from(v % 10).unwrap_or(0);
        v /= 10;
    }
    let mut n = 24 - decoded.start;
    decoded.decimal_point = i32::try_from(n).unwrap_or(0) + exp;

    // C also tests `n > mxRound`, which is 20 here and so implied by `n > 17`.
    if n > I_ROUND {
        let mut round = I_ROUND;
        let z = |i: usize| decoded.buf[decoded.start + i];
        let digits_after = |jj: usize| exp + i32::try_from(n - jj).unwrap_or(0);
        if z(15) == b'9' && z(14) == b'9' {
            let mut jj = 14;
            while jj > 0 && z(jj - 1) == b'9' {
                jj -= 1;
            }
            let v2 = if jj == 0 {
                1
            } else {
                decoded.digit_value(jj) + 1
            };
            if exact(r, fp10_convert2(v2, digits_after(jj))) {
                round = jj + 1;
            }
        } else if decoded.decimal_point >= i32::try_from(n).unwrap_or(i32::MAX)
            || (z(15) == b'0' && z(14) == b'0' && z(13) == b'0')
        {
            let mut jj = 13;
            while z(jj - 1) == b'0' {
                jj -= 1;
            }
            let v2 = decoded.digit_value(jj);
            if exact(r, fp10_convert2(v2, digits_after(jj))) {
                round = jj + 1;
            }
        }
        n = round;
        if decoded.buf[decoded.start + round] >= b'5' {
            let mut j = round - 1;
            loop {
                let slot = decoded.start + j;
                decoded.buf[slot] += 1;
                if decoded.buf[slot] <= b'9' {
                    break;
                }
                decoded.buf[slot] = b'0';
                if j == 0 {
                    decoded.start -= 1;
                    decoded.buf[decoded.start] = b'1';
                    n += 1;
                    decoded.decimal_point += 1;
                    break;
                }
                j -= 1;
            }
        }
    }
    while decoded.buf[decoded.start + n - 1] == b'0' {
        n -= 1;
    }
    decoded.n = n;
    decoded
}

#[expect(clippy::float_cmp, reason = "SQLite requires an exact round trip")]
fn exact(a: f64, b: f64) -> bool {
    a == b
}

/// A real as `sqlite3_str_appendf("%!.17g")` renders it, which is how SQLite
/// converts a real to text. `r` must not be NaN.
pub(super) fn real_text(r: f64) -> NumberText {
    let mut out = NumberText::new();
    let negative = r < 0.0;
    let magnitude = r.abs();
    if magnitude.is_infinite() {
        out.extend(if negative { b"-Inf" } else { b"Inf" });
        return out;
    }
    if negative {
        out.push(b'-');
    }
    if magnitude == 0.0 {
        out.extend(b"0.0");
        return out;
    }
    let decoded = fp_decode(magnitude);
    let digits = decoded.digits();
    let exp = decoded.decimal_point - 1;
    let exponential = !(-4..=16).contains(&exp);
    let mut precision: i32 = if exponential { 16 } else { 16 - exp };
    let mut e2: i32 = if exponential { 0 } else { exp };

    let mut j = 0;
    if e2 < 0 {
        out.push(b'0');
    } else {
        j = digits.len().min(unsigned(e2 + 1) as usize);
        out.extend(&digits[..j]);
        e2 -= i32::try_from(j).unwrap_or(0);
        if e2 >= 0 {
            out.zeros(unsigned(e2 + 1) as usize);
            e2 = -1;
        }
    }
    out.push(b'.');
    if e2 < -1 && precision > 0 {
        let nn = (-1 - e2).min(precision);
        out.zeros(unsigned(nn) as usize);
        precision -= nn;
    }
    if precision > 0 {
        let nn = (digits.len() - j).min(unsigned(precision) as usize);
        out.extend(&digits[j..j + nn]);
    }
    while out.bytes[out.len - 1] == b'0' {
        out.len -= 1;
    }
    if out.bytes[out.len - 1] == b'.' {
        out.push(b'0');
    }
    if exponential {
        out.push(b'e');
        out.push(if exp < 0 { b'-' } else { b'+' });
        let exp = exp.unsigned_abs();
        if exp >= 100 {
            out.push(b'0' + u8::try_from(exp / 100).unwrap_or(0));
        }
        out.push(b'0' + u8::try_from(exp / 10 % 10).unwrap_or(0));
        out.push(b'0' + u8::try_from(exp % 10).unwrap_or(0));
    }
    out
}
