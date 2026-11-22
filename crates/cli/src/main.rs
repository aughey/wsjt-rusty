use std::{
    error::Error,
    io::{self, Read, Write},
    path::Path,
};
use wsjt_rusty::decoder::{
    AudioDecoder, AudioStream, Decode, Ft8Decoder, Mode, Settings, WsprDecoder,
};
fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
fn emit(
    results: &[Decode],
    json: bool,
    ft8_phase: &mut (f64, f64, usize),
) -> Result<(), Box<dyn Error>> {
    let mut out = io::stdout().lock();
    for r in results {
        if r.mode == Mode::Ft8 {
            let angle = std::f64::consts::TAU * r.start_seconds / 15.0;
            ft8_phase.0 += angle.sin();
            ft8_phase.1 += angle.cos();
            ft8_phase.2 += 1;
        }
        if json {
            writeln!(out, "{}", serde_json::to_string(r)?)?;
        } else {
            writeln!(
                out,
                "{:4} {:9.3} {:8.2} {:6.1}  {}",
                r.mode, r.start_seconds, r.frequency_hz, r.snr_db, r.message
            )?;
        }
    }
    out.flush()?;
    Ok(())
}
fn run() -> Result<(), Box<dyn Error>> {
    let mut path = None;
    let mut stdin = false;
    let mut json = false;
    let mut mode = "ft8".to_string();
    let mut passes = None;
    let mut low = None;
    let mut high = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: wsjt-rusty [--mode ft8|wspr|all] [--json] [--passes N]\n                  [--min-hz F --max-hz F] FILE.wav\n       wsjt-rusty [options] --stdin\n\n--stdin reads raw signed 16-bit little-endian mono PCM at 12000 Hz.\nDefault mode: FT8 (100–4000 Hz, 2 passes).\nWSPR defaults: 1390–1610 Hz, 3 passes; WSPR-2 only.\nCustom frequency limits require a single mode. Times are relative to audio start.\nJSON emits one object per transmission; progress goes to stderr."
                );
                return Ok(());
            }
            "--mode" => mode = args.next().ok_or("missing --mode value")?.to_lowercase(),
            "--json" => json = true,
            "--stdin" => stdin = true,
            "--passes" => {
                passes = Some(
                    args.next()
                        .ok_or("missing --passes value")?
                        .parse::<usize>()?,
                )
            }
            "--min-hz" => {
                low = Some(
                    args.next()
                        .ok_or("missing --min-hz value")?
                        .parse::<f32>()?,
                )
            }
            "--max-hz" => {
                high = Some(
                    args.next()
                        .ok_or("missing --max-hz value")?
                        .parse::<f32>()?,
                )
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}").into()),
            _ => {
                if path.replace(arg).is_some() {
                    return Err("expected one WAV path".into());
                }
            }
        }
    }
    let modes = match mode.as_str() {
        "ft8" => vec![Mode::Ft8],
        "wspr" => vec![Mode::Wspr],
        "all" => vec![Mode::Ft8, Mode::Wspr],
        _ => return Err("mode must be ft8, wspr, or all".into()),
    };
    if modes.len() > 1 && (low.is_some() || high.is_some()) {
        return Err("custom frequency limits require a single --mode".into());
    }
    if stdin == path.is_some() {
        return Err("specify exactly one WAV path or --stdin".into());
    }
    let mut decoders: Vec<Box<dyn AudioDecoder>> = Vec::new();
    for mode in modes.iter().copied() {
        let mut settings = Settings::defaults(mode);
        if let Some(p) = passes {
            settings.passes = p;
        }
        if let Some(f) = low {
            settings.low_hz = f;
        }
        if let Some(f) = high {
            settings.high_hz = f;
        }
        decoders.push(match mode {
            Mode::Ft8 => Box::new(Ft8Decoder::new(settings)?),
            Mode::Wspr => Box::new(WsprDecoder::new(settings)?),
        });
    }
    let mut stream = AudioStream::new(decoders)?;
    let mut count = 0usize;
    let mut ft8_phase = (0.0f64, 0.0f64, 0usize);
    let mut samples = 0usize;
    if !json {
        println!("MODE  START(s)    Hz      SNR  MESSAGE");
    }
    if stdin {
        eprintln!("Reading 12000 Hz mono PCM16LE from stdin");
        let mut reader = io::stdin().lock();
        let mut bytes = [0u8; 8192];
        let mut carry = None;
        loop {
            let n = reader.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            let mut audio = Vec::with_capacity(n / 2 + 1);
            let mut start = 0;
            if let Some(first) = carry.take() {
                audio.push(i16::from_le_bytes([first, bytes[0]]) as f32 / 32768.0);
                start = 1;
            }
            let (chunks, remainder) = bytes[start..n].as_chunks::<2>();
            for pair in chunks {
                audio.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0);
            }
            carry = remainder.first().copied();
            samples += audio.len();
            let decoded = stream.push(&audio)?;
            count += decoded.len();
            emit(&decoded, json, &mut ft8_phase)?;
        }
        if carry.is_some() {
            return Err("stdin ended with an incomplete 16-bit PCM sample".into());
        }
    } else {
        let audio = wsjt_audio::read(Path::new(path.as_ref().unwrap()))?;
        samples = audio.len();
        eprintln!(
            "Loaded {:.3}s of audio at 12000 Hz",
            samples as f64 / 12000.0
        );
        let minimum = if modes.contains(&Mode::Ft8) { 13 } else { 108 };
        if samples < minimum * 12000 {
            return Err(format!("selected mode needs at least {minimum} seconds of audio").into());
        }
        let decoded = stream.push(&audio)?;
        count += decoded.len();
        emit(&decoded, json, &mut ft8_phase)?;
    }
    let decoded = stream.finish();
    count += decoded.len();
    emit(&decoded, json, &mut ft8_phase)?;
    if ft8_phase.2 > 0 {
        let phase =
            (ft8_phase.0.atan2(ft8_phase.1) * 15.0 / std::f64::consts::TAU).rem_euclid(15.0);
        eprintln!(
            "FT8 transmission phase: {phase:.3}s modulo 15s; estimated slot boundary {:.3}s (nominal 0.5s TX delay)",
            (phase - 0.5).rem_euclid(15.0)
        );
    }
    eprintln!(
        "Decoded {count} transmissions from {:.3}s of audio",
        samples as f64 / 12000.0
    );
    Ok(())
}
