use std::{error::Error, path::Path};
/// Read WAV, average channels, and low-pass resample to WSJT-X's 12 kHz rate.
pub fn read(path: &Path) -> Result<Vec<f32>, Box<dyn Error>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_rate < 12000 || spec.channels == 0 {
        return Err("WAV must have a sample rate of at least 12000 Hz".into());
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = samples
        .chunks_exact(spec.channels as usize)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    Ok(wsjt_dsp::resample_down(&mono, spec.sample_rate, 12000)?)
}
