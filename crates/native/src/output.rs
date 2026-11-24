//! CPAL output resource and sample-clock playback. No exchange policy lives here.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Short-lived approval refreshed by the operator UI. A stalled/closed UI cannot
/// leave audio or PTT authorized indefinitely. Zero always means disarmed.
pub struct Authority {
    epoch: Instant,
    id: AtomicU64,
    expires: AtomicU64,
    generation: AtomicU64,
}
impl Authority {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            id: AtomicU64::new(0),
            expires: AtomicU64::new(0),
            generation: AtomicU64::new(0),
        })
    }
    pub fn approve(&self, id: u64) {
        self.expires.store(
            self.epoch.elapsed().as_millis() as u64 + 250,
            Ordering::Release,
        );
        self.id.store(id, Ordering::Release);
    }
    pub fn revoke(&self) {
        self.id.store(0, Ordering::Release);
    }
    pub fn new_generation(&self) -> u64 {
        self.revoke();
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn allows(&self, id: u64) -> bool {
        id != 0
            && self.id.load(Ordering::Acquire) == id
            && self.epoch.elapsed().as_millis() as u64 <= self.expires.load(Ordering::Acquire)
    }
}

pub fn outputs() -> Result<Vec<String>, String> {
    cpal::default_host()
        .output_devices()
        .map_err(|e| e.to_string())?
        .map(|d| d.name().map_err(|e| e.to_string()))
        .collect()
}

struct Frame {
    id: u64,
    start: Instant,
    samples: Vec<f32>,
    cursor: usize,
}
#[derive(Default)]
struct Playback {
    frame: Option<Frame>,
    started: bool,
    end: Option<Instant>,
    error: Option<String>,
    previous_end: Option<Instant>,
}

pub struct Output {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Playback>>,
    fault: Arc<AtomicBool>,
    pub rate: u32,
}
impl Output {
    pub fn open(name: &str, authority: Arc<Authority>) -> Result<Self, String> {
        // Name selection is exact and unique. Never substitute the default card.
        let mut devices: Vec<_> = cpal::default_host()
            .output_devices()
            .map_err(|e| e.to_string())?
            .filter(|d| d.name().ok().as_deref() == Some(name))
            .collect();
        if devices.len() != 1 {
            return Err("Selected output is missing or ambiguous; refresh and select a unique radio output.".into());
        }
        let device = devices.remove(0);
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = supported.sample_rate().0;
        if !(12000..=192000).contains(&rate) || supported.channels() == 0 {
            return Err("Unsupported output sample rate or channels.".into());
        }
        let shared = Arc::new(Mutex::new(Playback::default()));
        let fault = Arc::new(AtomicBool::new(false));
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                build::<f32>(&device, &config, shared.clone(), fault.clone(), authority)
            }
            cpal::SampleFormat::F64 => {
                build::<f64>(&device, &config, shared.clone(), fault.clone(), authority)
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&device, &config, shared.clone(), fault.clone(), authority)
            }
            cpal::SampleFormat::I32 => {
                build::<i32>(&device, &config, shared.clone(), fault.clone(), authority)
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&device, &config, shared.clone(), fault.clone(), authority)
            }
            format => return Err(format!("Unsupported output format: {format:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _stream: stream,
            shared,
            fault,
            rate,
        })
    }
    pub fn schedule(&mut self, id: u64, start: Instant, samples: Vec<f32>) -> Result<(), String> {
        if self.fault.load(Ordering::Acquire) {
            return Err("Audio output interrupted.".into());
        }
        let mut p = self.shared.lock().map_err(|e| e.to_string())?;
        if p.frame.is_some() {
            return Err("Audio output already has a frame.".into());
        }
        p.frame = Some(Frame {
            id,
            start,
            samples,
            cursor: 0,
        });
        p.started = false;
        p.end = None;
        p.error = None;
        Ok(())
    }
    pub fn progress(&self) -> Result<(bool, Option<Instant>), String> {
        if self.fault.load(Ordering::Acquire) {
            return Err("Audio output interrupted or callback stalled.".into());
        }
        let p = self.shared.lock().map_err(|e| e.to_string())?;
        if let Some(error) = &p.error {
            return Err(error.clone());
        }
        Ok((p.started, p.end))
    }
    pub fn cancel(&mut self) {
        if let Ok(mut p) = self.shared.lock() {
            p.frame = None;
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<Mutex<Playback>>,
    fault: Arc<AtomicBool>,
    authority: Arc<Authority>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    let error_flag = fault.clone();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let zero = T::from_sample(0.0);
                data.fill(zero);
                let Ok(mut p) = shared.lock() else {
                    fault.store(true, Ordering::Release);
                    return;
                };
                let stamp = info.timestamp();
                let begin = Instant::now()
                    + stamp
                        .playback
                        .duration_since(&stamp.callback)
                        .unwrap_or_default();
                render(&mut p, data, channels, rate, begin, &authority);
            },
            move |_| {
                error_flag.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn render<T: cpal::Sample + cpal::FromSample<f32>>(
    p: &mut Playback,
    data: &mut [T],
    channels: usize,
    rate: u32,
    begin: Instant,
    authority: &Authority,
) {
    let callback_end =
        begin + Duration::from_secs_f64((data.len() / channels) as f64 / rate as f64);
    if p.frame.is_some()
        && p.previous_end
            .is_some_and(|last| begin.saturating_duration_since(last) > Duration::from_millis(100))
    {
        p.error = Some("Output clock gap; frame canceled.".into());
        p.frame = None;
    }
    p.previous_end = Some(callback_end);
    let Some(frame) = &mut p.frame else {
        return;
    };
    if !authority.allows(frame.id) {
        p.error = Some("Transmit authorization expired or canceled.".into());
        p.frame = None;
        return;
    }
    if !p.started && begin > frame.start + Duration::from_millis(25) {
        p.error = Some("Output missed the UTC start deadline.".into());
        p.frame = None;
        return;
    }
    for (i, channels) in data.chunks_exact_mut(channels).enumerate() {
        let time = begin + Duration::from_secs_f64(i as f64 / rate as f64);
        if time < frame.start {
            continue;
        }
        if let Some(&sample) = frame.samples.get(frame.cursor) {
            channels.fill(T::from_sample(sample));
            frame.cursor += 1;
            p.started = true;
        } else {
            p.end = Some(time);
            p.frame = None;
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_places_pcm_on_sample_clock_and_cancellation_never_counts_as_completion() {
        let authority = Authority::new();
        authority.approve(7);
        let start = Instant::now() + Duration::from_millis(10);
        let mut p = Playback {
            frame: Some(Frame {
                id: 7,
                start,
                samples: vec![0.25; 120],
                cursor: 0,
            }),
            ..Default::default()
        };
        let mut data = vec![0.0_f32; 720];
        render(
            &mut p,
            &mut data,
            2,
            12000,
            start - Duration::from_millis(10),
            &authority,
        );
        assert!(data[..240].iter().all(|&x| x == 0.0));
        assert!(data[240..480].iter().all(|&x| x == 0.25));
        assert!(p.end.is_some());
        assert!(p.started);
        p.frame = Some(Frame {
            id: 7,
            start,
            samples: vec![0.5; 120],
            cursor: 0,
        });
        p.end = None;
        authority.revoke();
        data.fill(0.0);
        render(&mut p, &mut data, 2, 12000, start, &authority);
        assert!(p.error.is_some());
        assert!(p.end.is_none());
        assert!(data.iter().all(|&x| x == 0.0));
    }
}
