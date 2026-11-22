// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from WSJT-X, Copyright (C) 2001-2023 Joe Taylor, K1JT, and contributors.
//! Receive-side port of WSJT-X packjt77.f90.
use std::collections::HashMap;
#[derive(Default)]
pub struct Messages {
    calls: HashMap<(u32, u32), String>,
}
fn bits(b: &[u8], start: usize, n: usize) -> u128 {
    b[start..start + n]
        .iter()
        .fold(0, |v, &x| (v << 1) | x as u128)
}
fn grid4(n: u32) -> String {
    format!(
        "{}{}{}{}",
        (b'A' + (n / 1800) as u8) as char,
        (b'A' + ((n / 100) % 18) as u8) as char,
        (b'0' + ((n / 10) % 10) as u8) as char,
        (b'0' + (n % 10) as u8) as char
    )
}
fn grid6(n: u32, radix: u32) -> Option<String> {
    if n >= 32400 * radix * radix {
        return None;
    }
    let grid = grid4(n / (radix * radix));
    let a = (n / radix) % radix;
    let c = n % radix;
    if radix == 25 && a == 24 && c == 24 {
        return Some(grid);
    }
    Some(format!(
        "{grid}{}{}",
        (b'A' + a as u8) as char,
        (b'A' + c as u8) as char
    ))
}
fn base(mut n: u128, alphabet: &[u8], len: usize) -> String {
    let mut s = vec![b' '; len];
    for c in s.iter_mut().rev() {
        *c = alphabet[(n % alphabet.len() as u128) as usize];
        n /= alphabet.len() as u128;
    }
    String::from_utf8(s).unwrap().trim().to_owned()
}
impl Messages {
    fn save(&mut self, call: &str) {
        if call.len() < 3 || call.contains(['<', ' ']) {
            return;
        }
        let mut n = 0u64;
        let a = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";
        for c in call.bytes().chain(std::iter::repeat(b' ')).take(11) {
            let Some(i) = a.iter().position(|&x| x == c) else {
                return;
            };
            n = 38 * n + i as u64;
        }
        let h = n.wrapping_mul(47055833459);
        for width in [10, 12, 22] {
            self.calls
                .insert((width, (h >> (64 - width)) as u32), call.to_owned());
        }
    }
    fn hash(&self, width: u32, n: u32) -> String {
        self.calls
            .get(&(width, n))
            .map(|s| format!("<{s}>"))
            .unwrap_or_else(|| "<...>".to_owned())
    }
    fn call(&self, n: u32) -> Option<String> {
        match n {
            0 => return Some("DE".into()),
            1 => return Some("QRZ".into()),
            2 => return Some("CQ".into()),
            3..=1002 => return Some(format!("CQ {:03}", n - 3)),
            1003..=532443 => {
                return Some(format!(
                    "CQ {}",
                    base((n - 1003) as u128, b" ABCDEFGHIJKLMNOPQRSTUVWXYZ", 4)
                ));
            }
            532444..=2063591 => return None,
            2063592..=6257895 => return Some(self.hash(22, n - 2063592)),
            _ => {}
        }
        let mut n = n - 6257896;
        let alphabets: [&[u8]; 6] = [
            b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            b"0123456789",
            b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ];
        let mut s = [b' '; 6];
        for i in (0..6).rev() {
            s[i] = *alphabets[i].get((n % alphabets[i].len() as u32) as usize)?;
            n /= alphabets[i].len() as u32;
        }
        if n != 0 {
            return None;
        }
        let s = String::from_utf8(s.to_vec()).ok()?.trim().to_owned();
        let area = s.bytes().rposition(|c| c.is_ascii_digit());
        let valid = area.is_some_and(|i| {
            (i == 1 || i == 2)
                && s[..i].bytes().any(|c| c.is_ascii_uppercase())
                && s[i + 1..].bytes().all(|c| c.is_ascii_uppercase())
        });
        if s.contains(' ') || s.len() < 3 || s.starts_with('Q') || !valid {
            None
        } else {
            Some(s)
        }
    }

