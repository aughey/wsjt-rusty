//! IC-718 CI-V control, independent of audio, UI and transport.
//!
//! Adapts WSJT-X Transceiver/HamlibTransceiver's current-VFO frequency/mode
//! operations. WSJT-X delegates the wire protocol to Hamlib; see docs/rig.md.
//! SPDX-License-Identifier: GPL-3.0-or-later

use std::fmt;

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
pub mod native;

pub const DEFAULT_ADDRESS: u8 = 0x5e;
pub const CONTROLLER_ADDRESS: u8 = 0xe0;
pub const BAUD_RATES: &[u32] = &[300, 1200, 4800, 9600, 19200];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    Lsb = 0,
    Usb = 1,
    Am = 2,
    Cw = 3,
    Rtty = 4,
    CwReverse = 7,
    RttyReverse = 8,
}

impl Mode {
    pub const ALL: [Self; 7] = [
        Self::Lsb,
        Self::Usb,
        Self::Am,
        Self::Cw,
        Self::Rtty,
        Self::CwReverse,
        Self::RttyReverse,
    ];

    pub fn from_byte(value: u8) -> Result<Self, Error> {
        Self::ALL
            .into_iter()
            .find(|mode| *mode as u8 == value)
            .ok_or(Error::InvalidMode)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lsb => "LSB",
            Self::Usb => "USB",
            Self::Am => "AM",
            Self::Cw => "CW",
            Self::Rtty => "RTTY",
            Self::CwReverse => "CW-R",
            Self::RttyReverse => "RTTY-R",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    ReadFrequency,
    ReadMode,
    SetFrequency(u64),
    SetMode(Mode),
    ReadPower,
    SetPower(u8),
    /// Only revisions documenting 1C 01 (IC-718 #82/#83); legacy rigs need RTS/DTR.
    SetTransmit(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    Frequency(u64),
    Mode(Mode),
    Accepted,
    /// Radio setting, 0–255; not measured watts or a calibrated percentage.
    Power(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidAddress,
    InvalidFrequency,
    InvalidMode,
    MalformedReply,
    Rejected,
    Collision,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidAddress => "CI-V address must be 01–7F hex",
            Self::InvalidFrequency => "IC-718 frequency must be 30,000–29,999,999 Hz",
            Self::InvalidMode => "Unsupported IC-718 mode",
            Self::MalformedReply => "Malformed CI-V reply",
            Self::Rejected => "Radio rejected the command",
            Self::Collision => "CI-V bus collision; reconnect before retrying",
        })
    }
}
impl std::error::Error for Error {}

/// One outstanding command. Callers serialize requests and impose a deadline.
/// On timeout, disconnect before another request: CI-V ACKs have no request ID.
/// Feed arbitrary serial chunks; echoes, other radios and broadcasts are ignored.
pub struct Transaction {
    address: u8,
    command: Command,
    request: Vec<u8>,
    buffer: Vec<u8>,
}

impl Transaction {
    pub fn new(address: u8, command: Command) -> Result<Self, Error> {
        if !(1..=0x7f).contains(&address) {
            return Err(Error::InvalidAddress);
        }
        let mut request = vec![0xfe, 0xfe, address, CONTROLLER_ADDRESS];
        match command {
            Command::ReadFrequency => request.push(0x03),
            Command::ReadMode => request.push(0x04),
            Command::ReadPower => request.extend([0x14, 0x0a]),
            Command::SetPower(level) => request.extend([
                0x14,
                0x0a,
                level / 100,
                ((level % 100 / 10) << 4) | (level % 10),
            ]),
            Command::SetTransmit(on) => request.extend([0x1c, 0x01, u8::from(on)]),
            Command::SetMode(mode) => request.extend([0x06, mode as u8]), // omit filter: preserve selection
            Command::SetFrequency(mut hz) => {
                if !(30_000..=29_999_999).contains(&hz) {
                    return Err(Error::InvalidFrequency);
                }
                request.push(0x05);
                for _ in 0..5 {
                    request.push(((hz / 10 % 10) << 4 | (hz % 10)) as u8);
                    hz /= 100;
                }
            }
        }
        request.push(0xfd);
        Ok(Self {
            address,
            command,
            request,
            buffer: Vec::with_capacity(32),
        })
    }

