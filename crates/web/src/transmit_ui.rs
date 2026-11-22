//! CQ-triggered station queue, saved station settings, and output lifecycle.
use crate::{contact_queue::StationQueue, radio::RadioControl};
use gloo_timers::callback::Interval;
use leptos::prelude::*;
use serde::Deserialize;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, future_to_promise, spawn_local};
use wsjt_rusty::ft8::transmit;

#[wasm_bindgen(module = "/src/transmit-runtime.js")]
extern "C" {
    #[wasm_bindgen(js_name = createTransmitter)]
    fn create_transmitter(
        key: &js_sys::Function,
        release: &js_sys::Function,
        available: &js_sys::Function,
    ) -> JsValue;
    #[wasm_bindgen(js_name = stopTransmitter)]
    fn stop_transmitter(tx: &JsValue) -> js_sys::Promise;
    #[wasm_bindgen(js_name = disposeTransmitter)]
    fn dispose_transmitter(tx: &JsValue) -> js_sys::Promise;
    #[wasm_bindgen(js_name = transmitterBusy)]
    fn transmitter_busy(tx: &JsValue) -> bool;
    #[wasm_bindgen(js_name = transmitterPlaying)]
    fn transmitter_playing(tx: &JsValue) -> bool;
    #[wasm_bindgen(js_name = prepareTransmitter)]
    fn prepare_transmitter(tx: &JsValue, output_id: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = cancelQueuedTransmission)]
    fn cancel_current(tx: &JsValue) -> js_sys::Promise;
    #[wasm_bindgen(js_name = transmitterOutcome)]
    fn transmitter_outcome(tx: &JsValue) -> String;
    #[wasm_bindgen(js_name = transmitterPrepared)]
    fn transmitter_prepared(tx: &JsValue) -> bool;
    #[wasm_bindgen(js_name = transmitterPhase)]
    fn transmitter_phase(tx: &JsValue) -> String;
    #[wasm_bindgen(js_name = transmitterStatus)]
    fn transmitter_status(tx: &JsValue) -> String;
    #[wasm_bindgen(js_name = loadTransmitPreferences)]
    fn load_preferences() -> String;
    #[wasm_bindgen(js_name = saveTransmitPreferences)]
    fn save_preferences(
        callsign: &str,
        grid: &str,
        output_id: &str,
        output_label: &str,
        attenuation_db: f32,
        auto_remove: bool,
        cq_timeout_seconds: u32,
        sort_state: bool,
        sort_signal: bool,
        sort_distance: bool,
        find_open_frequency: bool,
    ) -> String;
    #[wasm_bindgen(js_name = setTransmitAttenuation)]
    fn set_attenuation(tx: &JsValue, attenuation_db: f32);
    #[wasm_bindgen(js_name = listAudioOutputs)]
    fn list_outputs(permission: bool) -> js_sys::Promise;
    #[wasm_bindgen(js_name = outputSelectionSupported)]
    fn output_supported() -> bool;
    #[wasm_bindgen(js_name = playTransmission)]
    fn play_transmission(
        tx: &JsValue,
        samples: &[f32],
        receive_slot: f64,
        output_id: &str,
        use_radio: bool,
        start_ms: f64,
    ) -> js_sys::Promise;
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Preferences {
    callsign: String,
    grid: String,
    output_id: String,
    output_label: String,
    attenuation_db: f32,
    #[serde(default = "default_auto_remove")]
    auto_remove: bool,
    #[serde(default = "default_cq_timeout")]
    cq_timeout_seconds: u32,
    sort_state: bool,
    sort_signal: bool,
    sort_distance: bool,
    find_open_frequency: bool,
}
fn default_auto_remove() -> bool {
    true
}
fn default_cq_timeout() -> u32 {
    180
}
#[derive(Clone, Deserialize)]
struct Output {
    id: String,
    label: String,
}
fn preferences() -> Preferences {
    serde_json::from_str(&load_preferences())
        .unwrap_or_else(|_| serde_json::from_str("{}").expect("default preferences"))
}
pub fn saved_grid() -> String {
    preferences().grid
}
pub fn saved_callsign() -> String {
    preferences().callsign
}