    fn wspr(&mut self, b: &[u8; 77]) -> Option<String> {
        if b[49] == 0 && b[48] != 0 {
            if b[47] != 0 {
                return None;
            }
            let call = self.hash(22, bits(b, 0, 22) as u32);
            return Some(format!("{call} {}", grid6(bits(b, 22, 25) as u32, 25)?));
        }
        let call = self.call(bits(b, 0, 28) as u32)?;
        if b[49] == 0 {
            let grid = bits(b, 28, 15) as u32;
            if grid >= 32400 {
                return None;
            }
            let power = (bits(b, 43, 5) as f32 * 10.0 / 3.0).round() as u32;
            if power > 60 {
                return None;
            }
            self.save(&call);
            return Some(format!("{call} {} {power}", grid4(grid)));
        }
        let n = bits(b, 28, 16) as u32;
        let power = (bits(b, 44, 5) as f32 * 10.0 / 3.0).round() as u32;
        if power > 60 {
            return None;
        }
        let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let full = if n < 46656 {
            format!(
                "{}/{call}",
                if n == 0 {
                    "0".to_string()
                } else {
                    base(n as u128, alphabet, 3)
                        .trim_start_matches('0')
                        .to_string()
                }
            )
        } else {
            let n = n - 46656;
            let suffix = if n <= 35 {
                base(n as u128, alphabet, 1)
            } else if n <= 1295 {
                base(n as u128, alphabet, 2)
            } else if n <= 12959 {
                format!(
                    "{}{}{}",
                    alphabet[(n / 360) as usize] as char,
                    alphabet[((n / 10) % 36) as usize] as char,
                    (b'0' + (n % 10) as u8) as char
                )
            } else {
                return None;
            };
            format!("{call}/{suffix}")
        };
        self.save(&full);
        Some(format!("{full} {power}"))
    }

