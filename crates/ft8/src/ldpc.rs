// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from WSJT-X, Copyright (C) 2001-2023 Joe Taylor, K1JT, and contributors.
//! Port of WSJT-X bpdecode174_91, encode174_91, and crc14.cpp.
use crate::tables::{CHECKS, GENERATOR};

pub fn crc(message: &[u8]) -> u16 {
    let mut crc = 0u16;
    // WSJT-X feeds twelve bytes: 77 payload bits and nineteen zero bits.
    for bit in message[..77]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(0, 19))
    {
        let bit = bit as u16;
        let feedback = crc >> 13;
        crc = ((crc << 1) | bit) & 0x3fff;
        if feedback != 0 {
            crc ^= 0x2757;
        }
    }
    crc
}

pub fn encode(message: &[u8; 77]) -> [u8; 174] {
    let mut cw = [0; 174];
    cw[..77].copy_from_slice(message);
    let c = crc(message);
    for i in 0..14 {
        cw[77 + i] = ((c >> (13 - i)) & 1) as u8;
    }
    for (r, row) in GENERATOR.iter().enumerate() {
        for j in 0..91 {
            let nibble = row.as_bytes()[j / 4];
            let value = if nibble <= b'9' {
                nibble - b'0'
            } else {
                nibble - b'a' + 10
            };
            cw[91 + r] ^= cw[j] * ((value >> (3 - j % 4)) & 1);
        }
    }
    cw
}

pub fn decode(llr: &[f32; 174]) -> Option<[u8; 77]> {
    let mut incoming = [[0f32; 7]; 83];
    let mut saved = [[0f32; 174]; 2];
    let mut accumulated = [0f32; 174];
    let mut last = 83;
    let mut stalled = 0;
    for iteration in 0..=30 {
        let mut posterior = *llr;
        for (r, row) in CHECKS.iter().enumerate() {
            for (e, &bit) in row.iter().enumerate().filter(|(_, b)| **b > 0) {
                posterior[bit - 1] += incoming[r][e];
            }
        }
        for i in 0..174 {
            accumulated[i] += posterior[i];
        }
        if (1..=2).contains(&iteration) {
            saved[iteration - 1] = accumulated;
        }
        let cw: [u8; 174] = std::array::from_fn(|i| u8::from(posterior[i] > 0.0));
        let failures = CHECKS
            .iter()
            .filter(|row| {
                row.iter()
                    .filter(|&&b| b > 0)
                    .fold(0, |a, &b| a ^ cw[b - 1])
                    != 0
            })
            .count();
        if failures == 0 && cw.iter().any(|&b| b != 0) {
            let received = cw[77..91].iter().fold(0u16, |a, &b| (a << 1) | b as u16);
            if crc(&cw[..77]) == received {
                return Some(cw[..77].try_into().unwrap());
            }
        }
        if failures < last {
            stalled = 0;
        } else {
            stalled += 1;
        }
        if iteration >= 10 && stalled >= 5 && failures > 15 {
            break;
        }
        last = failures;
        for (r, row) in CHECKS.iter().enumerate() {
            let mut tanh = [0.0; 7];
            for (e, &b) in row.iter().enumerate().filter(|(_, b)| **b > 0) {
                tanh[e] = (-0.5 * (posterior[b - 1] - incoming[r][e])).tanh();
            }
            for (e, _) in row.iter().enumerate().filter(|(_, b)| **b > 0) {
                let product = row
                    .iter()
                    .enumerate()
                    .filter(|(j, b)| *j != e && **b > 0)
                    .fold(1.0, |a, (j, _)| a * tanh[j]);
                incoming[r][e] = 2.0 * (-product).clamp(-0.999999, 0.999999).atanh();
            }
        }
    }
    saved.iter().find_map(osd)
}