#[component]
pub fn StationsToContact(
    queue: RwSignal<StationQueue>,
    mine: RwSignal<String>,
    grid: RwSignal<String>,
    radio: RadioControl,
    running: ReadSignal<bool>,
    open_frequency: ReadSignal<Option<(f64, f32)>>,
    playing: RwSignal<bool>,
) -> impl IntoView {
    let prefs = preferences();
    let find_open_frequency = RwSignal::new(prefs.find_open_frequency);
    let sort_state = RwSignal::new(prefs.sort_state);
    let sort_signal = RwSignal::new(prefs.sort_signal);
    let sort_distance = RwSignal::new(prefs.sort_distance);
    let auto_remove = RwSignal::new(prefs.auto_remove);
    let cq_timeout = RwSignal::new(if prefs.cq_timeout_seconds > 0 {
        prefs.cq_timeout_seconds
    } else {
        default_cq_timeout()
    });

    let output_id = RwSignal::new(prefs.output_id);
    let output_label = RwSignal::new(prefs.output_label);
    let attenuation = RwSignal::new(if prefs.attenuation_db.is_finite() {
        prefs.attenuation_db.clamp(0.0, 60.0)
    } else {
        60.0
    });
    let outputs = RwSignal::new(Vec::<Output>::new());
    let output_status = RwSignal::new(String::new());
    let storage_status = RwSignal::new(String::new());
    let use_radio = RwSignal::new(true);
    Effect::new(move |_| {
        storage_status.set(save_preferences(
            &mine.get(),
            &grid.get(),
            &output_id.get(),
            &output_label.get(),
            attenuation.get(),
            auto_remove.get(),
            cq_timeout.get(),
            sort_state.get(),
            sort_signal.get(),
            sort_distance.get(),
            find_open_frequency.get(),
        ));
    });
    Effect::new(move |_| {
        let find = find_open_frequency.get();
        queue.update(|q| q.find_open_frequency = find);
        let keys = [sort_state.get(), sort_signal.get(), sort_distance.get()];
        let mine = grid.get();
        queue.update(|q| {
            q.sort_preferences = keys;
            q.sort_selected(keys, &mine, js_sys::Date::now());
        });
    });
    let refresh_outputs = move |permission| {
        let promise = list_outputs(permission);
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(json) => {
                    let list: Vec<Output> =
                        serde_json::from_str(&json.as_string().unwrap_or_default())
                            .unwrap_or_default();
                    outputs.try_set(list);
                    output_status.try_set(String::new());
                }
                Err(error) => {
                    output_status.try_set(format!("Audio outputs: {error:?}"));
                }
            }
        });
    };
    let preparing = RwSignal::new(false);
    let current_plan = StoredValue::new_local(None::<u64>);
    let busy = RwSignal::new(false);
    let phase = RwSignal::new(String::new());
    let clock = RwSignal::new(js_sys::Date::now());
    let status =
        RwSignal::new("Select stations, configure your station, then start contacting.".to_owned());
    let ptt = radio.ptt();
    let key_ptt = ptt.clone();
    let key = Rc::new(Closure::<dyn Fn() -> js_sys::Promise>::new(move || {
        let ptt = key_ptt.clone();
        future_to_promise(async move {
            ptt.key()
                .await
                .map(|_| JsValue::UNDEFINED)
                .map_err(|e| JsValue::from_str(&e))
        })
    }));
    let release_ptt = ptt.clone();
    let release = Rc::new(Closure::<dyn Fn() -> js_sys::Promise>::new(move || {
        let ptt = release_ptt.clone();
        future_to_promise(async move {
            ptt.release()
                .await
                .map(|_| JsValue::UNDEFINED)
                .map_err(|e| JsValue::from_str(&e))
        })
    }));
    let available = Rc::new(Closure::<dyn Fn() -> bool>::new(move || ptt.available()));
    let tx = StoredValue::new_local(create_transmitter(
        key.as_ref().as_ref().unchecked_ref(),
        release.as_ref().as_ref().unchecked_ref(),
        available.as_ref().as_ref().unchecked_ref(),
    ));
    Effect::new(move |_| {
        let db = attenuation.get();
        tx.with_value(|tx| set_attenuation(tx, db));
    });
    let generation = StoredValue::new_local(0_u64);
    let callbacks = StoredValue::new_local((key, release, available));
    on_cleanup(move || {
        let promise = tx.with_value(dispose_transmitter);
        // JS may still be awaiting a keying ACK. Keep callbacks alive through release.
        let keep = callbacks.get_value();
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
            drop(keep);
        });
    });

    let validation = Signal::derive(move || {
        transmit::pack_call(&mine.get())
            .and_then(|_| transmit::grid_index(&grid.get()))
            .map(|_| ())
            .and_then(|_| {
                if output_id.get().is_empty() {
                    Err("Select your radio audio output")
                } else {
                    Ok(())
                }
            })
    });
    Effect::new(move |_| {
        if output_supported() {
            refresh_outputs(false);
        }
    });
    let stop = move || {
        generation.update_value(|g| *g += 1);
        queue.update(|q| q.stop());
        let _ = tx.with_value(stop_transmitter);
        playing.set(false);
        preparing.set(false);
        status.set("Contacting paused. Waiting for Start contacting.".into());
    };
    // Audio input Stop/restart also disarms the queue, including output preparation.
    Effect::new(move |_| {
        if !running.get() {
            stop();
        }
    });
    let start = move |_| {
        if validation.get_untracked().is_err() || !running.get_untracked() {
            return;
        }
        preparing.set(true);
        generation.update_value(|g| *g += 1);
        let gen_id = generation.get_value();
        let promise = tx.with_value(|tx| prepare_transmitter(tx, &output_id.get_untracked()));
        status.set("Preparing selected audio output…".into());
        spawn_local(async move {
            let result = JsFuture::from(promise).await;
            if generation
                .try_with_value(|g| *g)
                .is_none_or(|g| g != gen_id)
            {
                return;
            }
            preparing.set(false);
            match result {
                Ok(_) => {
                    queue.update(|q| q.enabled = true);
                    status.set(
                        "Contacting enabled · waiting for a fresh CQ from a queued station.".into(),
                    );
                }
                Err(e) => status.set(format!(
                    "Could not prepare transmit audio: {}",
                    error_text(e)
                )),
            }
        });
    };
    let _timer = StoredValue::new_local(Interval::new(100, move || {
        let now = js_sys::Date::now();
        if (now / 1000.0).floor() != (clock.get_untracked() / 1000.0).floor() {
            clock.set(now);
            if running.get_untracked() {
                queue.update(|q| q.sort_cycle(now, &grid.get_untracked()));
            }
            if running.get_untracked() && auto_remove.get_untracked() {
                queue.update(|q| q.expire_waiting(now, cq_timeout.get_untracked()));
            }
        }
        let active = tx.with_value(transmitter_busy);
        playing.set(tx.with_value(transmitter_playing));
        phase.set(tx.with_value(transmitter_phase));
        if let Some(id) = current_plan.get_value() {
            let still_pending =
                queue.with_untracked(|q| q.pending.as_ref().is_some_and(|p| p.id == id));
            if !still_pending {
                let _ = tx.with_value(|tx| {
                    if queue.with_untracked(|q| q.enabled) {
                        cancel_current(tx)
                    } else {
                        stop_transmitter(tx)
                    }
                });
                current_plan.set_value(None);
            } else if !active {
                let success = tx.with_value(transmitter_outcome) == "complete";
                queue.update(|q| q.complete(id, success));
                current_plan.set_value(None);
                status.set(tx.with_value(transmitter_status));
            }
        }
        busy.set(active);
        if active || preparing.get_untracked() || !queue.with_untracked(|q| q.enabled) {
            return;
        }
        if !running.get_untracked() {
            stop();
            return;
        }
        if !tx.with_value(transmitter_prepared) {
            queue.update(|q| q.stop());
            status.set(format!(
                "{} Click Start contacting to resume.",
                tx.with_value(transmitter_status)
            ));
            return;
        }
        if use_radio.get_untracked() {
            if !radio.configured() {
                stop();
                status.set("Radio disconnected; contacting paused.".into());
                return;
            }
            if !radio.ready() {
                return;
            }
        }
        let mut plan = None;
        queue.update(|q| {
            q.open_frequency_hz = crate::open_frequency::recent_choice(
                open_frequency.get_untracked(),
                js_sys::Date::now(),
            );
            plan = q.plan(
                js_sys::Date::now(),
                &mine.get_untracked(),
                &grid.get_untracked(),
            )
        });
        let Some(plan) = plan else {
            return;
        };
        queue.update(|q| q.record_frequency(plan.id, radio.frequency()));
        let samples = match transmit::audio(&plan.message, plan.frequency_hz) {
            Ok(pcm) => pcm,
            Err(e) => {
                queue.update(|q| q.complete(plan.id, false));
                status.set(e.into());
                return;
            }
        };
        if use_radio.get_untracked()
            && let Err(e) = radio.reserve()
        {
            queue.update(|q| q.complete(plan.id, false));
            status.set(e);
            return;
        }
        let promise = tx.with_value(|tx| {
            play_transmission(
                tx,
                &samples,
                plan.receive_slot as f64,
                &output_id.get_untracked(),
                use_radio.get_untracked(),
                plan.start_ms,
            )
        });
        current_plan.set_value(Some(plan.id));
        busy.set(true);
        status.set(format!("{} · scheduled {}", plan.target, plan.message));
        spawn_local(async move {
            if let Err(e) = JsFuture::from(promise).await {
                if current_plan.try_with_value(|id| *id).flatten() != Some(plan.id) {
                    return;
                }
                queue.update(|q| q.complete(plan.id, false));
                current_plan.set_value(None);
                status.set(format!("Transmit paused: {}", error_text(e)));
            }
        });
    }));
    view! {
        <section class="stations-panel results" aria-label="Stations to contact">
            <div class="toolbar"><h2>"Stations to contact"</h2><button class="secondary" disabled=move || queue.with(|q| q.items.is_empty()) on:click=move |_| { stop(); queue.update(|q| q.clear()); }>"Remove all"</button><span>{move || format!("{} queued · first eligible CQ gets priority", queue.with(|q| q.items.len()))}</span></div>
            <label><input type="checkbox" prop:checked=move || find_open_frequency.get() on:change=move |ev| find_open_frequency.set(event_target_checked(&ev)) />"find an open frequency"</label>
            <p>"Uses up to 60 seconds of available waterfall history to choose a quiet 50 Hz transmit space within 100–3000 audio Hz. Each new contact gets a new choice, held through retries and replies. Changes apply to the next contact. This estimates occupancy; it cannot guarantee a clear channel."</p>
            <p hidden=move || { let _ = clock.get(); !find_open_frequency.get() || playing.get() || queue.with(|q| q.active.is_some()) || crate::open_frequency::recent_choice(open_frequency.get(), js_sys::Date::now()).is_some() }>"Waiting for waterfall data before choosing a transmit frequency."</p>
            <div class="card-sort" aria-label="Sort stations to contact">
                <span>"Sort each cycle:"</span>
                <label><input type="checkbox" prop:checked=move || sort_state.get() on:change=move |ev| sort_state.set(event_target_checked(&ev)) />"State · CQ first"</label>
                <label><input type="checkbox" prop:checked=move || sort_signal.get() on:change=move |ev| sort_signal.set(event_target_checked(&ev)) />"Signal ↓"</label>
                <label><input type="checkbox" prop:checked=move || sort_distance.get() on:change=move |ev| sort_distance.set(event_target_checked(&ev)) />"Distance ↑"</label>
                <span>"Active first · penalized last · checked keys: state, signal, distance (set My grid)"</span>
            </div>
            <p>"Click a conversation below to move that station here. Start contacting enables CQ-triggered calls and the ensuing exchange. Queued frequencies are searched first; full-band decoding continues. One station at a time; stations in other contacts wait for a fresh CQ."</p>
            <div class="transmit-settings">
                <label><input type="checkbox" prop:checked=move || auto_remove.get()
                    on:change=move |ev| auto_remove.set(event_target_checked(&ev)) />"Automatically remove stations without CQ"</label>
                <label>"CQ timeout (seconds)"<input type="number" min="1" step="1"
                    prop:value=move || cq_timeout.get() disabled=move || !auto_remove.get()
                    on:change=move |ev| cq_timeout.set(event_target_value(&ev).parse::<u32>().ok().filter(|s| *s > 0).unwrap_or(180)) /></label>
            </div>
            <p>"Measured from the last CQ's end while receiving. Active exchanges are kept; stations with no CQ get a full timeout."</p>
            <details class="queue-settings" open><summary>"Station and transmit settings"</summary>
            <div class="transmit-settings">
                <label>"My callsign"<input prop:value=move || mine.get() disabled=move || busy.get() || preparing.get() || queue.with(|q| q.enabled)
                    on:input=move |ev| mine.set(event_target_value(&ev).trim().to_ascii_uppercase()) placeholder="W9XYZ" /></label>
                <label>"My grid"<input prop:value=move || grid.get() disabled=move || busy.get() || preparing.get() || queue.with(|q| q.enabled)
                    on:input=move |ev| grid.set(event_target_value(&ev).trim().to_ascii_uppercase()) placeholder="EN37" maxlength="4" /></label>
            </div>
            <label class="transmit-output">"Transmit audio output"
                <select aria-label="Transmit audio output" prop:value=move || output_id.get() disabled=move || busy.get() || preparing.get() || queue.with(|q| q.enabled) || !output_supported()
                    on:change=move |ev| { let id = event_target_value(&ev); let label = outputs.with_untracked(|list| list.iter().find(|o| o.id == id).map(|o| o.label.clone())).unwrap_or_else(|| "System default".into()); output_label.set(label); output_id.set(id); }>
                    <option value="" selected=move || output_id.get().is_empty()>"Choose radio audio output…"</option>
                    <option value="default" selected=move || output_id.get() == "default">"System default output (explicit choice)"</option>
                    {move || { let saved = output_id.get(); let list = outputs.get(); let missing = !saved.is_empty() && saved != "default" && !list.iter().any(|o| o.id == saved); view! {
                        {missing.then(|| view! { <option value=saved selected=true>{format!("{} (saved; refresh devices)", output_label.get())}</option> })}
                        {list.into_iter().filter(|o| o.id != "default").map(|o| { let id = o.id.clone(); view! { <option value=o.id selected=move || output_id.get() == id>{o.label}</option> } }).collect_view()}
                    } }}
                </select>
            </label>
            <button class="secondary" on:click=move |_| refresh_outputs(true) disabled=move || busy.get() || preparing.get() || queue.with(|q| q.enabled)>"Refresh audio outputs"</button>
            <p>{move || output_status.get()}</p>
            <p>{move || storage_status.get()}</p>
            <p hidden=output_supported()>"Audio output selection is unavailable in this browser. Use a browser supporting AudioContext.setSinkId."</p>
            <label><input type="checkbox" prop:checked=move || use_radio.get() disabled=move || busy.get() || preparing.get() || queue.with(|q| q.enabled) on:change=move |ev| use_radio.set(event_target_checked(&ev)) />"Use connected radio PTT"</label>
            <p hidden=move || !use_radio.get() || radio.configured() || busy.get()>"Connect the radio and configure PTT in the radio panel before transmitting."</p>


            </details>
            <div class="tx-level">
                <label for="tx-attenuation">"TX audio attenuation"</label>
                <input id="tx-attenuation" type="range" min="0" max="60" step="1" prop:value=move || attenuation.get()
                    aria-valuetext=move || format!("{} dB attenuation", attenuation.get())
                    on:input=move |ev| attenuation.set(event_target_value(&ev).parse::<f32>().ok().filter(|db| db.is_finite()).unwrap_or(60.0).clamp(0.0,60.0)) />
                <output for="tx-attenuation">{move || format!("−{:.0} dB · {:.1}% amplitude", attenuation.get(), 100.0*10.0_f32.powf(-attenuation.get()/20.0))}</output>
                <p>"Move right to lower the radio’s audio drive. Applies while transmitting and is saved in this browser."</p>
            </div>
            <p class="transmit-validation">{move || validation.get().err().unwrap_or("")}</p>
            <div class="toolbar">
                <button on:click=start disabled=move || preparing.get() || busy.get() || queue.with(|q| q.enabled || q.items.is_empty()) || !running.get() || validation.get().is_err() || !output_supported() || (use_radio.get() && !radio.configured())>"Start contacting"</button>
                <button class="secondary" on:click=move |_| stop() disabled=move || !preparing.get() && !busy.get() && !queue.with(|q| q.enabled)>"Stop contacting"</button>
                <strong>{move || if queue.with(|q| q.enabled) { "CQ watch enabled" } else { "Paused" }}</strong>
            </div>
            <p class="transmit-phase">{move || phase.get()}</p>
            <p role="status">{move || status.get()}</p>
            <p hidden=move || running.get()>"Start listening to watch for CQ calls."</p>
            <p class="empty" hidden=move || !queue.with(|q| q.items.is_empty())>"No stations selected. Click a conversation to add a station."</p>
            <div class="station-queue">
                <For each=move || queue.with(|q| q.items.clone()) key=|s| s.call.clone() children=move |initial| {
                    let call = initial.call.clone();
                    let card_call = call.clone();
                    let station = Signal::derive(move || queue.with(|q| q.items.iter().find(|s| s.call == card_call).cloned()).unwrap_or_else(|| initial.clone()));
                    let active_call = call.clone();
                    let contact = Signal::derive(move || queue.with(|q| q.active.as_ref().filter(|a| a.contact.target == active_call).map(|a| a.contact.clone())));
                    let stage_call = call.clone();
                    let pending_call = call.clone();
                    let pending = Signal::derive(move || queue.with(|q| q.pending.as_ref().filter(|p| p.target == pending_call).cloned()));
                    let transmitting = Signal::derive(move || pending.get().is_some() && playing.get());
                    let stages = Signal::derive(move || queue.with(|q| q.exchange_slots(&stage_call, clock.get())));
                    let rank_call = call.clone();
                    let front_call = call.clone(); let back_call = call.clone(); let remove_call = call.clone();
                    view! {
                        <article class="queued-station" data-station=call.clone() class:active=move || contact.get().is_some()
                            class:transmitting=move || transmitting.get()
                            class:has-transmitted=move || station.with(|s| s.sent_messages.iter().any(Option::is_some))
                            style=move || format!("--station-color:{0};--conversation-color:{0}", station.get().kind.color())>
                            <div class="toolbar"><h3>{call.clone()}</h3><span>{move || queue.with(|q| q.items.iter().position(|s| s.call == rank_call).map(|i| format!("Priority {}", i+1)).unwrap_or_default())}</span></div>
                            <strong>{move || contact.get().map(|c| c.label().to_owned()).unwrap_or_else(|| station.get().state(js_sys::Date::now()).to_owned())}</strong>
                            <p class="station-cq">{move || station.with(|s| {
                                s.last_cq_ms.map(|ms| format!("Last CQ: {}", age(clock.get() - ms - 12_640.0))).unwrap_or_else(|| "Last CQ: not heard this session".into())
                            })}</p>
                            <p class="station-cq-delay">{move || station.with(|s| match (s.last_cq_ms, s.last_cq_arrival_ms) {
                                (Some(start), Some(arrival)) => {
                                    let delay = ((arrival-start-12640.0)/1000.0).max(0.0);
                                    let deadline = ((start/15000.0).round()+1.0)*15000.0+300.0;
                                    if arrival > deadline { format!("Decoded {delay:.1} s after CQ ended · reply deadline missed by {:.1} s", (arrival-deadline)/1000.0) }
                                    else { format!("Decoded {delay:.1} s after CQ ended · received before reply deadline") }
                                }
                                _ => String::new(),
                            })}</p>
                            <p class="station-sort-penalty">{move || station.with(|s| {
                                let seconds = ((s.sort_penalty_until_ms - clock.get()) / 1000.0).ceil();
                                if seconds > 0.0 { format!("Sorting penalty: {seconds:.0} s remaining · four unanswered calls") } else { String::new() }
                            })}</p>
                            <p class="station-match">{move || station.with(|s| {
                                let chosen = queue.with(|q| q.active.as_ref().filter(|a| a.contact.target == s.call).and_then(|a| a.transmit_frequency_hz));
                                let placement = match chosen {
                                    Some(hz) => format!("TX {hz:.1} Hz · held open-frequency choice"),
                                    None if find_open_frequency.get() && contact.get().is_none() => "TX pending · open frequency chosen when contact starts".into(),
                                    None => format!("TX {:.1} Hz · matches received audio", s.frequency_hz),
                                };
                                format!("{placement} · report {:+03} dB from received SNR", s.snr_db.round().clamp(-50.0,49.0) as i32)
                            })}</p>
                            <p class="station-observation">{move || station.with(|s| if s.latest_text.is_empty() { "No current-session evidence yet".into() } else { format!("Last heard: {:+.1} dB · {:.0} audio Hz", s.snr_db, s.frequency_hz) })}</p>
                            <p class="station-location">{move || station.with(|s| crate::geography::description(s.location.as_ref().map(|(_, l)| l), &grid.get()))}</p>
                            <p class="message">{move || station.get().latest_text}</p>
                            <p>{move || station.get().note}</p>
                            <p>{move || format!("Contact tries: {} · successful initial-call audio; RF not confirmed", station.get().contact_tries)}</p>
                            <p class="station-tx-status" role="status">{move || pending.get().map(|p| format!("{} · {}", if transmitting.get() { "TX AUDIO ON" } else { "TX queued / finishing" }, p.message)).unwrap_or_else(|| if contact.get().is_some() { "Listening · waiting for partner’s reply".into() } else { String::new() })}</p>
                            <crate::exchange_ui::ExchangeStages slots=stages />
                            <ol class="transmit-steps" aria-label="Our contact sequence">
                                {["Locator", "Report", "R-report", "RR73", "73"].into_iter().enumerate().map(|(step, label)| view! {
                                    <li class:current=move || contact.with(|c| c.as_ref().is_some_and(|c| c.step == step))
                                        class:transmitting=move || transmitting.get() && pending.with(|p| p.as_ref().is_some_and(|p| p.step == step))
                                        class:sent=move || station.with(|s| s.sent_messages[step].is_some())>
                                        <span>{label}</span>
                                        <strong class="tx-step-status">{move || if transmitting.get() && pending.with(|p| p.as_ref().is_some_and(|p| p.step == step)) { "● Transmitting" } else if pending.with(|p| p.as_ref().is_some_and(|p| p.step == step)) { "Queued / finishing" } else if station.with(|s| s.sent_messages[step].is_some()) { "✓ Sent audio" } else if contact.with(|c| c.as_ref().is_some_and(|c| c.step == step)) { "Next reply" } else { "Not sent" }}</strong>
                                        <span class="tx-step-message">{move || pending.get().filter(|p| p.step == step).map(|p| p.message).or_else(|| station.get().sent_messages[step].clone()).unwrap_or_default()}</span>
                                    </li>
                                }).collect_view()}
                            </ol>
                            <p class="tx-state-legend">"Pulsing amber: transmitting audio · Steady cyan: sent audio. Local playback does not confirm RF delivery."</p>
                            <details><summary>"Received evidence"</summary><ul>{move || station.get().evidence.into_iter().map(|m| view! { <li class="message">{m}</li> }).collect_view()}</ul></details>
                            <div class="station-actions">
                                <button class="secondary" on:click=move |_| queue.update(|q| q.to_front(&front_call))>"Move to front"</button>
                                <button class="secondary" on:click=move |_| queue.update(|q| q.to_back(&back_call))>"Move to back"</button>
                                <button class="secondary" on:click=move |_| queue.update(|q| q.remove(&remove_call))>"Remove"</button>
                            </div>
                        </article>
                    }
                } />
            </div>
            <p class="spectrum-detail">"CQ responses use the immediately following UTC slot. After four initial calls with no reply, the station moves to the back, sorts behind other stations for three minutes and waits for a fresh CQ. Later stages allow three transmissions; attempts stop after eight transmissions or three minutes. Removing or moving an active station to the back cancels its attempt. Move to front and sorting keep the current exchange active. Stop, hidden tabs and input interruption pause contacting. Physical radio timing remains uncalibrated."</p>
        </section>
    }
}

fn error_text(e: JsValue) -> String {
    js_sys::Reflect::get(&e, &JsValue::from_str("message"))
        .ok()
        .and_then(|v| v.as_string())
        .or_else(|| e.as_string())
        .unwrap_or_else(|| format!("{e:?}"))
}

fn age(elapsed_ms: f64) -> String {
    let seconds = (elapsed_ms.max(0.0) / 1000.0).floor() as u64;
    if seconds == 0 {
        "just now".into()
    } else if seconds < 60 {
        format!("{seconds} s ago")
    } else if seconds < 3600 {
        format!("{} min {} s ago", seconds / 60, seconds % 60)
    } else {
        format!("{} h {} min ago", seconds / 3600, seconds % 3600 / 60)
    }
}
