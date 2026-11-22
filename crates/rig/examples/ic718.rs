//! cargo run -p wsjt-rig --features native --example ic718 -- /dev/ttyUSB0 19200 [Hz] [USB]
use wsjt_rig::{Command, DEFAULT_ADDRESS, Mode, native::Radio};

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=4).contains(&args.len()) {
        return Err(
            "Usage: ic718 PORT BAUD [FREQUENCY_HZ] [MODE], e.g. COM3 19200 14074000 USB".into(),
        );
    }
    let frequency = args.get(2).map(|s| s.parse::<u64>()).transpose()?;
    let mode = args
        .get(3)
        .map(|s| {
            Mode::ALL
                .into_iter()
                .find(|m| m.to_string() == s.to_uppercase())
                .ok_or("Unsupported mode")
        })
        .transpose()?;
    let mut radio = Radio::open(&args[0], args[1].parse()?, DEFAULT_ADDRESS)?;
    if let Some(hz) = frequency {
        radio.execute(Command::SetFrequency(hz))?;
    }
    if let Some(mode) = mode {
        radio.execute(Command::SetMode(mode))?;
    }
    println!("{:?}", radio.execute(Command::ReadFrequency)?);
    println!("{:?}", radio.execute(Command::ReadMode)?);
    Ok(())
}