// Most-reliable-basis elimination and order-two test patterns from osd174_91.
// Bit-packed rows replace Fortran's byte arrays; CRC is checked only on the
// minimum-distance word, never used to select among test patterns.
fn osd(llr: &[f32; 174]) -> Option<[u8; 77]> {
    type Word = [u64; 3];
    fn get(w: &Word, i: usize) -> bool {
        w[i / 64] & (1 << (i % 64)) != 0
    }
    fn flip(w: &mut Word, i: usize) {
        w[i / 64] ^= 1 << (i % 64);
    }
    fn xor(a: Word, b: Word) -> Word {
        std::array::from_fn(|i| a[i] ^ b[i])
    }
    let mut order: Vec<usize> = (0..174).collect();
    order.sort_by(|&a, &b| llr[b].abs().total_cmp(&llr[a].abs()));
    let mut rows = [[0u64; 3]; 91];
    for (i, row) in rows.iter_mut().enumerate() {
        let mut original = [0u8; 174];
        original[i] = 1;
        for (r, hex) in GENERATOR.iter().enumerate() {
            let value = hex.as_bytes()[i / 4];
            let nibble = if value <= b'9' {
                value - b'0'
            } else {
                value - b'a' + 10
            };
            original[91 + r] = (nibble >> (3 - i % 4)) & 1;
        }
        for (j, &index) in order.iter().enumerate() {
            if original[index] != 0 {
                flip(row, j);
            }
        }
    }
    for diagonal in 0..91 {
        let (column, pivot) = (diagonal..174).find_map(|column| {
            (diagonal..91)
                .find(|&row| get(&rows[row], column))
                .map(|row| (column, row))
        })?;
        rows.swap(diagonal, pivot);
        if column != diagonal {
            order.swap(diagonal, column);
            for row in &mut rows {
                if get(row, column) != get(row, diagonal) {
                    flip(row, column);
                    flip(row, diagonal);
                }
            }
        }
        for row in 0..91 {
            if row != diagonal && get(&rows[row], diagonal) {
                rows[row] = xor(rows[row], rows[diagonal]);
            }
        }
    }
    let mut hard = [0u64; 3];
    for (j, &i) in order.iter().enumerate() {
        if llr[i] >= 0.0 {
            flip(&mut hard, j);
        }
    }
    let mut zero = [0u64; 3];
    for (i, &row) in rows.iter().enumerate() {
        if get(&hard, i) {
            zero = xor(zero, row);
        }
    }
    let distance = |word: Word| -> f32 {
        let differences = xor(word, hard);
        order
            .iter()
            .enumerate()
            .filter(|(j, _)| get(&differences, *j))
            .map(|(_, &i)| llr[i].abs())
            .sum()
    };
    let mut best = zero;
    let mut minimum = distance(zero);
    // ntheta=10 over the first 40 parity bits (WSJT-X depth 2).
    let mut consider = |word: Word| {
        let difference = xor(word, hard);
        let errors = (91..131).filter(|&i| get(&difference, i)).count();
        if errors > 10 {
            return;
        }
        let d = distance(word);
        if d < minimum {
            minimum = d;
            best = word;
        }
    };
    for i in (0..91).rev() {
        let one = xor(zero, rows[i]);
        consider(one);
        for &row in &rows[..i] {
            consider(xor(one, row));
        }
    }
    let mut cw = [0u8; 174];
    for (j, &i) in order.iter().enumerate() {
        cw[i] = u8::from(get(&best, j));
    }
    let received = cw[77..91].iter().fold(0u16, |v, &b| (v << 1) | b as u16);
    if cw.iter().any(|&b| b != 0) && crc(&cw[..77]) == received {
        Some(cw[..77].try_into().unwrap())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generator_satisfies_every_parity_check() {
        for i in 0..77 {
            let mut message = [0; 77];
            message[i] = 1;
            let cw = encode(&message);
            for row in CHECKS {
                assert_eq!(
                    row.iter()
                        .filter(|&&b| b > 0)
                        .fold(0, |v, &b| v ^ cw[b - 1]),
                    0,
                    "payload bit {i}"
                );
            }
        }
    }
    #[test]
    fn crc_matches_polynomial_long_division() {
        // Independent integer polynomial division, including the augmented zeros.
        let mut message = [0; 77];
        for i in [0, 9, 19, 38, 55, 76] {
            message[i] = 1;
        }
        let n = message.iter().fold(0u128, |n, &b| (n << 1) | b as u128);
        let mut remainder = n << 19;
        let polynomial = 0x6757u128;
        for bit in (14..96).rev() {
            if remainder & (1u128 << bit) != 0 {
                remainder ^= polynomial << (bit - 14);
            }
        }
        assert_eq!(crc(&message) as u128, remainder);
    }
}
