//! Native resource ownership only: CPAL, bounded PCM delivery, and worker threads.
//! The browser and this runtime call the same Receiver and spectrum algorithms.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver as Inbox, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wsjt_receiver::{
    receiver::{ReceiveRole, Receiver, TimedDecode, Update},
    spectrum::{FFT_SIZE, Spectrum, input_peak},
};

pub const WIDTH: usize = 1000;

pub enum Event {
    Partial(TimedDecode),
    Decoded(ReceiveRole, Update, Duration),
    Spectrum {
        center_ms: f64,
        levels: Vec<f32>,
        peak: f32,
    },
    Error(String),
    Finished,
}

pub fn utc_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}

pub fn devices() -> Result<Vec<cpal::Device>, String> {
    cpal::default_host()
        .input_devices()
        .map(|d| d.collect())
        .map_err(|e| e.to_string())
}

/// One shared credit per mono frame, released only after all lanes consume it.
struct Packet {
    offset: u64,
    origin_ms: f64,
    samples: Vec<f32>,
    pending: Arc<AtomicUsize>,
}
impl Drop for Packet {
    fn drop(&mut self) {
        self.pending
            .fetch_sub(self.samples.len(), Ordering::Relaxed);
    }
}

struct Fanout {
    lanes: Vec<SyncSender<Arc<Packet>>>,
    pending: Arc<AtomicUsize>,
    failed: Arc<AtomicBool>,
    events: SyncSender<Event>,
    limit: usize,
    offset: u64,
    captured: Arc<AtomicU64>,
}
impl Fanout {
    fn fail(&self, message: String) {
        if !self.failed.swap(true, Ordering::Relaxed) {
            let _ = self.events.try_send(Event::Error(message));
        }
    }
    fn push(&mut self, origin_ms: f64, samples: Vec<f32>) -> bool {
        if self.failed.load(Ordering::Relaxed) {
            return false;
        }
        let len = samples.len();
        let previous = self.pending.fetch_add(len, Ordering::Relaxed);
        let packet = Arc::new(Packet {
            offset: self.offset,
            origin_ms,
            samples,
            pending: self.pending.clone(),
        });
        if previous + len > self.limit {
            self.fail("Audio backlog exceeded 60 seconds; input stopped. No samples were silently skipped.".into());
            return false;
        }
        for lane in &self.lanes {
            if lane.try_send(packet.clone()).is_err() {
                self.fail("Receive worker queue unavailable; input stopped.".into());
                return false;
            }
        }
        self.offset += len as u64;
        self.captured.store(self.offset, Ordering::Release);
        true
    }
}

/// Dropping the input closes capture immediately. Workers drain contiguous PCM
/// and flush both tails asynchronously; the UI keeps polling until Finished.
pub struct Session {
    stream: Option<cpal::Stream>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    pub events: Inbox<Event>,
    pub rate: u32,
    priorities: Arc<Mutex<Vec<f32>>>,
    captured: Arc<AtomicU64>,
}
impl Session {
    pub fn live(device: &cpal::Device) -> Result<Self, String> {
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let rate = supported.sample_rate().0;
        if rate < 12000 {
            return Err("Input must supply at least 12000 samples/second.".into());
        }
        let config: cpal::StreamConfig = supported.clone().into();
        if config.channels == 0 {
            return Err("Input has no audio channels.".into());
        }
        let (mut session, fanout) = Self::workers(rate);
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build::<f32>(device, &config, fanout),
            cpal::SampleFormat::F64 => build::<f64>(device, &config, fanout),
            cpal::SampleFormat::I16 => build::<i16>(device, &config, fanout),
            cpal::SampleFormat::I32 => build::<i32>(device, &config, fanout),
            cpal::SampleFormat::U16 => build::<u16>(device, &config, fanout),
            format => return Err(format!("Unsupported native input format: {format:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        session.stream = Some(stream);
        Ok(session)
    }

    /// Paced WAV input exercises the exact live pipeline without a sound device.
    pub fn wav(path: &Path) -> Result<Self, String> {
        let pcm = wsjt_audio::read(path).map_err(|e| e.to_string())?;
        let (session, mut fanout) = Self::workers(12000);
        let stop = session.stop.clone();
        thread::spawn(move || {
            let origin = utc_ms();
            let start = Instant::now();
            for chunk in pcm.chunks(3000) {
                if stop.load(Ordering::Relaxed) || !fanout.push(origin, chunk.to_vec()) {
                    break;
                }
                let deadline = start + Duration::from_secs_f64(fanout.offset as f64 / 12000.0);
                // Check cancellation every 20 ms; closing never waits for a slot.
                while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(20));
                }
            }
        });
        Ok(session)
    }

