// SPDX-License-Identifier: GPL-3.0-or-later
// Port of WSJT-X wsprd_utils.c and nhash.c (Taylor/Franke; Jenkins hash).
use std::collections::HashMap;
#[derive(Default)]
pub struct Messages {
    calls: HashMap<u16, String>,
}
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub callsign: String,
    pub locator: Option<String>,
    pub power_dbm: u8,
    pub message_type: u8,
}
fn raw_call(mut n: u32) -> Option<[u8; 6]> {
    if n >= 262177560 {
        return None;
    }
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ ";
    let mut call = [b' '; 6];
    for i in (3..6).rev() {
        call[i] = alphabet[(n % 27 + 10) as usize];
        n /= 27;
    }
    call[2] = alphabet[(n % 10) as usize];
    n /= 10;
    call[1] = alphabet[(n % 36) as usize];
    n /= 36;
    call[0] = *alphabet.get(n as usize)?;
    Some(call)
}
fn grid(n: u32) -> Option<String> {
    if n >= 32400 {
        return None;
    }
    let lon = 179 - n / 180;
    let lat = n % 180;
    Some(format!(
        "{}{}{}{}",
        (b'A' + (lon / 10) as u8) as char,
        (b'A' + (lat / 10) as u8) as char,
        (b'0' + (lon % 10) as u8) as char,
        (b'0' + (lat % 10) as u8) as char
    ))
}
fn prefix(n: u32, call: &str) -> Option<String> {
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ ";
    if n < 60000 {
        let mut n = n;
        let mut chars = [b' '; 3];
        for c in chars.iter_mut().rev() {
            *c = alphabet[(n % 37) as usize];
            n /= 37;
        }
        Some(format!(
            "{}/{call}",
            String::from_utf8(chars.to_vec()).ok()?.trim()
        ))
    } else {
        let n = n - 60000;
        let suffix = match n {
            0..=35 => (alphabet[n as usize] as char).to_string(),
            36..=125 => format!("{}{}", (n - 26) / 10, (n - 26) % 10),
            _ => return None,
        };
        Some(format!("{call}/{suffix}"))
    }
}
// Bob Jenkins' hashlittle, with wsprd's seed 146 and 15-bit output.
pub fn hash(call: &str) -> u16 {
    let mut bytes = call.as_bytes();
    let initial = 0xdeadbeefu32
        .wrapping_add(bytes.len() as u32)
        .wrapping_add(146);
    let (mut a, mut b, mut c) = (initial, initial, initial);
    while bytes.len() > 12 {
        a = a.wrapping_add(u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        b = b.wrapping_add(u32::from_le_bytes(bytes[4..8].try_into().unwrap()));
        c = c.wrapping_add(u32::from_le_bytes(bytes[8..12].try_into().unwrap()));
        a = a.wrapping_sub(c);
        a ^= c.rotate_left(4);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= a.rotate_left(6);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= b.rotate_left(8);
        b = b.wrapping_add(a);
        a = a.wrapping_sub(c);
        a ^= c.rotate_left(16);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= a.rotate_left(19);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= b.rotate_left(4);
        b = b.wrapping_add(a);
        bytes = &bytes[12..];
    }
    for (i, &byte) in bytes.iter().enumerate() {
        let word = (byte as u32) << ((i % 4) * 8);
        match i / 4 {
            0 => a = a.wrapping_add(word),
            1 => b = b.wrapping_add(word),
            _ => c = c.wrapping_add(word),
        }
    }
    if !bytes.is_empty() {
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(14));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(11));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(25));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(16));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(4));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(14));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(24));
    }
    (c & 32767) as u16
}
impl Messages {
    pub fn unpack(&mut self, payload: &[u8; 50]) -> Option<Message> {
        let n1 = payload[..28].iter().fold(0u32, |n, &b| (n << 1) | b as u32);
        let n2 = payload[28..].iter().fold(0u32, |n, &b| (n << 1) | b as u32);
        let raw = raw_call(n1)?;
        let kind = (n2 & 127) as i32 - 64;
        if kind >= 0 {
            if kind > 62 {
                return None;
            }
            let call = String::from_utf8(raw.to_vec()).ok()?.trim().to_string();
            let digit = call.bytes().rposition(|c| c.is_ascii_digit())?;
            if !(1..=2).contains(&digit)
                || call.contains(' ')
                || !call[..digit].bytes().any(|c| c.is_ascii_uppercase())
                || !call[digit + 1..].bytes().all(|c| c.is_ascii_uppercase())
            {
                return None;
            }
            let nu = kind % 10;
            let (call, locator, power, message_type) = if [0, 3, 7].contains(&nu) {
                (call, Some(grid(n2 >> 7)?), kind as u8, 1)
            } else {
                let add = if nu > 7 {
                    nu - 7
                } else if nu > 3 {
                    nu - 3
                } else {
                    nu
                };
                let n = (n2 >> 7) + 32768 * (add as u32 - 1);
                (prefix(n, &call)?, None, (kind - add) as u8, 2)
            };
            if power > 60 {
                return None;
            }
            self.calls.insert(hash(&call), call.clone());
            let text = if let Some(grid) = &locator {
                format!("{call} {grid} {power}")
            } else {
                format!("{call} {power}")
            };
            Some(Message {
                text,
                callsign: call,
                locator,
                power_dbm: power,
                message_type,
            })
        } else {
            let power = -(kind + 1);
            if power > 60 || ![0, 3, 7].contains(&(power % 10)) {
                return None;
            }
            let rotated = [raw[5], raw[0], raw[1], raw[2], raw[3], raw[4]];
            if !(b'A'..=b'R').contains(&rotated[0])
                || !(b'A'..=b'R').contains(&rotated[1])
                || !rotated[2].is_ascii_digit()
                || !rotated[3].is_ascii_digit()
            {
                return None;
            }
            let grid = String::from_utf8(rotated.to_vec()).ok()?.trim().to_string();
            let call = self
                .calls
                .get(&((n2 >> 7) as u16))
                .map(|c| format!("<{c}>"))
                .unwrap_or_else(|| "<...>".into());
            Some(Message {
                text: format!("{call} {grid} {power}"),
                callsign: call,
                locator: Some(grid),
                power_dbm: power as u8,
                message_type: 3,
            })
        }
    }
}
