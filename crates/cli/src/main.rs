use anyhow::{Context, bail};
use clap::{ArgGroup, Parser, ValueEnum};
use std::{
    error::Error,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use wsjt_rusty::decoder::{
    AudioDecoder, AudioStream, Decode, Ft8Decoder, Mode, SAMPLE_RATE, Settings, WsprDecoder,
};

const AFTER_HELP: &str = "\
--stdin reads raw signed 16-bit little-endian mono PCM at 12000 Hz.
Default mode: FT8 (100–4000 Hz, 2 passes).
WSPR defaults: 1390–1610 Hz, 3 passes; WSPR-2 only.
Custom frequency limits require a single mode. Times are relative to audio start.
JSON emits one object per transmission; progress goes to stderr.
--realtime requires --stdin and FT8. It buffers audio and decodes the latest
16 seconds at each UTC slot + 13.5 s. Start times stay relative to the first
sample. The producer must buffer live audio while a decode is running.";

/// Decode FT8 and WSPR-2 from a WAV file or raw 12 kHz PCM.
#[derive(Parser)]
#[command(
    about,
    after_help = AFTER_HELP,
    group(
        ArgGroup::new("input")
            .args(["stdin", "path"])
            .required(true)
            .multiple(false)
    )
)]
struct Args {
    /// Protocol to decode. `all` runs FT8 and WSPR on the same audio.
    #[arg(long, value_enum, ignore_case = true, default_value = "ft8")]
    mode: CliMode,
    /// Emit one JSON object per transmission.
    #[arg(long)]
    json: bool,
    /// Read raw signed 16-bit little-endian mono PCM at 12000 Hz from stdin.
    #[arg(long)]
    stdin: bool,
    /// Buffer --stdin and decode FT8 at each UTC slot + 13.5 seconds.
    #[arg(long, requires = "stdin")]
    realtime: bool,
    /// Decode and subtraction passes. Defaults: FT8 2, WSPR 3.
    #[arg(long, value_name = "N")]
    passes: Option<usize>,
    /// Lowest audio frequency to search, in hertz.
    #[arg(long, value_name = "HZ")]
    min_hz: Option<f32>,
    /// Highest audio frequency to search, in hertz.
    #[arg(long, value_name = "HZ")]
    max_hz: Option<f32>,
    /// WAV file to decode.
    #[arg(value_name = "FILE")]
    path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum CliMode {
    Ft8,
    Wspr,
    All,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn emit(
    results: &[Decode],
    json: bool,
    ft8_phase: &mut (f64, f64, usize),
) -> Result<(), anyhow::Error> {
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
                "{:4} {:9.3} {:8.2} {:6.1} {} {}",
                r.mode, r.start_seconds, r.frequency_hz, r.snr_db, r.frequency_reference, r.message
            )?;
        }
    }
    out.flush()?;
    Ok(())
}