    pub fn unpack(&mut self, b: &[u8; 77]) -> Option<String> {
        let i3 = bits(b, 74, 3);
        let n3 = bits(b, 71, 3);
        match i3 {
            0 if n3 == 0 => {
                let text = base(
                    bits(b, 0, 71),
                    b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?",
                    13,
                );
                if text.is_empty() { None } else { Some(text) }
            }
            0 if n3 == 5 => Some(format!("{:X}", bits(b, 0, 71))),
            0 if n3 == 1 && bits(b, 0, 28) > 2 && bits(b, 28, 28) > 2 => Some(format!(
                "{} RR73; {} {} {:+03}",
                self.call(bits(b, 0, 28) as u32)?,
                self.call(bits(b, 28, 28) as u32)?,
                self.hash(10, bits(b, 56, 10) as u32),
                2 * bits(b, 66, 5) as i32 - 30
            )),
            0 if n3 == 3 || n3 == 4 => {
                if bits(b, 0, 28) <= 2 || bits(b, 28, 28) <= 2 {
                    return None;
                }
                let a = self.call(bits(b, 0, 28) as u32)?;
                let c = self.call(bits(b, 28, 28) as u32)?;
                let section = bits(b, 64, 7) as usize;
                let section = *crate::message_tables::SECTIONS.get(section.checked_sub(1)?)?;
                let ntx = bits(b, 57, 4) + 1 + if n3 == 4 { 16 } else { 0 };
                let class = (b'A' + bits(b, 61, 3) as u8) as char;
                Some(format!(
                    "{a} {c} {}{ntx}{class} {section}",
                    if b[56] != 0 { "R " } else { "" }
                ))
            }
            0 if n3 == 6 => self.wspr(b),
            1 | 2 => {
                let mut a = self.call(bits(b, 0, 28) as u32)?;
                let mut c = self.call(bits(b, 29, 28) as u32)?;
                let suffix = if i3 == 1 { "/R" } else { "/P" };
                if b[28] != 0 && !a.starts_with('<') {
                    a.push_str(suffix);
                }
                if b[57] != 0 && !c.starts_with('<') {
                    c.push_str(suffix);
                }
                self.save(&c);
                let reply = b[58] != 0;
                let grid = bits(b, 59, 15) as i32;
                let tail = if grid < 32400 {
                    if (a == "CQ" || a.starts_with("CQ ")) && reply {
                        return None;
                    }
                    let g = format!(
                        "{}{}{}{}",
                        (b'A' + (grid / 1800) as u8) as char,
                        (b'A' + ((grid / 100) % 18) as u8) as char,
                        (b'0' + ((grid / 10) % 10) as u8) as char,
                        (b'0' + (grid % 10) as u8) as char
                    );
                    format!("{}{g}", if reply { "R " } else { "" })
                } else {
                    let rpt = grid - 32400;
                    if (a == "CQ" || a.starts_with("CQ ")) && rpt >= 2 {
                        return None;
                    }
                    match rpt {
                        1 => String::new(),
                        2 => "RRR".into(),
                        3 => "RR73".into(),
                        4 => "73".into(),
                        5.. => {
                            let mut snr = rpt - 35;
                            if snr > 50 {
                                snr -= 101;
                            }
                            format!("{}{snr:+03}", if reply { "R" } else { "" })
                        }
                        _ => return None,
                    }
                };
                if a == "CQ" && c.starts_with('<') {
                    return None;
                }
                Some(format!("{a} {c} {tail}").trim().into())
            }
            3 => {
                let a = self.call(bits(b, 1, 28) as u32)?;
                let c = self.call(bits(b, 29, 28) as u32)?;
                let rpt = bits(b, 58, 3) + 2;
                let exchange = bits(b, 61, 13) as usize;
                let exchange = if exchange > 8000 {
                    crate::message_tables::MULTIPLIERS
                        .get(exchange - 8001)?
                        .to_string()
                } else if exchange > 0 && exchange < 8000 {
                    format!("{exchange:04}")
                } else {
                    return None;
                };
                Some(format!(
                    "{}{a} {c} {}5{rpt}9 {exchange}",
                    if b[0] != 0 { "TU; " } else { "" },
                    if b[57] != 0 { "R " } else { "" }
                ))
            }
            5 => {
                let a = self.hash(12, bits(b, 0, 12) as u32);
                let c = self.hash(22, bits(b, 12, 22) as u32);
                let rpt = 52 + bits(b, 35, 3);
                let serial = bits(b, 38, 11);
                let grid = grid6(bits(b, 49, 25) as u32, 24)?;
                Some(format!(
                    "{a} {c} {}{rpt}{serial:04} {grid}",
                    if b[34] != 0 { "R " } else { "" }
                ))
            }
            4 => {
                let hash = self.hash(12, bits(b, 0, 12) as u32);
                let call = base(
                    bits(b, 12, 58),
                    b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/",
                    11,
                );
                self.save(&call);
                let (a, c) = if b[70] == 0 {
                    (hash, call)
                } else {
                    (call, hash)
                };
                if b[73] != 0 {
                    return Some(format!("CQ {c}"));
                }
                let tail = ["", " RRR", " RR73", " 73"][bits(b, 71, 2) as usize];
                Some(format!("{a} {c}{tail}"))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wsjtx_hash_vectors() {
        let mut messages = Messages::default();
        for (call, h10, h12, h22) in [
            ("K1ABC", 712, 2851, 2920267),
            ("W9XYZ", 972, 3889, 3982604),
            ("PJ4/K1ABC", 346, 1387, 1420834),
            ("KH1/KH7Z", 201, 806, 825805),
            ("K1JT/P", 959, 3839, 3931158),
            ("3DA0ABC", 48, 192, 196695),
        ] {
            messages.save(call);
            for (width, hash) in [(10, h10), (12, h12), (22, h22)] {
                assert_eq!(messages.hash(width, hash), format!("<{call}>"));
            }
        }
    }
}
