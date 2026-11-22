#[path = "audio.rs"]
mod audio;

use std::{cell::RefCell, rc::Rc};

use gloo_render::{AnimationFrame, request_animation_frame};
use gloo_timers::callback::Interval;
use leptos::{html::Canvas, prelude::*};
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::conversations::Conversations;
use crate::message_kind::{self, MessageKind};
use crate::waterfall::{self, Waterfall};
use audio::Capture;
use wsjt_rusty::decoder::{Mode, Settings};

#[derive(Default)]
struct Monitor {
    capture: Option<Rc<Capture>>,
    pending: Option<Rc<audio::Cancellation>>,
    frame: Option<AnimationFrame>,
    samples: Vec<f32>,
    waterfall: Option<Waterfall>,
    generation: u64,
    disposed: bool,
    failure: Option<String>,
}

impl Monitor {
    fn stop(&mut self) {
        self.generation += 1;
        self.frame = None;
        self.waterfall = None;
        if let Some(pending) = self.pending.take() {
            pending.cancel();
        }
        if let Some(capture) = self.capture.take() {
            capture.abort();
        }
    }
}

fn draw(canvas: &HtmlCanvasElement, ctx: &CanvasRenderingContext2d, samples: &[f32]) {
    let width = f64::from(canvas.width());
    let height = f64::from(canvas.height());
    ctx.set_fill_style_str("#101c29");
    ctx.fill_rect(0.0, 0.0, width, height);
    ctx.set_stroke_style_str("#293a4c");
    ctx.set_line_width(1.0);
    ctx.begin_path();
    for i in 1..8 {
        let x = width * f64::from(i) / 8.0;
        ctx.move_to(x, 0.0);
        ctx.line_to(x, height);
    }
    for i in 1..4 {
        let y = height * f64::from(i) / 4.0;
        ctx.move_to(0.0, y);
        ctx.line_to(width, y);
    }
    ctx.stroke();
    ctx.set_stroke_style_str("#68dec8");
    ctx.set_line_width(2.0);
    ctx.begin_path();
    if samples.is_empty() {
        ctx.move_to(0.0, height / 2.0);
        ctx.line_to(width, height / 2.0);
    } else {
        for (i, &sample) in samples.iter().enumerate() {
            let x = i as f64 * width / (samples.len() - 1) as f64;
            let y = (0.5 - f64::from(sample.clamp(-1.0, 1.0)) * 0.46) * height;
            if i == 0 {
                ctx.move_to(x, y);
            } else {
                ctx.line_to(x, y);
            }
        }
    }
    ctx.stroke();
}

#[derive(Clone, Copy)]
struct DisplaySignals {
    floor: ReadSignal<f32>,
    automatic: ReadSignal<bool>,
    clock: WriteSignal<Option<(f64, f64)>>,
    actual_floor: WriteSignal<f32>,
    open_frequency: WriteSignal<Option<(f64, f32)>>,
    playing: RwSignal<bool>,
    queue: RwSignal<crate::contact_queue::StationQueue>,
}

fn animate(
    monitor: &Rc<RefCell<Monitor>>,
    canvas: HtmlCanvasElement,
    ctx: CanvasRenderingContext2d,
    display: DisplaySignals,
) {
    // The callback only keeps a Weak reference; dropping the owner cancels the frame.
    let weak = Rc::downgrade(monitor);
    monitor.borrow_mut().frame = Some(request_animation_frame(move |timestamp| {
        let Some(monitor) = weak.upgrade() else {
            return;
        };
        {
            let mut state = monitor.borrow_mut();
            let Monitor {
                capture,
                samples,
                waterfall,
                ..
            } = &mut *state;
            let Some(capture) = capture else { return };
            capture.analyser.get_float_time_domain_data(samples);
            // Keep the small waveform at its original time span; the FFT uses
            // the longer snapshot for enough resolution to distinguish FT8 tones.
            draw(
                &canvas,
                &ctx,
                &samples[samples.len().saturating_sub(2048)..],
            );
            if let Some(waterfall) = waterfall
                && let Ok(Some(frame)) = waterfall.update(
                    timestamp,
                    capture.time_ms(),
                    samples,
                    display.floor.get_untracked(),
                    display.automatic.get_untracked(),
                    display.playing.get_untracked()
                        || display.queue.with_untracked(|q| {
                            q.pending.as_ref().is_some_and(|p| {
                                capture.time_ms() >= p.start_ms
                                    && capture.time_ms() < p.start_ms + 12_640.0
                            })
                        }),
                )
            {
                display.clock.set(Some(frame.clock));
                display.actual_floor.set(frame.floor_db);
                display.open_frequency.set(frame.open_frequency);
            }
        }
        animate(&monitor, canvas, ctx, display);
    }));
}

#[derive(Clone, Deserialize)]
struct Row {
    start_utc_ms: f64,
    end_utc_ms: f64,
    frequency_hz: f32,
    snr_db: f32,
    message: String,
    payload: String,
    #[serde(default)]
    received_at_ms: f64,
}

#[derive(Deserialize)]
struct WorkerUpdate {
    capture_start_ms: f64,
    capture_end_ms: f64,
    received_seconds: f64,
    next_attempt_utc_ms: f64,
    decodes: Vec<Row>,
}