fn run() -> Result<(), anyhow::Error> {
    let args = Args::parse();
    if args.realtime && args.mode != CliMode::Ft8 {
        bail!("--realtime follows FT8 UTC slots; use --mode ft8");
    }
    let modes = match args.mode {
        CliMode::Ft8 => vec![Mode::Ft8],
        CliMode::Wspr => vec![Mode::Wspr],
        CliMode::All => vec![Mode::Ft8, Mode::Wspr],
    };
    if modes.len() > 1 && (args.min_hz.is_some() || args.max_hz.is_some()) {
        bail!("custom frequency limits require a single --mode");
    }
    let mut decoders: Vec<Box<dyn AudioDecoder>> = Vec::new();
    for mode in modes.iter().copied() {
        let mut settings = Settings::defaults(mode);
        if let Some(passes) = args.passes {
            settings.passes = passes;
        }
        if let Some(low_hz) = args.min_hz {
            settings.low_hz = low_hz;
        }
        if let Some(high_hz) = args.max_hz {
            settings.high_hz = high_hz;
        }
        decoders.push(match mode {
            Mode::Ft8 => Box::new(Ft8Decoder::new(settings).map_err(decode_err)?),
            Mode::Wspr => Box::new(WsprDecoder::new(settings).map_err(decode_err)?),
        });
    }
    let mut stream = AudioStream::new(decoders).map_err(decode_err)?;
    let mut count = 0usize;
    let mut ft8_phase = (0.0f64, 0.0f64, 0usize);
    let mut samples = 0usize;
    if !args.json {
        println!("MODE  START(s)    Hz      SNR  MESSAGE");
    }
    if args.stdin {
        eprintln!("Reading 12000 Hz mono PCM16LE from stdin");
        if args.realtime {
            eprintln!("Realtime FT8: decoding the latest 16s at each UTC slot + 13.5s");
        }
        let mut reader = io::stdin().lock();
        let mut bytes = [0u8; 8192];
        let mut carry = None;
        let mut next_attempt_ms = args.realtime.then(|| next_ft8_attempt_ms(utc_now_ms()));
        loop {
            let n = reader.read(&mut bytes)?;
            eprintln!("Read {n} bytes");
            if n == 0 {
                break;
            }
            let mut audio = Vec::with_capacity(n / 2 + 1);
            append_pcm16(&bytes[..n], &mut carry, &mut audio);
            samples += audio.len();
            if let Some(due_ms) = next_attempt_ms {
                stream.push_buffered(&audio).map_err(decode_err)?;
                if utc_now_ms() >= due_ms {
                    count += emit_realtime_attempt(&mut stream, args.json, &mut ft8_phase, due_ms)?;
                    next_attempt_ms = Some(next_ft8_attempt_ms(utc_now_ms()));
                }
            } else {
                let decoded = stream.push(&audio).map_err(decode_err)?;
                count += decoded.len();
                emit(&decoded, args.json, &mut ft8_phase)?;
            }
        }
        if carry.is_some() {
            bail!("stdin ended with an incomplete 16-bit PCM sample");
        }
    } else {
        let path = args
            .path
            .as_deref()
            .context("specify exactly one WAV path or --stdin")?;
        let audio = read_wav(path)?;
        samples = audio.len();
        eprintln!(
            "Loaded {:.3}s of audio at 12000 Hz",
            samples as f64 / 12000.0
        );
        let minimum = if modes.contains(&Mode::Ft8) { 13 } else { 108 };
        if samples < minimum * 12000 {
            bail!("selected mode needs at least {minimum} seconds of audio");
        }
        let decoded = stream.push(&audio).map_err(decode_err)?;
        count += decoded.len();
        emit(&decoded, args.json, &mut ft8_phase)?;
    }
    let decoded = stream.finish();
    count += decoded.len();
    emit(&decoded, args.json, &mut ft8_phase)?;
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

fn read_wav(path: &Path) -> Result<Vec<f32>, anyhow::Error> {
    wsjt_audio::read(path)
        .map_err(decode_err)
        .with_context(|| format!("reading {}", path.display()))
}

fn decode_err(err: Box<dyn Error>) -> anyhow::Error {
    anyhow::anyhow!("{err}")
}

fn append_pcm16(bytes: &[u8], carry: &mut Option<u8>, audio: &mut Vec<f32>) {
    let mut start = 0;
    if let Some(first) = carry.take() {
        if bytes.is_empty() {
            *carry = Some(first);
            return;
        }
        audio.push(i16::from_le_bytes([first, bytes[0]]) as f32 / 32768.0);
        start = 1;
    }
    let (chunks, remainder) = bytes[start..].as_chunks::<2>();
    for pair in chunks {
        audio.push(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0);
    }
    *carry = remainder.first().copied();
}

/// Next UTC instant strictly after `now_ms` at which an FT8 slot is 13.5 s old.
/// The schedule is 13.5, 28.5, 43.5, ... seconds within successive 15 s slots.
fn next_ft8_attempt_ms(now_ms: i64) -> i64 {
    (now_ms - 13_500).div_euclid(15_000) * 15_000 + 28_500
}

fn utc_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn format_utc_hms(utc_ms: i64) -> String {
    let seconds = utc_ms.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

fn emit_realtime_attempt(
    stream: &mut AudioStream,
    json: bool,
    ft8_phase: &mut (f64, f64, usize),
    due_ms: i64,
) -> Result<usize, anyhow::Error> {
    eprintln!(
        "FT8 realtime decode for UTC slot {}",
        format_utc_hms(due_ms - 13_500)
    );
    let mut emit_error = None;
    let mut emitted = 0usize;
    stream.decode_pending_with_observer(16 * SAMPLE_RATE, &mut |decode| {
        if emit_error.is_some() {
            return;
        }
        match emit(std::slice::from_ref(decode), json, ft8_phase) {
            Ok(()) => emitted += 1,
            Err(err) => emit_error = Some(err),
        }
    });
    emit_error.map_or(Ok(emitted), Err)
}

#[cfg(test)]
mod tests {
    use super::next_ft8_attempt_ms;

    #[test]
    fn ft8_attempts_follow_utc_slot_plus_13_5_seconds() {
        assert_eq!(next_ft8_attempt_ms(0), 13_500);
        assert_eq!(next_ft8_attempt_ms(13_499), 13_500);
        assert_eq!(next_ft8_attempt_ms(13_500), 28_500);
        assert_eq!(next_ft8_attempt_ms(14_000), 28_500);
        assert_eq!(next_ft8_attempt_ms(28_500), 43_500);
        // 1_700_000_000_000 ms is 5.0 s into a slot, so the attempt is 8.5 s later.
        assert_eq!(next_ft8_attempt_ms(1_700_000_000_000), 1_700_000_008_500);
    }
}