    fn workers(rate: u32) -> (Self, Fanout) {
        let (events, inbox) = mpsc::sync_channel(1024);
        let failed = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicUsize::new(0));
        let priorities = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::new(AtomicU64::new(0));
        let mut lanes = Vec::new();
        let mut workers = Vec::new();
        for role in [ReceiveRole::Slot, ReceiveRole::Recovery] {
            let (tx, rx) = mpsc::sync_channel::<Arc<Packet>>(65536);
            lanes.push(tx);
            let events = events.clone();
            let failed = failed.clone();
            let priorities = priorities.clone();
            workers.push(thread::spawn(move || {
                let result = decode_lane(rate, role, rx, &events, priorities);
                if let Err(error) = result {
                    failed.store(true, Ordering::Relaxed);
                    let _ = events.send(Event::Error(error));
                }
            }));
        }
        let (tx, rx) = mpsc::sync_channel::<Arc<Packet>>(65536);
        lanes.push(tx);
        let visual_events = events.clone();
        workers.push(thread::spawn(move || visual_lane(rate, rx, visual_events)));
        let finish_events = events.clone();
        thread::spawn(move || {
            for worker in workers {
                if worker.join().is_err() {
                    let _ = finish_events.send(Event::Error("Receive worker panicked.".into()));
                }
            }
            let _ = finish_events.send(Event::Finished);
        });
        (
            Self {
                stream: None,
                stop: Arc::new(AtomicBool::new(false)),
                failed: failed.clone(),
                events: inbox,
                rate,
                priorities,
                captured: captured.clone(),
            },
            Fanout {
                lanes,
                pending,
                failed,
                events,
                limit: rate as usize * 60,
                offset: 0,
                captured,
            },
        )
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.stream.take();
    }
    pub fn set_priorities(&self, frequencies: Vec<f32>) {
        if let Ok(mut current) = self.priorities.lock() {
            *current = frequencies;
        }
    }
    pub fn captured_frames(&self) -> u64 {
        self.captured.load(Ordering::Acquire)
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn decode_lane(
    rate: u32,
    role: ReceiveRole,
    rx: Inbox<Arc<Packet>>,
    events: &SyncSender<Event>,
    priorities: Arc<Mutex<Vec<f32>>>,
) -> Result<(), String> {
    let mut receiver = None;
    for packet in rx {
        if receiver.is_none() {
            let mut r = Receiver::new(rate, packet.origin_ms)?;
            r.set_role(role)?;
            let events = events.clone();
            r.set_result_observer(move |decode| {
                let _ = events.send(Event::Partial(decode.clone()));
            });
            receiver = Some(r);
        }
        let start = Instant::now();
        if let Ok(frequencies) = priorities.lock() {
            receiver
                .as_mut()
                .unwrap()
                .set_priority_frequencies(&frequencies);
        }
        let update = receiver
            .as_mut()
            .unwrap()
            .push(packet.offset, &packet.samples)?;
        events
            .send(Event::Decoded(role, update, start.elapsed()))
            .map_err(|e| e.to_string())?;
    }
    if let Some(mut receiver) = receiver {
        let start = Instant::now();
        let update = receiver.finish()?;
        let _ = events.send(Event::Decoded(role, update, start.elapsed()));
    }
    Ok(())
}

fn visual_lane(rate: u32, rx: Inbox<Arc<Packet>>, events: SyncSender<Event>) {
    let mut spectrum = Spectrum::new(rate as f32, 100.0, 4000.0, WIDTH).unwrap();
    let mut history = VecDeque::with_capacity(FFT_SIZE);
    let hop = (rate / 10) as u64;
    let mut next = FFT_SIZE as u64;
    for packet in rx {
        for (i, &sample) in packet.samples.iter().enumerate() {
            if history.len() == FFT_SIZE {
                history.pop_front();
            }
            history.push_back(sample);
            let end = packet.offset + i as u64 + 1;
            if end >= next && history.len() == FFT_SIZE {
                let samples = history.make_contiguous();
                let mut levels = vec![0.0; WIDTH];
                spectrum.levels(samples, &mut levels).unwrap();
                let center_ms =
                    packet.origin_ms + (end as f64 - FFT_SIZE as f64 / 2.0) / rate as f64 * 1000.0;
                if events
                    .send(Event::Spectrum {
                        center_ms,
                        levels,
                        peak: input_peak(samples),
                    })
                    .is_err()
                {
                    return;
                }
                next = end + hop;
            }
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut fanout: Fanout,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::Sample;
    let channels = config.channels as usize;
    let rate = config.sample_rate.0;
    let mut samples = Vec::with_capacity(rate as usize / 4);
    let mut origin = None;
    let mut first_capture = None;
    let failed = fanout.failed.clone();
    let events = fanout.events.clone();
    device.build_input_stream(config, move |data: &[T], info: &cpal::InputCallbackInfo| {
        if fanout.failed.load(Ordering::Relaxed) { return; }
        let stamp = info.timestamp();
        let capture = *first_capture.get_or_insert(stamp.capture);
        let origin_ms = *origin.get_or_insert_with(|| {
            utc_ms() - stamp.callback.duration_since(&stamp.capture).unwrap_or_default().as_secs_f64() * 1000.0
        });
        let expected = (fanout.offset + samples.len() as u64) as f64 / rate as f64;
        if stamp.capture.duration_since(&capture).is_none_or(|elapsed| (elapsed.as_secs_f64() - expected).abs() > 0.1) {
            fanout.fail("Input clock discontinuity; stopped rather than joining audio across a gap.".into());
            return;
        }
        for frame in data.chunks_exact(channels) {
            let mono = frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / channels as f32;
            samples.push(mono);
        }
        // Native packets follow device callbacks. The sample ledger is the
        // authoritative 60-second limit; queues allow tiny device callbacks.
        if !samples.is_empty() {
            let packet = std::mem::replace(&mut samples, Vec::with_capacity(rate as usize / 4));
            fanout.push(origin_ms, packet);
        }
    }, move |error| {
        if !failed.swap(true, Ordering::Relaxed) {
            let _ = events.try_send(Event::Error(format!("Native audio input failed: {error}")));
        }
    }, None).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wsjt_receiver::history::History;

    #[test]
    fn credits_wait_for_every_lane_and_overrun_stops_explicitly() {
        let (events, errors) = mpsc::sync_channel(8);
        let (a, ar) = mpsc::sync_channel(8);
        let (b, br) = mpsc::sync_channel(8);
        let pending = Arc::new(AtomicUsize::new(0));
        let mut fanout = Fanout {
            lanes: vec![a, b],
            pending: pending.clone(),
            failed: Arc::new(AtomicBool::new(false)),
            events,
            limit: 4,
            offset: 0,
            captured: Arc::new(AtomicU64::new(0)),
        };
        assert!(fanout.push(0.0, vec![0.0; 4]));
        let one = ar.recv().unwrap();
        let two = br.recv().unwrap();
        drop(one);
        assert_eq!(pending.load(Ordering::Relaxed), 4);
        assert!(!fanout.push(0.0, vec![0.0]));
        assert!(matches!(errors.recv().unwrap(), Event::Error(_)));
        assert_eq!(pending.load(Ordering::Relaxed), 4);
        drop(two);
        assert_eq!(pending.load(Ordering::Relaxed), 0);
        assert!(!fanout.push(0.0, vec![0.0]));
    }

    #[test]
    fn stop_releases_paced_source_and_flushes_before_restart() {
        for _ in 0..2 {
            let mut session =
                Session::wav(Path::new("../../tests/fixtures/ft8-210703-133430.wav")).unwrap();
            assert!(matches!(
                session.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                Event::Decoded(..)
            ));
            session.stop();
            let mut finished = [false; 2];
            loop {
                match session.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Event::Decoded(role, update, _) => {
                        finished[usize::from(role == ReceiveRole::Recovery)] |= update.finished;
                    }
                    Event::Error(error) => panic!("{error}"),
                    Event::Finished => break,
                    _ => {}
                }
            }
            assert_eq!(finished, [true, true]);
        }
    }

    #[test]
    fn native_workers_preserve_fixture_payloads_emit_live_and_flush_both_lanes() {
        let mut audio =
            wsjt_audio::read(Path::new("../../tests/fixtures/ft8-210703-133430.wav")).unwrap();
        audio.resize(30 * 12000, 0.0);
        let origin = 1_700_000_010_000.0;
        let mut oracle = Receiver::new(12000, origin).unwrap();
        let mut expected = oracle.push(0, &audio).unwrap().decodes;
        expected.extend(oracle.finish().unwrap().decodes);
        let (session, mut fanout) = Session::workers(12000);
        thread::spawn(move || {
            for chunk in audio.chunks(731) {
                assert!(fanout.push(origin, chunk.to_vec()));
            }
        });
        let mut history = History::default();
        let mut flushed = [false; 2];
        let mut live = false;
        let mut rows = 0;
        loop {
            match session
                .events
                .recv_timeout(Duration::from_secs(60))
                .unwrap()
            {
                Event::Partial(row) => {
                    live = true;
                    history.ingest(origin, vec![row]);
                }
                Event::Decoded(role, update, _) => {
                    flushed[usize::from(role == ReceiveRole::Recovery)] |= update.finished;
                    history.ingest(origin, update.decodes);
                }
                Event::Spectrum { center_ms, .. } => {
                    assert!(
                        (center_ms - origin - (FFT_SIZE as f64 / 2.0 / 12.0 + rows as f64 * 100.0))
                            .abs()
                            < 0.001
                    );
                    rows += 1;
                }
                Event::Error(error) => panic!("{error}"),
                Event::Finished => break,
            }
        }
        assert!(live);
        assert_eq!(flushed, [true, true]);
        assert!(rows > 280);
        for expected in &expected {
            assert!(
                history
                    .decodes
                    .iter()
                    .any(|actual| actual.decode.payload == expected.decode.payload
                        && (actual.start_utc_ms - expected.start_utc_ms).abs() < 350.0
                        && (actual.decode.frequency_hz - expected.decode.frequency_hz).abs() < 4.0),
                "missing {}",
                expected.decode.message
            );
        }
        assert_eq!(history.decodes.len(), expected.len());
        assert!(!history.conversations.items.is_empty());
    }
}