#[derive(Deserialize)]
struct WorkerEvent {
    kind: String,
    error: Option<String>,
    update: Option<WorkerUpdate>,
    decode: Option<Row>,
    decodes: Option<Vec<Row>>,
    attempt: Option<String>,
    elapsed_ms: Option<f64>,
    recovery_end_ms: Option<f64>,
}

fn utc(ms: f64) -> String {
    js_sys::Date::new(&ms.into())
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

// Explicit integer grid coordinates avoid packing gaps left by expired contacts.
fn tile_position(slot: usize) -> String {
    [2, 3, 4, 6, 8, 10]
        .into_iter()
        .map(|columns| {
            format!(
                "--row-{columns}:{};--col-{columns}:{};",
                slot / columns + 1,
                slot % columns + 1
            )
        })
        .collect()
}

#[component]
pub fn App() -> impl IntoView {
    let canvas = NodeRef::<Canvas>::new();
    let waterfall_canvas = NodeRef::<Canvas>::new();
    let passband = Settings::defaults(Mode::Ft8);
    let (low_hz, high_hz) = (passband.low_hz, passband.high_hz);
    let (floor_db, set_floor_db) = signal(-100.0_f32);
    let (automatic, set_automatic) = signal(true);
    let (actual_floor, set_actual_floor) = signal(-100.0_f32);
    let (waterfall_clock, set_waterfall_clock) = signal(None::<(f64, f64)>);
    let (open_frequency, set_open_frequency) = signal(None::<(f64, f32)>);
    let (hovered, set_hovered) = signal(None::<Row>);
    let scale_floor = Signal::derive(move || {
        if automatic.get() {
            actual_floor.get()
        } else {
            floor_db.get()
        }
    });
    let (resolution, set_resolution) = signal("16,384-point Hann FFT".to_owned());
    let (running, set_running) = signal(false);
    let (starting, set_starting) = signal(false);
    let (finishing, set_finishing) = signal(false);
    let (decoding, set_decoding) = signal(false);
    let (decode_activity, set_decode_activity) =
        signal("Waiting for an FT8 decode attempt".to_owned());
    let (decode_started, set_decode_started) = signal(String::new());
    let slot_decoding = RwSignal::new(false);
    let recovery_decoding = RwSignal::new(false);
    let slot_duration = RwSignal::new(None::<f64>);
    let recovery_duration = RwSignal::new(None::<f64>);
    let recovery_lag = RwSignal::new(None::<f64>);
    let (status, set_status) =
        signal("Ready. Start listening to allow microphone access.".to_owned());
    let (rate, set_rate) = signal("—".to_owned());
    let (clock, set_clock) = signal(utc(js_sys::Date::now()));
    let (capture_start, set_capture_start) = signal("—".to_owned());
    let (capture_end, set_capture_end) = signal("—".to_owned());
    let (progress, set_progress) = signal("No audio captured".to_owned());
    let (rows, set_rows) = signal(Vec::<Row>::new());
    let radio_control = crate::radio::RadioControl::new();
    let mut queue_state = crate::contact_queue::StationQueue::default();
    queue_state.log_session = format!("{:.0}-{}", js_sys::Date::now(), js_sys::Math::random());
    let contact_queue = RwSignal::new(queue_state);
    let playing = RwSignal::new(false);
    let display = DisplaySignals {
        floor: floor_db,
        playing,
        queue: contact_queue,
        automatic,
        clock: set_waterfall_clock,
        actual_floor: set_actual_floor,
        open_frequency: set_open_frequency,
    };

    let contacted = RwSignal::new(Vec::<String>::new());
    let contact_history_ready = RwSignal::new(false);
    // Deliberately session-only: never restore or save this opt-in.
    let auto_add_cq = RwSignal::new(false);
    let my_grid = RwSignal::new(crate::transmit_ui::saved_grid());
    let my_call = RwSignal::new(crate::transmit_ui::saved_callsign());
    let (conversations, set_conversations) = signal(Conversations::default());
    let (hovered_contact, set_hovered_contact) = signal(None::<u64>);
    let (focused_contact, set_focused_contact) = signal(None::<u64>);
    let selected_contact = Signal::derive(move || {
        let id = hovered_contact.get().or(focused_contact.get());
        conversations.with(|tracker| {
            tracker
                .items
                .iter()
                .find(|contact| Some(contact.id) == id)
                .cloned()
        })
    });
    let add_cqs = move |fresh: Option<&[Row]>| {
        if !contact_history_ready.get_untracked() {
            return;
        }
        conversations.with_untracked(|t| {
            contact_queue.update(|q| {
                let added = contacted.with_untracked(|calls| {
                    q.add_cq_stations(
                        t.items.iter().filter(|c| {
                            fresh.is_none_or(|rows| {
                                rows.iter().any(|r| {
                                    r.end_utc_ms == c.last_ms
                                        && crate::message_kind::classify(&r.message, &r.payload)
                                            == MessageKind::Cq
                                        && r.message.split_whitespace().any(|word| word == c.caller)
                                })
                            })
                        }),
                        t.now_ms,
                        calls,
                        &my_call.get_untracked(),
                    )
                });
                if added > 0 {
                    rows.with_untracked(|rows| {
                        let mut sorted: Vec<_> = rows.iter().collect();
                        sorted.sort_by(|a, b| a.start_utc_ms.total_cmp(&b.start_utc_ms));
                        for r in sorted {
                            q.hear(
                                &my_call.get_untracked(),
                                r.start_utc_ms,
                                &r.message,
                                &r.payload,
                                r.snr_db,
                                r.frequency_hz,
                            );
                            q.record_arrival(r.start_utc_ms, &r.message, r.received_at_ms);
                        }
                    });
                }
            })
        });
    };
    let ingest = move |incoming: Vec<Row>, processed_ms: Option<f64>| {
        let mut fresh = Vec::new();
        set_rows.update(|history| {
            for mut row in incoming {
                if history.iter().any(|old| {
                    old.payload == row.payload
                        && (old.start_utc_ms - row.start_utc_ms).abs() < 350.0
                        && (old.frequency_hz - row.frequency_hz).abs() < 4.0
                }) {
                    continue;
                }
                row.received_at_ms = js_sys::Date::now();
                fresh.push(row.clone());
                history.push(row);
            }
            history.sort_by(|a, b| {
                b.start_utc_ms
                    .total_cmp(&a.start_utc_ms)
                    .then(a.frequency_hz.total_cmp(&b.frequency_hz))
            });
            history.truncate(500);
        });
        fresh.sort_by(|a, b| a.start_utc_ms.total_cmp(&b.start_utc_ms));
        let clock = processed_ms.unwrap_or_else(|| {
            fresh
                .iter()
                .map(|r| r.end_utc_ms)
                .max_by(f64::total_cmp)
                .unwrap_or_else(|| conversations.with_untracked(|t| t.now_ms))
        });
        set_conversations.update(|t| {
            t.update(
                clock,
                fresh.iter().map(|r| {
                    (
                        r.start_utc_ms,
                        r.end_utc_ms,
                        r.message.as_str(),
                        r.payload.as_str(),
                        r.snr_db,
                    )
                }),
            )
        });
        if auto_add_cq.get_untracked() {
            add_cqs(Some(&fresh));
        }
        contact_queue.update(|q| {
            for r in &fresh {
                q.hear(
                    &my_call.get_untracked(),
                    r.start_utc_ms,
                    &r.message,
                    &r.payload,
                    r.snr_db,
                    r.frequency_hz,
                );
                q.record_arrival(r.start_utc_ms, &r.message, r.received_at_ms);
            }
        });
    };
    // Owner disposal drops the interval and cancels the clock updates.
    let _clock_timer = StoredValue::new_local(Interval::new(1000, move || {
        set_clock.set(utc(js_sys::Date::now()));
    }));
    let monitor = Rc::new(RefCell::new(Monitor::default()));
    let cleanup = StoredValue::new_local(monitor.clone());
    on_cleanup(move || {
        cleanup.with_value(|monitor| {
            let mut state = monitor.borrow_mut();
            state.disposed = true;
            state.stop();
        });
    });

    Effect::new(move |_| {
        if let Some(canvas) = waterfall_canvas.get()
            && let Ok(Some(ctx)) = canvas.get_context("2d")
            && let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>()
        {
            waterfall::clear(&canvas, &ctx);
        }
        if let Some(canvas) = canvas.get()
            && let Ok(Some(ctx)) = canvas.get_context("2d")
            && let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>()
        {
            draw(&canvas, &ctx, &[]);
        }
    });

    let priority_monitor = StoredValue::new_local(monitor.clone());
    let last_priorities = StoredValue::new_local((u64::MAX, Vec::<f32>::new()));
    Effect::new(move |_| {
        let frequencies = contact_queue.with(|q| q.priorities());
        if !running.get() {
            return;
        }
        priority_monitor.with_value(|monitor| {
            let state = monitor.borrow();
            if let Some(capture) = &state.capture {
                let changed = last_priorities.with_value(|(generation, old)| {
                    *generation != state.generation || *old != frequencies
                });
                if changed {
                    capture.set_priorities(&frequencies);
                    last_priorities.set_value((state.generation, frequencies));
                }
            }
        });
    });
    let start_monitor = monitor.clone();
    let start = move |_| {
        let Some(canvas) = canvas.get() else { return };
        let ctx = canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|ctx| ctx.dyn_into::<CanvasRenderingContext2d>().ok());
        let Some(ctx) = ctx else {
            set_status.set("This browser does not provide a 2D canvas.".to_owned());
            return;
        };
        let Some(waterfall_canvas) = waterfall_canvas.get() else {
            return;
        };
        let waterfall_context = waterfall_canvas
            .get_context("2d")
            .ok()
            .flatten()
            .and_then(|ctx| ctx.dyn_into::<CanvasRenderingContext2d>().ok());
        let Some(waterfall_context) = waterfall_context else {
            set_status.set("This browser does not provide a waterfall canvas.".to_owned());
            return;
        };
        start_monitor.borrow_mut().stop();
        start_monitor.borrow_mut().failure = None;
        let generation = start_monitor.borrow().generation;
        let monitor = start_monitor.clone();
        let token = Rc::new(audio::Cancellation::new());
        monitor.borrow_mut().pending = Some(token.clone());
        set_decoding.set(false);
        slot_decoding.set(false);
        recovery_decoding.set(false);
        slot_duration.set(None);
        recovery_duration.set(None);
        recovery_lag.set(None);
        set_decode_activity.set("Waiting for an FT8 decode attempt".into());
        set_decode_started.set(String::new());
        set_rows.set(Vec::new());
        set_conversations.set(Conversations::default());
        contact_queue.update(|q| q.reset_observations());
        set_hovered_contact.set(None);
        set_focused_contact.set(None);
        set_hovered.set(None);
        set_waterfall_clock.set(None);
        set_open_frequency.set(None);
        set_capture_start.set("—".into());
        set_capture_end.set("—".into());
        set_progress.set("Waiting for the first audio chunk".into());
        set_starting.set(true);
        set_status.set("Starting microphone and FT8 worker…".to_owned());
        let weak = Rc::downgrade(&monitor);
        let callback = Closure::new(move |json: String| {
            let Some(monitor) = weak.upgrade() else {
                return;
            };
            if monitor.borrow().disposed || monitor.borrow().generation != generation {
                return;
            }
            let Ok(event) = serde_json::from_str::<WorkerEvent>(&json) else {
                return;
            };
            if let Some(error) = event.error {
                monitor.borrow_mut().failure = Some(error);
            }
            if let Some(decodes) = event.decodes {
                ingest(decodes, None);
            }
            if let Some(end_ms) = event.recovery_end_ms {
                recovery_lag.set(Some(((js_sys::Date::now() - end_ms) / 1000.0).max(0.0)));
            }
            if let Some(decode) = event.decode {
                ingest(vec![decode], None);
            }
            if let Some(update) = event.update {
                set_capture_start.set(utc(update.capture_start_ms));
                set_capture_end.set(utc(update.capture_end_ms));
                let next_attempt = (update.next_attempt_utc_ms - update.capture_end_ms) / 1000.0;
                let lag = ((js_sys::Date::now() - update.capture_end_ms) / 1000.0).max(0.0);
                set_progress.set(if event.kind == "finished" {
                    format!(
                        "{:.1} s processed · final flush complete",
                        update.received_seconds
                    )
                } else {
                    format!(
                        "{:.1} s processed · {:.1} s behind live · next slot attempt in {:.1} s",
                        update.received_seconds, lag, next_attempt
                    )
                });
                ingest(update.decodes, Some(update.capture_end_ms));
            }
            match event.kind.as_str() {
                "decode-started" | "decode-completed" => {
                    let active = event.kind == "decode-started";
                    let recovery = event.attempt.as_deref() == Some("recovery");
                    if recovery {
                        recovery_decoding.set(active);
                    } else {
                        slot_decoding.set(active);
                    }
                    if active {
                        set_decode_started.set(utc(js_sys::Date::now()));
                    } else if let Some(ms) = event.elapsed_ms {
                        if recovery {
                            recovery_duration.set(Some(ms / 1000.0));
                        } else {
                            slot_duration.set(Some(ms / 1000.0));
                        }
                    }
                    let slot = slot_decoding.get_untracked();
                    let recovery = recovery_decoding.get_untracked();
                    set_decoding.set(slot || recovery);
                    set_decode_activity.set(
                        match (slot, recovery) {
                            (true, true) => {
                                "Decoding FT8 · slot and recovery (independent workers)"
                            }
                            (true, false) => "Decoding FT8 · slot response window",
                            (false, true) => "Decoding FT8 · background recovery",
                            (false, false) => "FT8 decoding complete",
                        }
                        .into(),
                    );
                }
                "flushing" => {
                    set_running.set(false);
                    set_finishing.set(true);
                    monitor.borrow_mut().frame = None;
                    let reason =
                        monitor.borrow().failure.clone().unwrap_or_else(|| {
                            "Microphone released. Finishing captured audio…".into()
                        });
                    set_status.set(reason);
                }
                "finished" | "error" => {
                    slot_decoding.set(false);
                    recovery_decoding.set(false);
                    set_decoding.set(false);
                    if event.kind == "error" {
                        set_decode_activity.set("FT8 decoding stopped after an error".into());
                    }
                    set_running.set(false);
                    set_starting.set(false);
                    set_finishing.set(false);
                    monitor.borrow_mut().frame = None;
                    let reason = monitor.borrow().failure.clone().unwrap_or_else(|| {
                        "Stopped. Final FT8 decode complete; microphone released.".into()
                    });
                    set_status.set(reason);
                }
                _ => {}
            }
        });
        spawn_local(async move {
            let result = Capture::open(callback, &token).await;
            if monitor.borrow().disposed || monitor.borrow().generation != generation {
                return;
            }
            monitor.borrow_mut().pending = None;
            set_starting.set(false);
            match result {
                Ok(capture) => {
                    let waterfall = Waterfall::new(
                        waterfall_canvas,
                        waterfall_context,
                        capture.sample_rate(),
                        low_hz,
                        high_hz,
                    );
                    let Ok(waterfall) = waterfall else {
                        set_status.set(
                            "Cannot display the FT8 passband at this input sample rate.".into(),
                        );
                        return;
                    };
                    set_resolution.set(format!(
                        "16,384-point Hann FFT · {:.2} Hz/bin",
                        capture.sample_rate() / crate::spectrum::FFT_SIZE as f32
                    ));
                    set_rate.set(format!("{:.0} Hz → 12000 Hz", capture.sample_rate()));
                    set_status
                        .set("Listening for FT8 · decoding near each UTC slot end".to_owned());
                    let mut state = monitor.borrow_mut();
                    state.samples = vec![0.0; capture.analyser.fft_size() as usize];
                    state.waterfall = Some(waterfall);
                    state.capture = Some(Rc::new(capture));
                    drop(state);
                    set_running.set(true);
                    animate(&monitor, canvas.clone(), ctx, display);
                }
                Err(error) => set_status.set(audio::error_message(error)),
            }
        });
    };

    let stop = move |_| {
        // Keep ownership in Monitor too, so unmount can abort a pending flush.
        let capture = monitor.borrow().capture.clone();
        monitor.borrow_mut().frame = None;
        set_running.set(false);
        if let Some(capture) = capture {
            set_finishing.set(true);
            set_status.set("Stopping input and finishing captured audio…".into());
            spawn_local(async move {
                let _ = capture.finish().await;
                // Drop now aborts an already closed runtime and frees the callback.
            });
        } else {
            // Invalidates an in-flight permission request; late success is dropped.
            monitor.borrow_mut().stop();
            set_starting.set(false);
            set_status.set("Stopped. Microphone request cancelled.".into());
        }
    };

    view! {
        <main>
            <header>
                <span class="brand">"wsjt-rusty"</span>
                <span class="badge">"FT8 · Live receiver"</span>
            </header>
            <h1>"FT8 receiver"</h1>
            <p class="intro">"Decode live audio from your browser’s input device."</p>
            <crate::radio::RadioPanel control=radio_control />
            <p class="clock">"Computer clock (UTC): " {move || clock.get()}</p>
            <section class="monitor" aria-label="Audio input monitor">
                <div class="toolbar">
                    <h2>"FT8 waterfall"</h2>
                    <span class="rate">"Sample rate: " {move || rate.get()}</span>
                </div>
                <div class="controls">
                    <button on:click=start disabled=move || running.get() || starting.get() || finishing.get()>
                        {move || if starting.get() { "Starting…" } else { "Start listening" }}
                    </button>
                    <button class="secondary" on:click=stop disabled=move || !running.get() && !starting.get()>"Stop"</button>
                </div>
                <p class="status" role="status">{move || status.get()}</p>
                <div class="decode-activity" class:decoding=move || decoding.get()>
                    <span class="decode-spinner" aria-hidden="true"></span>
                    <span role="status">{move || decode_activity.get()}</span>
                    <span class="decode-start-time">{move || {
                        let started = decode_started.get();
                        if started.is_empty() { String::new() } else { format!("Started {started}") }
                    }}</span>
                </div>
                <p class="decode-timing">{move || {
                    let slot = slot_duration.get().map(|s| format!("Last slot attempt: {s:.2} s")).unwrap_or_else(|| "Slot attempt: waiting".into());
                    let recovery = recovery_duration.get().map(|s| format!("last recovery: {s:.2} s")).unwrap_or_else(|| "recovery: waiting".into());
                    let lag = recovery_lag.get().map(|s| format!(" · recovery lag at last update: {s:.1} s")).unwrap_or_default();
                    format!("{slot} · {recovery}{lag}")
                }}</p>
                <div class="waterfall-caption">
                    <span>{format!("{low_hz:.0}–{high_hz:.0} audio Hz · decoder passband")}</span>
                    <span>"Newest at top · 60 s history"</span>
                </div>
                <div class="waterfall-layout">
                    <div class="frequency-axis" aria-label="Audio frequency in hertz">
                        {[low_hz, 500.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 3500.0, high_hz]
                            .into_iter().map(|hz| view! {
                                <span class:minor-tick=hz % 1000.0 != 0.0 && hz != low_hz && hz != high_hz
                                    class:first-tick=hz == low_hz class:last-tick=hz == high_hz
                                    style=format!("left: {}%", (hz - low_hz) / (high_hz - low_hz) * 100.0)>
                                    {format!("{hz:.0}")}
                                </span>
                            }).collect_view()}
                    </div>
                    <div class="waterfall-plot">
                        <canvas id="waterfall" node_ref=waterfall_canvas width=waterfall::WIDTH height=waterfall::HEIGHT
                            role="img" aria-label=format!("Live FFT waterfall across {low_hz:.0} to {high_hz:.0} audio hertz. Newest audio at top; 60 seconds of history. Brighter colors indicate stronger signals.")>
                            "Live frequency waterfall. Your browser must support canvas."
                        </canvas>
                        {move || contact_queue.with(|q| q.active.as_ref().map(|a| (a.contact.target.clone(), a.transmit_frequency_hz.or_else(|| q.items.iter().find(|s| s.call == a.contact.target).map(|s| s.frequency_hz))))).and_then(|(call, hz)| hz.map(|hz| view! {
                            <div class="tx-frequency-marker" class:label-right={hz > (low_hz + high_hz) / 2.0} role="img" aria-label=format!("Transmit to {call} at {hz:.1} audio Hz; 50 Hz bandwidth")
                                style=format!("left:{}%;width:{}%", (hz-low_hz)/(high_hz-low_hz)*100.0, 50.0/(high_hz-low_hz)*100.0)>
                                <span>{format!("TX {hz:.1} Hz")}</span>
                            </div>
                        }))}
                        <div class="decode-overlays" aria-label="Decoded FT8 signals">
                            <For each=move || rows.get()
                                key=|row| (row.start_utc_ms.to_bits(), row.frequency_hz.to_bits(), row.payload.clone())
                                children=move |row| {
                                    let start = row.start_utc_ms;
                                    let end = row.end_utc_ms;
                                    let frequency = row.frequency_hz;
                                    let geometry = Signal::derive(move || crate::annotations::bounds(waterfall_clock.get(), low_hz, high_hz, start, end, frequency));
                                    let kind = message_kind::classify(&row.message, &row.payload);
                                    let description = format!("{} · {} · SNR {:+.1} dB · {:.1} Hz", kind.label(), row.message, row.snr_db, frequency);
                                    let marker_row = row.clone();
                                    let marker = Signal::derive(move || contact_queue.with(|q| q.marker_status(&marker_row.message, &marker_row.payload)));
                                    let pointer_row = row.clone();
                                    view! {
                                        <button class="decode-box" type="button"
                                            class:queued=move || marker.get().0
                                            class:pursued=move || marker.get().1
                                            aria-label=move || format!("{}{}", description, if marker.get().1 { " · Actively contacting" } else if marker.get().0 { " · Station queued" } else { "" }) aria-describedby="decode-tooltip"
                                            hidden=move || geometry.get().is_none()
                                            style=move || geometry.get().map(|b| format!("left:{}%;top:{}%;width:{}%;height:{}%;--decode-color:{}", b.left, b.top, b.width, b.height, kind.color())).unwrap_or_default()
                                            on:mouseenter=move |_| set_hovered.set(Some(pointer_row.clone()))
                                            on:mouseleave=move |_| set_hovered.set(None)
                                            on:focus=move |_| set_hovered.set(Some(row.clone()))
                                            on:blur=move |_| set_hovered.set(None)>
                                            <span class="decode-kind-icon" aria-hidden="true">{kind.icon()}</span>
                                        </button>
                                    }
                                }
                            />
                        </div>
                        <Show when=move || hovered.get().is_some_and(|row| crate::annotations::bounds(waterfall_clock.get(), low_hz, high_hz, row.start_utc_ms, row.end_utc_ms, row.frequency_hz).is_some())>
                            <div id="decode-tooltip" class="decode-tooltip" role="tooltip">
                                {move || hovered.get().map(|row| view! {
                                    <span class="tooltip-kind" style=format!("color:{}", message_kind::classify(&row.message, &row.payload).color())>
                                        {format!("{} {}", message_kind::classify(&row.message, &row.payload).icon(), message_kind::classify(&row.message, &row.payload).label())}
                                    </span>
                                    <strong>{row.message}</strong>
                                    <span>{format!("SNR {:+.1} dB · {:.1} audio Hz", row.snr_db, row.frequency_hz)}</span>
                                    <span>{format!("{} – {}", utc(row.start_utc_ms), utc(row.end_utc_ms))}</span>
                                })}
                            </div>
                        </Show>
                    </div>
                    <div class="time-axis" aria-label="Seconds ago">
                        <span>"0 s"</span><span>"15 s"</span><span>"30 s"</span><span>"45 s"</span><span>"60 s"</span>
                    </div>
                </div>
                <div class="message-kind-legend" aria-label="Message type colors and icons">
                    {MessageKind::ALL.into_iter().map(|kind| view! {
                        <span class="message-kind-key" style=format!("--decode-color:{}", kind.color())>
                            <span class="legend-kind-icon" aria-hidden="true">{kind.icon()}</span>{kind.label()}
                        </span>
                    }).collect_view()}
                </div>
                <p class="spectrum-detail">"Thick outline: station queued · Filled, bright outline: actively contacting · Dashed cyan band: TX frequency"</p>
                <div class="waterfall-settings">
                    <label class="auto-level"><input type="checkbox" prop:checked=move || automatic.get()
                        on:change=move |event| set_automatic.set(event_target_checked(&event)) />"Auto level"</label>
                    <label for="waterfall-floor">"New-row floor " {move || format!("{:.0} dBFS", scale_floor.get())}</label>
                    <input id="waterfall-floor" disabled=move || automatic.get() type="range" min="-120" max="-40" step="1" prop:value=move || floor_db.get()
                        on:input=move |event| { if let Ok(value) = event_target_value(&event).parse::<f32>() { set_floor_db.set(value); } } />
                    <div class="color-key" aria-label="FFT level color scale">
                        <span>{move || format!("{:.0}", scale_floor.get())}</span>
                        <span class="color-ramp"></span>
                        <span>{move || format!("{:.0} dBFS", scale_floor.get() + crate::spectrum::DISPLAY_RANGE_DB)}</span>
                    </div>
                </div>
                <p class="spectrum-detail">{move || resolution.get()} " · 10 rows/s · hover or focus a decode box for message and SNR"</p>
                <details class="waveform-panel">
                    <summary>"Input waveform"</summary>
                    <canvas node_ref=canvas width="1200" height="160"
                        role="img" aria-label="Live microphone waveform, amplitude from minus one to plus one">
                        "Live microphone waveform. Your browser must support canvas."
                    </canvas>
                    <div class="axis"><span>"Time →"</span><span>"Amplitude ±1"</span></div>
                </details>
                <p class="progress">{move || progress.get()}</p>
                <dl class="timing">
                    <div><dt>"Capture start (UTC)"</dt><dd>{move || capture_start.get()}</dd></div>
                    <div><dt>{move || if running.get() { "Audio through (UTC)" } else { "Capture end (UTC)" }}</dt><dd>{move || capture_end.get()}</dd></div>
                </dl>
            </section>
            <crate::transmit_ui::StationsToContact queue=contact_queue mine=my_call grid=my_grid radio=radio_control running=running open_frequency=open_frequency playing=playing />
            <crate::contact_map::ContactMap conversations=conversations queue=contact_queue grid=my_grid mine=my_call />
            <section class="results conversation-board" aria-label="FT8 conversations">
                <div class="toolbar"><h2>"Conversations"</h2><span class="rate">{move || format!("{} tracked", conversations.with(|t| t.items.len()))}</span></div>
                <div class="card-sort" aria-label="Add CQ stations">
                    <button class="secondary" disabled=move || !contact_history_ready.get() on:click=move |_| add_cqs(None)>"Add all CQ stations"</button>
                    <label><input type="checkbox" prop:checked=move || auto_add_cq.get()
                        on:change=move |event| auto_add_cq.set(event_target_checked(&event)) />" Automatically add new CQ stations"</label>
                    <span>"Previously contacted stations are excluded. Auto-add starts off on every page load."</span>
                </div>
                <div class="card-sort" aria-label="Sort conversations">
                    <span>"Sort once:"</span>
                    <button class="secondary" on:click=move |_| set_conversations.update(|c| c.sort(crate::geography::CardSort::Signal, &my_grid.get_untracked()))>"Signal ↓"</button>
                    <button class="secondary" on:click=move |_| set_conversations.update(|c| c.sort(crate::geography::CardSort::State, &my_grid.get_untracked()))>"State · CQ first"</button>
                    <button class="secondary" disabled=move || crate::geography::Location::parse(&my_grid.get()).is_none() on:click=move |_| set_conversations.update(|c| c.sort(crate::geography::CardSort::Distance, &my_grid.get_untracked()))>"Distance ↑"</button>
                    <span>"Strongest / CQ first / nearest caller · unknown distances last"</span>
                </div>
                <div class="exchange-state-legend" aria-label="Conversation colors and exchange stage appearance">
                    {[(MessageKind::Cq, "CQ"), (MessageKind::Reply, "Reply"), (MessageKind::Report, "Report"),
                        (MessageKind::RogerReport, "R-report"), (MessageKind::Acknowledgment, "RRR"),
                        (MessageKind::Signoff, "Ended"), (MessageKind::Other, "Stale")].into_iter().map(|(kind, label)| view! {
                            <span class="conversation-color-key" style=format!("--conversation-color:{}", kind.color())>{label}</span>
                        }).collect_view()}

                    <span class="missing-key">"Not received"</span>
                    <span class="upcoming-key">"Ahead / optional"</span>
                    <span class="expected-key">"Next"</span>
                </div>
                <p>"Click a conversation to move its calling station into Stations to contact."</p>
                <p class="empty" hidden=move || !conversations.get().items.is_empty()>"Waiting for FT8 conversations."</p>
                <div class="conversation-list">
                    <For each=move || conversations.with(|t| t.items.iter().filter(|c| !contact_queue.with(|q| q.contains(&c.caller) || c.partner.as_ref().is_some_and(|p| q.contains(p)))).cloned().collect::<Vec<_>>()) key=|contact| contact.id children=move |initial| {
                        let id = initial.id;
                        let contact = Signal::derive(move || conversations.with(|t|
                            t.items.iter().find(|c| c.id == id).cloned()).unwrap_or_else(|| initial.clone()));
                        let status = Signal::derive(move || contact.get().status(conversations.with(|t| t.now_ms)));
                        let previously_contacted = Signal::derive(move || contact.with(|c| {
                            let matches = |call: &str| c.caller == call || c.partner.as_deref() == Some(call);
                            contacted.with(|calls| calls.iter().any(|call| matches(call)))
                                || contact_queue.with(|q| q.was_contacted(&c.caller, &[]) || c.partner.as_ref().is_some_and(|p| q.was_contacted(p, &[])))
                        }));
                        view! {
                            <article class="conversation" tabindex="0" role="button" data-conversation-id=id
                                style=move || contact.with(|c| format!("{}--conversation-color:{};", tile_position(c.display_slot), c.color(conversations.with(|t| t.now_ms))))
                                class:contacted=move || previously_contacted.get()
                                class:inactive=move || matches!(status.get(), "Stale" | "Sign-off heard")
                                aria-label=move || format!("{} · {}. {} {}", contact.get().label(), status.get(), contact.get().expectation(conversations.with(|t| t.now_ms)), if previously_contacted.get() { "Contacted previously; logged software exchange." } else { "" })
                                aria-describedby="conversation-tooltip"
                                on:mouseenter=move |_| set_hovered_contact.set(Some(id))
                                on:mouseleave=move |_| set_hovered_contact.set(None)
                                on:focus=move |_| set_focused_contact.set(Some(id))
                                on:blur=move |_| set_focused_contact.set(None)
                                on:click=move |_| {
                                    let c = contact.get_untracked();
                                    let target = if c.caller == my_call.get_untracked() { c.partner.clone().unwrap_or(c.caller.clone()) } else { c.caller.clone() };
                                    contact_queue.update(|q| {
                                        q.add(target.clone());
                                        if let Some(s) = q.items.iter_mut().find(|s| s.call == target) { s.location = c.locations.get(&target).cloned(); }
                                        q.to_front(&target);
                                        rows.with_untracked(|rows| { let mut sorted: Vec<_> = rows.iter().collect(); sorted.sort_by(|a,b| a.start_utc_ms.total_cmp(&b.start_utc_ms)); for r in sorted { q.hear(&my_call.get_untracked(), r.start_utc_ms, &r.message, &r.payload, r.snr_db, r.frequency_hz); q.record_arrival(r.start_utc_ms, &r.message, r.received_at_ms); } });
                                    });
                                    set_hovered_contact.set(None); set_focused_contact.set(None);
                                }
                                on:keydown=move |event| { if event.key() == "Enter" || event.key() == " " { event.prevent_default(); if let Some(element) = event.current_target().and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) { element.click(); } }
                                    if event.key() == "Escape" { set_hovered_contact.set(None); set_focused_contact.set(None); } }>
                                <div class="conversation-header">
                                    <h3>{move || contact.get().label()}</h3>
                                    <span class="conversation-snr" title="Received SNR of the latest transmission">
                                        {move || contact.with(|c| c.messages.last().map(|heard| format!("{:+.1} dB", heard.snr_db)))}
                                    </span>
                                </div>
                                <crate::exchange_ui::ExchangeStages slots=Signal::derive(move || contact.get().exchange_slots(conversations.with(|t| t.now_ms))) />
                                <span class="conversation-state" title=move || contact.with(|c| crate::geography::description(c.location(), &my_grid.get()))>{move || contact.with(|c| {
                                    let place = c.location().map(|l| crate::geography::Location::parse(&my_grid.get()).map(|home| format!("{} ≈{:.0} km", l.grid, home.distance_km(l))).unwrap_or_else(|| l.grid.clone())).unwrap_or_else(|| "Grid unknown".into());
                                    format!("{}{} · {}", if previously_contacted.get() { "Contacted · " } else { "" }, status.get(), place)
                                })}</span>
                            </article>
                        }
                    } />
                </div>
                <Show when=move || selected_contact.get().is_some()>
                    <div id="conversation-tooltip" class="conversation-tooltip" role="tooltip">
                        {move || selected_contact.get().map(|contact| view! {
                            <strong>{contact.label()}</strong>
                            <span>{contact.status(conversations.with(|t| t.now_ms))}</span>
                            {contact.locations.iter().map(|(call, (_, l))| view! { <p>{format!("{} · {}", call, crate::geography::description(Some(l), &my_grid.get()))}</p> }).collect_view()}
                            <p class="exchange-expectation">{contact.expectation(conversations.with(|t| t.now_ms))}</p>
                            <p class="inference" hidden=!contact.inferred>{contact.inference_note()}</p>
                            <span class="tooltip-evidence-label">"Recent received messages"</span>
                            <ul class="conversation-evidence">{contact.messages.iter().rev().take(6).rev().map(|heard| view! {
                                <li class="message">{heard.message.clone()}</li>
                            }).collect_view()}</ul>
                        })}
                    </div>
                </Show>
            </section>
            <crate::contact_log_ui::ContactLog queue=contact_queue contacted=contacted ready=contact_history_ready />
            <section class="results" aria-label="FT8 decodes">
                <div class="toolbar"><h2>"Decoded messages"</h2><span class="rate">{move || format!("{} shown · latest 500", rows.get().len())}</span></div>
                <p class="empty" hidden=move || !rows.get().is_empty()>"Messages appear after a decode attempt. Tune your receiver to FT8 and route its audio to this input."</p>
                <div class="table-scroll">
                    <table hidden=move || rows.get().is_empty()>
                        <thead><tr><th>"Start UTC"</th><th>"End UTC"</th><th>"Hz"</th><th>"SNR dB"</th><th>"Message"</th></tr></thead>
                        <tbody>
                            {move || rows.get().into_iter().map(|row| view! {
                                <tr title=format!("77-bit payload: {}", row.payload)>
                                    <td>{utc(row.start_utc_ms)}</td><td>{utc(row.end_utc_ms)}</td>
                                    <td>{format!("{:.0}", row.frequency_hz)}</td><td>{format!("{:.1}", row.snr_db)}</td><td class="message">{row.message}</td>
                                </tr>
                            }).collect_view()}
                        </tbody>
                    </table>
                </div>
            </section>
            <footer>
                <p>"Receive audio stays in this browser. Transmit audio follows the station queue while Start contacting is enabled. Uses the default audio input; localhost or HTTPS required."</p>
                <p>"UTC estimates use your computer’s clock and captured sample counts. Input latency is not calibrated. Transmission end = start + 12.64 s."</p>
                <p>"FT8: 100–4000 audio Hz, two passes. Keep the tab active. Stop releases input and flushes remaining audio."</p>
            </footer>
        </main>
    }
}