    pub fn request(&self) -> &[u8] {
        &self.request
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Option<Reply>, Error> {
        for &byte in bytes {
            if byte == 0xfc {
                self.buffer.clear();
                return Err(Error::Collision);
            }
            // CI-V frames here are at most 12 bytes. Bound junk/missing terminators.
            if self.buffer.len() >= 64 {
                self.buffer.clear();
            }
            self.buffer.push(byte);
            // Resynchronize on a preamble, allowing extra FE bytes.
            if self.buffer.len() >= 3 && self.buffer.ends_with(&[0xfe, 0xfe]) {
                self.buffer.clear();
                self.buffer.extend([0xfe, 0xfe]);
            }
            if byte != 0xfd {
                continue;
            }
            let frame = std::mem::take(&mut self.buffer);
            if frame.len() < 6 || frame[..4] != [0xfe, 0xfe, CONTROLLER_ADDRESS, self.address] {
                continue;
            }
            let payload = &frame[4..frame.len() - 1];
            if payload == [0xfa] {
                return Err(Error::Rejected);
            }
            let reply = match self.command {
                Command::SetFrequency(_)
                | Command::SetMode(_)
                | Command::SetPower(_)
                | Command::SetTransmit(_)
                    if payload == [0xfb] =>
                {
                    Reply::Accepted
                }
                Command::ReadFrequency if payload[0] == 0x03 => {
                    if payload.len() != 6 {
                        return Err(Error::MalformedReply);
                    }
                    let mut hz = 0u64;
                    for &bcd in payload[1..].iter().rev() {
                        if bcd & 15 > 9 || bcd >> 4 > 9 {
                            return Err(Error::MalformedReply);
                        }
                        hz = hz * 100 + u64::from(bcd >> 4) * 10 + u64::from(bcd & 15);
                    }
                    if !(30_000..=29_999_999).contains(&hz) {
                        return Err(Error::InvalidFrequency);
                    }
                    Reply::Frequency(hz)
                }
                Command::ReadPower if payload.starts_with(&[0x14, 0x0a]) => {
                    if payload.len() != 4
                        || payload[2] > 2
                        || payload[3] >> 4 > 9
                        || payload[3] & 15 > 9
                    {
                        return Err(Error::MalformedReply);
                    }
                    let level = u16::from(payload[2]) * 100
                        + u16::from(payload[3] >> 4) * 10
                        + u16::from(payload[3] & 15);
                    Reply::Power(u8::try_from(level).map_err(|_| Error::MalformedReply)?)
                }
                Command::ReadMode if payload[0] == 0x04 => {
                    if !(2..=3).contains(&payload.len()) {
                        return Err(Error::MalformedReply);
                    }
                    Reply::Mode(Mode::from_byte(payload[1])?)
                }
                _ => continue,
            };
            return Ok(Some(reply));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_frequency_vector_and_fragmented_echo() {
        // 14.074 MHz, five little-endian packed BCD bytes, not a generated waveform.
        let mut t = Transaction::new(0x5e, Command::SetFrequency(14_074_000)).unwrap();
        assert_eq!(
            t.request(),
            &[
                0xfe, 0xfe, 0x5e, 0xe0, 0x05, 0x00, 0x40, 0x07, 0x14, 0x00, 0xfd
            ]
        );
        let echo = t.request().to_vec();
        assert_eq!(t.feed(&echo), Ok(None));
        assert_eq!(t.feed(&[0xfe, 0xfe, 0xe0]), Ok(None));
        assert_eq!(t.feed(&[0x5e, 0xfb, 0xfd]), Ok(Some(Reply::Accepted)));
        let mut t = Transaction::new(0x5e, Command::ReadFrequency).unwrap();
        let frame = [
            0xfe, 0xfe, 0xe0, 0x5e, 0x03, 0x00, 0x40, 0x07, 0x14, 0x00, 0xfd,
        ];
        for byte in &frame[..10] {
            assert_eq!(t.feed(&[*byte]), Ok(None));
        }
        assert_eq!(t.feed(&frame[10..]), Ok(Some(Reply::Frequency(14_074_000))));
    }

    #[test]
    fn ignores_noise_broadcast_other_radio_and_unrelated_replies() {
        let mut t = Transaction::new(0x5e, Command::ReadMode).unwrap();
        assert_eq!(t.feed(&[42; 1000]), Ok(None));
        for frame in [
            &[0xfe, 0xfe, 0, 0x5e, 1, 1, 0xfd][..],
            &[0xfe, 0xfe, 0xe0, 0x60, 4, 1, 0xfd],
            &[0xfe, 0xfe, 0xe0, 0x5e, 0xfb, 0xfd],
        ] {
            assert_eq!(t.feed(frame), Ok(None));
        }
        assert_eq!(
            t.feed(&[0xfe, 0xfe, 0xfe, 0xe0, 0x5e, 4, 1, 1, 0xfd]),
            Ok(Some(Reply::Mode(Mode::Usb)))
        );
    }

    #[test]
    fn transmitter_vectors_and_power_validation() {
        assert_eq!(
            Transaction::new(0x5e, Command::SetPower(255))
                .unwrap()
                .request(),
            &[0xfe, 0xfe, 0x5e, 0xe0, 0x14, 0x0a, 0x02, 0x55, 0xfd]
        );
        assert_eq!(
            Transaction::new(0x5e, Command::SetTransmit(true))
                .unwrap()
                .request(),
            &[0xfe, 0xfe, 0x5e, 0xe0, 0x1c, 1, 1, 0xfd]
        );
        assert_eq!(
            Transaction::new(0x5e, Command::SetTransmit(false))
                .unwrap()
                .request(),
            &[0xfe, 0xfe, 0x5e, 0xe0, 0x1c, 1, 0, 0xfd]
        );
        let mut t = Transaction::new(0x5e, Command::ReadPower).unwrap();
        assert_eq!(
            t.feed(&[0xfe, 0xfe, 0xe0, 0x5e, 0x14, 0x0a, 2, 0x55, 0xfd]),
            Ok(Some(Reply::Power(255)))
        );
        for data in [[2, 0x56], [3, 0], [0, 0xaa]] {
            let mut t = Transaction::new(0x5e, Command::ReadPower).unwrap();
            let mut bytes = vec![0xfe, 0xfe, 0xe0, 0x5e, 0x14, 0x0a];
            bytes.extend(data);
            bytes.push(0xfd);
            assert_eq!(t.feed(&bytes), Err(Error::MalformedReply));
        }
    }

    #[test]
    fn rejects_errors_and_invalid_data() {
        assert!(Transaction::new(0xe0, Command::ReadMode).is_err());
        for hz in [0, 29_999, 30_000_000, u64::MAX] {
            assert!(Transaction::new(0x5e, Command::SetFrequency(hz)).is_err());
        }
        for (bytes, error) in [
            (vec![0xfe, 0xfe, 0xe0, 0x5e, 0xfa, 0xfd], Error::Rejected),
            (vec![0xfc], Error::Collision),
            (
                vec![0xfe, 0xfe, 0xe0, 0x5e, 3, 0xaa, 0, 0, 0, 0, 0xfd],
                Error::MalformedReply,
            ),
            (
                vec![0xfe, 0xfe, 0xe0, 0x5e, 3, 0, 0xfd],
                Error::MalformedReply,
            ),
        ] {
            assert_eq!(
                Transaction::new(0x5e, Command::ReadFrequency)
                    .unwrap()
                    .feed(&bytes),
                Err(error)
            );
        }
        assert_eq!(
            Transaction::new(0x5e, Command::SetMode(Mode::Usb))
                .unwrap()
                .request(),
            &[0xfe, 0xfe, 0x5e, 0xe0, 6, 1, 0xfd]
        );
        assert_eq!(
            Transaction::new(0x5e, Command::ReadMode)
                .unwrap()
                .feed(&[0xfe, 0xfe, 0xe0, 0x5e, 4, 5, 0xfd]),
            Err(Error::InvalidMode)
        );
    }
}
