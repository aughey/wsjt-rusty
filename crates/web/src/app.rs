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
                )
            {
                display.clock.set(Some(frame.clock));
                display.actual_floor.set(frame.floor_db);
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
}

fn utc(ms: f64) -> String {
    js_sys::Date::new(&ms.into())
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
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
    let (hovered, set_hovered) = signal(None::<Row>);
    let display = DisplaySignals {
        floor: floor_db,
        automatic,
        clock: set_waterfall_clock,
        actual_floor: set_actual_floor,
    };
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
    let (status, set_status) =
        signal("Ready. Start listening to allow microphone access.".to_owned());
    let (rate, set_rate) = signal("—".to_owned());
    let (clock, set_clock) = signal(utc(js_sys::Date::now()));
    let (capture_start, set_capture_start) = signal("—".to_owned());
    let (capture_end, set_capture_end) = signal("—".to_owned());
    let (progress, set_progress) = signal("No audio captured".to_owned());
    let (rows, set_rows) = signal(Vec::<Row>::new());
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
        set_decode_activity.set("Waiting for an FT8 decode attempt".into());
        set_decode_started.set(String::new());
        set_rows.set(Vec::new());
        set_hovered.set(None);
        set_waterfall_clock.set(None);
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
                set_rows.update(|rows| {
                    rows.extend(update.decodes);
                    rows.sort_by(|a, b| {
                        b.start_utc_ms
                            .total_cmp(&a.start_utc_ms)
                            .then(a.frequency_hz.total_cmp(&b.frequency_hz))
                    });
                    rows.truncate(500);
                });
            }
            match event.kind.as_str() {
                "decode-started" => {
                    set_decoding.set(true);
                    set_decode_started.set(utc(js_sys::Date::now()));
                    set_decode_activity.set("Decoding FT8…".into());
                }
                "decode-completed" => {
                    set_decoding.set(false);
                    set_decode_activity.set("FT8 decode attempt complete".into());
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
                                    let pointer_row = row.clone();
                                    view! {
                                        <button class="decode-box" type="button"
                                            aria-label=description.clone() title=description aria-describedby="decode-tooltip"
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
                <p>"Audio stays in this browser and is not played through your speakers. Uses the default audio input; localhost or HTTPS required."</p>
                <p>"UTC estimates use your computer’s clock and captured sample counts. Input latency is not calibrated. Transmission end = start + 12.64 s."</p>
                <p>"FT8: 100–4000 audio Hz, two passes. Keep the tab active. Stop releases input and flushes remaining audio."</p>
            </footer>
        </main>
    }
}
