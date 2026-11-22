//! Native serial adapter. No Hamlib, decoder process, or native DSP dependency.
use crate::{BAUD_RATES, Command, Reply, Transaction};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Radio {
    port: Option<Box<dyn serialport::SerialPort>>,
    address: u8,
    transmit_requested: bool,
    ptt_line: Option<PttLine>,
}

impl Radio {
    /// Open 8N1 with no flow control. Match the radio's CI-V baud/address settings.
    pub fn open(path: &str, baud: u32, address: u8) -> Result<Self> {
        Transaction::new(address, Command::ReadFrequency)?;
        if !BAUD_RATES.contains(&baud) {
            return Err("Unsupported IC-718 baud rate".into());
        }
        let port = serialport::new(path, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()?;
        port.clear(serialport::ClearBuffer::Input)?;
        Ok(Self {
            port: Some(port),
            address,
            transmit_requested: false,
            ptt_line: None,
        })
    }

    /// Blocking, bounded transaction. Errors close the port; reopen to recover.
    pub fn execute(&mut self, command: Command) -> Result<Reply> {
        let mut transaction = Transaction::new(self.address, command)?;
        if command == Command::SetTransmit(true) {
            self.transmit_requested = true;
        }
        let result = (|| {
            let port = self
                .port
                .as_mut()
                .ok_or("Radio disconnected; reopen the port")?;
            port.write_all(transaction.request())?;
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut bytes = [0; 64];
            while Instant::now() < deadline {
                match port.read(&mut bytes) {
                    Ok(0) => return Err("Serial port closed".into()),
                    Ok(n) => {
                        if let Some(reply) = transaction.feed(&bytes[..n])? {
                            return Ok(reply);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => (),
                    Err(error) => return Err(error.into()),
                }
            }
            Err("CI-V reply timed out; check cable, baud rate and address".into())
        })();
        if result.is_err() {
            if self.transmit_requested
                && let Some(port) = self.port.as_mut()
            {
                let off = Transaction::new(self.address, Command::SetTransmit(false))?;
                let _ = port.write_all(off.request());
            }
            if let Some(line) = self.ptt_line {
                let _ = self.set_line_ptt(line, false);
            }
            self.port = None;
        } else if command == Command::SetTransmit(false) {
            self.transmit_requested = false;
        }
        result
    }
}

/// A dedicated PTT interface for legacy IC-718s. Uses active-high RTS or DTR.
/// Connect through a PTT interface circuit to ACC SEND, not directly to CI-V.
/// Drop makes a best-effort release. Call `set(false)` explicitly to observe failures.
pub struct Ptt {
    port: Box<dyn serialport::SerialPort>,
    line: PttLine,
}

#[derive(Clone, Copy)]
pub enum PttLine {
    Rts,
    Dtr,
}

impl Ptt {
    pub fn open(path: &str, line: PttLine) -> Result<Self> {
        let port = serialport::new(path, 9600)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()?;
        let mut ptt = Self { port, line };
        ptt.set(false)?;
        Ok(ptt)
    }

    pub fn set(&mut self, on: bool) -> Result<()> {
        match self.line {
            PttLine::Rts => self.port.write_request_to_send(on)?,
            PttLine::Dtr => self.port.write_data_terminal_ready(on)?,
        }
        Ok(())
    }
}
impl Drop for Ptt {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

impl Drop for Radio {
    fn drop(&mut self) {
        if let Some(line) = self.ptt_line {
            let _ = self.set_line_ptt(line, false);
        }
        // Only send the modern CAT release if this session has requested TX.
        if self.transmit_requested
            && let Some(port) = self.port.as_mut()
            && let Ok(t) = Transaction::new(self.address, Command::SetTransmit(false))
        {
            let _ = port.write_all(t.request());
        }
    }
}

impl Radio {
    /// Key a combined CI-V/PTT interface using its active-high RTS or DTR line.
    /// Select the interface's actual PTT line; the other may power the interface.
    pub fn set_line_ptt(&mut self, line: PttLine, on: bool) -> Result<()> {
        if let Some(previous) = self.ptt_line
            && std::mem::discriminant(&previous) != std::mem::discriminant(&line)
        {
            return Err("Release the selected PTT line before changing lines".into());
        }
        self.ptt_line = Some(line); // retain for best-effort release even after an ioctl error
        let port = self.port.as_mut().ok_or("Radio disconnected")?;
        match line {
            PttLine::Rts => port.write_request_to_send(on)?,
            PttLine::Dtr => port.write_data_terminal_ready(on)?,
        }
        if !on {
            self.ptt_line = None;
        }
        Ok(())
    }
}

impl Radio {
    /// Open a combined interface and immediately request receive on its PTT line.
    /// Serial drivers may pulse modem lines while opening; use a suitable interface.
    pub fn open_with_ptt(path: &str, baud: u32, address: u8, line: PttLine) -> Result<Self> {
        let mut radio = Self::open(path, baud, address)?;
        radio.set_line_ptt(line, false)?;
        Ok(radio)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serialport::SerialPort;

    #[test]
    fn native_serial_transaction_and_drop_release() {
        let (mut peer, client) = serialport::TTYPort::pair().unwrap();
        peer.set_timeout(Duration::from_secs(3)).unwrap();
        // Keep the PTY slave alive while the peer reads the final release bytes.
        let _keep_slave_open = client.try_clone().unwrap();
        let mut radio = Radio {
            port: Some(Box::new(client)),
            address: 0x5e,
            transmit_requested: false,
            ptt_line: None,
        };
        let responder = std::thread::spawn(move || {
            let mut request = [0; 6];
            peer.read_exact(&mut request).unwrap();
            assert_eq!(request, [0xfe, 0xfe, 0x5e, 0xe0, 3, 0xfd]);
            peer.write_all(&request).unwrap(); // CI-V adapter echo
            peer.write_all(&[0xfe, 0xfe, 0xe0, 0x5e, 3, 0, 0x40, 7, 0x14, 0, 0xfd])
                .unwrap();
            let mut tx = [0; 8];
            peer.read_exact(&mut tx).unwrap();
            assert_eq!(tx, [0xfe, 0xfe, 0x5e, 0xe0, 0x1c, 1, 1, 0xfd]);
            peer.write_all(&[0xfe, 0xfe, 0xe0, 0x5e, 0xfb, 0xfd])
                .unwrap();
            peer.read_exact(&mut tx).unwrap();
            assert_eq!(tx, [0xfe, 0xfe, 0x5e, 0xe0, 0x1c, 1, 0, 0xfd]);
        });
        assert_eq!(
            radio.execute(Command::ReadFrequency).unwrap(),
            Reply::Frequency(14_074_000)
        );
        assert_eq!(
            radio.execute(Command::SetTransmit(true)).unwrap(),
            Reply::Accepted
        );
        drop(radio);
        responder.join().unwrap();
    }

    #[test]
    fn negative_ack_closes_native_session() {
        let (mut peer, client) = serialport::TTYPort::pair().unwrap();
        peer.set_timeout(Duration::from_secs(3)).unwrap();
        let mut radio = Radio {
            port: Some(Box::new(client)),
            address: 0x5e,
            transmit_requested: false,
            ptt_line: None,
        };
        let responder = std::thread::spawn(move || {
            let mut request = [0; 6];
            peer.read_exact(&mut request).unwrap();
            peer.write_all(&[0xfe, 0xfe, 0xe0, 0x5e, 0xfa, 0xfd])
                .unwrap();
            // Wait for the client to consume the response before the PTY hangs up.
            let mut tail = [0];
            let _ = peer.read(&mut tail);
        });
        assert!(
            radio
                .execute(Command::ReadMode)
                .unwrap_err()
                .to_string()
                .contains("rejected")
        );
        assert!(radio.port.is_none());
        assert!(radio.execute(Command::ReadMode).is_err());
        responder.join().unwrap();
    }
}
