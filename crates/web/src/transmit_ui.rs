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
        validate: &js_sys::Function,
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
        plan_id: f64,
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
    visible: Signal<bool>,
    configuration: Signal<bool>,
    queue: RwSignal<StationQueue>,
    mine: RwSignal<String>,
    grid: RwSignal<String>,
    radio: RadioControl,
    running: ReadSignal<bool>,
    receive_status: ReadSignal<String>,
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
    let validate = Rc::new(Closure::<dyn Fn(f64) -> bool>::new(move |id: f64| {
        id.is_finite()
            && id >= 1.0
            && id.fract() == 0.0
            && queue.with_untracked(|q| q.can_transmit(id as u64))
    }));
    let tx = StoredValue::new_local(create_transmitter(
        key.as_ref().as_ref().unchecked_ref(),
        release.as_ref().as_ref().unchecked_ref(),
        available.as_ref().as_ref().unchecked_ref(),
        validate.as_ref().as_ref().unchecked_ref(),
    ));
    Effect::new(move |_| {
        let db = attenuation.get();
        tx.with_value(|tx| set_attenuation(tx, db));
    });
    let generation = StoredValue::new_local(0_u64);
    let callbacks = StoredValue::new_local((key, release, available, validate));
    on_cleanup(move || {
        let promise = tx.with_value(dispose_transmitter);
        // JS may still be awaiting a keying ACK. Keep callbacks alive through release.
        let keep = callbacks.get_value();
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
            drop(keep);
        });
    });

    // React to decoded evidence immediately; an interval-only recheck can miss
    // the gap between the last 100 ms tick and the audio/PTT deadline.
    Effect::new(move |_| {
        let stale = queue.with(|q| {
            q.pending
                .as_ref()
                .is_some_and(|p| js_sys::Date::now() < p.start_ms && !q.can_transmit(p.id))
        });
        if stale {
            queue.update(|q| q.refresh_pending(js_sys::Date::now()));
            if current_plan.get_value().is_some() {
                current_plan.set_value(None);
                let _ = tx.with_value(cancel_current);
            }
        }
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
        if configuration.get() && output_supported() {
            refresh_outputs(false);
        }
    });
    let stop_for_reason = move |reason: String| {
        generation.update_value(|g| *g += 1);
        queue.update(|q| q.stop_with_reason(&reason));
        let _ = tx.with_value(stop_transmitter);
        playing.set(false);
        busy.set(false);
        current_plan.set_value(None);
        phase.set(String::new());
        preparing.set(false);
        status.set(reason);
    };
    let stop = move || stop_for_reason("Contacting paused. Waiting for Start contacting.".into());
    // Audio input Stop/restart also disarms the queue, including output preparation.
    Effect::new(move |_| {
        if !running.get()
            && (queue.with_untracked(|q| q.enabled || q.active.is_some())
                || preparing.get_untracked())
        {
            stop_for_reason(format!(
                "Receive stopped: {}",
                receive_status.get_untracked()
            ));
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
    let frequency_queries = StoredValue::new_local(std::collections::BTreeSet::<String>::new());
    let frequency_query_busy = RwSignal::new(false);
    let timer = StoredValue::new_local(None::<Interval>);
    Effect::new(move |_| {
        timer.update_value(|timer| *timer = None);
        // Session scheduling continues while other application pages are open.
        timer.set_value(Some(Interval::new(100, move || {
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
            queue.update(|q| q.refresh_pending(now));
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
                    let reason = tx.with_value(transmitter_status);
                    queue.update(|q| {
                        if success {
                            q.complete(id, true);
                        } else if tx.with_value(transmitter_outcome) == "superseded" {
                            q.revoke_pending(id);
                        } else {
                            q.fail(id, &reason);
                        }
                    });
                    current_plan.set_value(None);
                    status.set(reason);
                }
            }
            // Clock-driven QSO limits must not depend on radio/query readiness.
            queue.update(|q| q.advance_time(now));
            // Leave at least three seconds before the next slot's keying deadline.
            // Completed records survive queue removal and asynchronous persistence.
            if !active
                && radio.contact_frequency_ready()
                && !frequency_query_busy.get_untracked()
                && now.rem_euclid(15_000.0) < 12_000.0
            {
                let completed = queue.with_untracked(StationQueue::completed_record_ids);
                frequency_queries.update_value(|ids| ids.retain(|id| completed.contains(id)));
                let next = completed
                    .into_iter()
                    .find(|id| !frequency_queries.with_value(|ids| ids.contains(id)));
                if let Some(id) = next {
                    frequency_queries.update_value(|ids| {
                        ids.insert(id.clone());
                    });
                    frequency_query_busy.set(true);
                    spawn_local(async move {
                        if let Some(sample) = radio.read_contact_frequency().await {
                            queue.try_update(|q| q.record_completed_frequency(&id, sample));
                        }
                        frequency_query_busy.try_set(false);
                    });
                }
            }
            busy.set(active);
            if active
                || frequency_query_busy.get_untracked()
                || preparing.get_untracked()
                || !queue.with_untracked(|q| q.enabled)
            {
                return;
            }
            if !running.get_untracked() {
                stop();
                return;
            }
            if !tx.with_value(transmitter_prepared) {
                stop_for_reason(format!(
                    "{} Click Start contacting to resume.",
                    tx.with_value(transmitter_status)
                ));
                return;
            }
            if use_radio.get_untracked() {
                if !radio.configured() {
                    stop_for_reason("Radio disconnected; contacting paused.".into());
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
                    queue.update(|q| q.fail(plan.id, e));
                    status.set(e.into());
                    return;
                }
            };
            if use_radio.get_untracked()
                && let Err(e) = radio.reserve()
            {
                queue.update(|q| q.fail(plan.id, &e));
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
                    plan.id as f64,
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
                    let reason = format!("Transmit paused: {}", error_text(e));
                    queue.update(|q| q.fail(plan.id, &reason));
                    current_plan.set_value(None);
                    status.set(reason);
                }
            });
        })));
    });
    view! {
        <Show when=move || configuration.get()>
        <section class="contact-configuration results" aria-label="Contact configuration">
            <h2>"Contact configuration"</h2>
            <label><input type="checkbox" prop:checked=move || find_open_frequency.get() on:change=move |ev| find_open_frequency.set(event_target_checked(&ev)) />"find an open frequency"</label>
            <p>"Uses up to 60 seconds of available waterfall history to choose a quiet 50 Hz transmit space within 250–2500 audio Hz. Each new contact gets a new choice, held through retries and replies. Changes apply to the next contact. This estimates occupancy; it cannot guarantee a clear channel."</p>
            <p hidden=move || { let _ = clock.get(); !find_open_frequency.get() || playing.get() || queue.with(|q| q.active.is_some()) || crate::open_frequency::recent_choice(open_frequency.get(), js_sys::Date::now()).is_some() }>"Waiting for waterfall data before choosing a transmit frequency."</p>
            <p>"Open Conversations and click a station to move it here. Start contacting enables CQ-triggered calls and the ensuing exchange. Queued frequencies are searched first; full-band decoding continues. One station at a time; stations in other contacts wait for a fresh CQ."</p>
            <div class="transmit-settings">
                <label><input type="checkbox" prop:checked=move || auto_remove.get()
                    on:change=move |ev| auto_remove.set(event_target_checked(&ev)) />"Automatically remove stations without CQ"</label>
                <label>"CQ timeout (seconds)"<input type="number" min="1" step="1"
                    prop:value=move || cq_timeout.get() disabled=move || !auto_remove.get()
                    on:change=move |ev| cq_timeout.set(event_target_value(&ev).parse::<u32>().ok().filter(|s| *s > 0).unwrap_or(180)) /></label>
            </div>
            <p>"Measured from the last CQ's end. Checked continuously while listening; stations with no CQ get a full timeout."</p>

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



            <p class="spectrum-detail">"CQ responses use the immediately following UTC slot. After four initial calls with no reply, the station moves to the back, sorts behind other stations for three minutes and waits for a fresh CQ. Later stages allow three transmissions; attempts stop after eight transmissions or three minutes. Removing or moving an active station to the back cancels its attempt. Move to front and sorting keep the current exchange active. Stop, hidden tabs and input interruption pause contacting. Physical radio timing remains uncalibrated."</p>
        </section>
        </Show>
        <Show when=move || visible.get()>
        <section class="stations-panel results" aria-label="Stations to contact">
            <div class="toolbar"><h2>"Stations to contact"</h2><button on:click=move |ev| { if preparing.get() || busy.get() || queue.with(|q| q.enabled) { stop(); } else { start(ev); } }
                    disabled=move || !(preparing.get() || busy.get() || queue.with(|q| q.enabled)) && (queue.with(|q| q.items.is_empty()) || !running.get() || validation.get().is_err() || !output_supported() || (use_radio.get() && !radio.configured()))>
                    {move || if preparing.get() || busy.get() || queue.with(|q| q.enabled) { "Stop contacting" } else { "Start contacting" }}
                </button><button class="secondary" disabled=move || queue.with(|q| q.items.is_empty()) on:click=move |_| { stop(); queue.update(|q| q.clear()); }>"Remove all"</button><span>{move || format!("{} queued · first eligible CQ gets priority", queue.with(|q| q.items.len()))}</span></div>
            <p class="contact-machine-state" role="status">{move || queue.with(|q| q.state_label())}</p>
            <section class="contact-policy" aria-label="Conversation policy">
                <strong>"Next action"</strong>
                <p class="active-conversation">{move || queue.with(|q| q.active.as_ref()
                    .map(|a| format!("{}: {}", if a.contact.is_established() { "Active conversation" } else { "Attempting contact" }, a.contact.target))
                    .unwrap_or_else(|| "No active conversation · listening for an eligible CQ".into()))}</p>
                <p class="contact-policy-decision" role="status">{move || queue.with(|q| q.policy_label())}</p>
                <details class="contact-policy-rules"><summary>"Selection and attempt rules"</summary>
                  <p>"Until a partner report arrives, this is a contact attempt: a fresh CQ can replace it in queue priority order. A partner report establishes an active conversation. Initial calls allow up to four transmissions. Recheck the received exchange before keying; otherwise listen."</p>
                <p>"Received stages below show only decoded evidence. Local sent audio stays separate. Later stages allow three transmissions, with eight transmissions or three minutes per attempt."</p>
                </details>
            </section>
            <div class="card-sort" aria-label="Sort stations to contact">
                <span>"Auto-sort:"</span>
                <label><input type="checkbox" prop:checked=move || sort_state.get() on:change=move |ev| sort_state.set(event_target_checked(&ev)) />"State · CQ first"</label>
                <label><input type="checkbox" prop:checked=move || sort_signal.get() on:change=move |ev| sort_signal.set(event_target_checked(&ev)) />"Signal ↓"</label>
                <label><input type="checkbox" prop:checked=move || sort_distance.get() on:change=move |ev| sort_distance.set(event_target_checked(&ev)) />"Distance ↑"</label>
                  <span>"Established QSO first · penalized last · checked keys: state, signal, distance (set My grid)"</span>
            </div>
            <div class="tx-level">
                <label for="tx-attenuation">"TX level"</label>
                <input id="tx-attenuation" type="range" min="0" max="60" step="1" prop:value=move || attenuation.get()
                    aria-valuetext=move || format!("{} dB attenuation", attenuation.get())
                    on:input=move |ev| attenuation.set(event_target_value(&ev).parse::<f32>().ok().filter(|db| db.is_finite()).unwrap_or(60.0).clamp(0.0,60.0)) />
                <output for="tx-attenuation">{move || format!("−{:.0} dB · {:.1}% amplitude", attenuation.get(), 100.0*10.0_f32.powf(-attenuation.get()/20.0))}</output>
            </div>
            <p class="transmit-validation">{move || validation.get().err().unwrap_or("")}</p>
            <p class="transmit-phase">{move || phase.get()}</p>
            <p role="status">{move || status.get()}</p>
            <p hidden=move || running.get()>"Start listening to watch for CQ calls."</p>
            <p class="empty" hidden=move || !queue.with(|q| q.items.is_empty())>"No stations selected. Click a conversation to add a station."</p>
            <div class="station-queue" tabindex="0" role="region" aria-label="Queued station cards">
                <For each=move || queue.with(|q| q.items.clone()) key=|s| s.call.clone() children=move |initial| {
                    let call = initial.call.clone();
                    let card_call = call.clone();
                    let station = Signal::derive(move || queue.with(|q| q.items.iter().find(|s| s.call == card_call).cloned()).unwrap_or_else(|| initial.clone()));
                    let active_call = call.clone();
                    let contact = Signal::derive(move || queue.with(|q| q.active.as_ref().filter(|a| a.contact.target == active_call).map(|a| a.contact.clone())));
                    let attempt_call = call.clone();
                    let stage_call = call.clone();
                    let pending_call = call.clone();
                    let pending = Signal::derive(move || queue.with(|q| q.pending.as_ref().filter(|p| p.target == pending_call).cloned()));
                    let transmitting = Signal::derive(move || pending.get().is_some() && playing.get());
                    let stages = Signal::derive(move || queue.with(|q| q.exchange_slots(&stage_call, clock.get())));
                    let history_call = call.clone();
                    let expected_call = call.clone();
                    let rank_call = call.clone();
                    let front_call = call.clone(); let back_call = call.clone(); let remove_call = call.clone();
                    view! {
                        <article class="queued-station" data-station=call.clone() class:active=move || contact.get().is_some()
                            class:transmitting=move || transmitting.get()
                            class:has-transmitted=move || station.with(|s| s.sent_messages.iter().any(Option::is_some))
                            style=move || format!("--station-color:{0};--conversation-color:{0}", station.get().kind.color())>
                            <div class="toolbar"><h3>{call.clone()}</h3><span>{move || queue.with(|q| q.items.iter().position(|s| s.call == rank_call).map(|i| format!("Priority {}", i+1)).unwrap_or_default())}</span></div>
                            <span class="station-role">{move || contact.get().map(|c| if c.is_established() { "Active conversation" } else { "Contact attempt" }).unwrap_or("Waiting conversation")}</span>
                            <p class="station-expected">{move || queue.with(|q| q.expected_action(&expected_call))}</p>
                            <strong>{move || contact.get().map(|c| c.label().to_owned()).unwrap_or_else(|| station.get().state(js_sys::Date::now()).to_owned())}</strong>
                            <p class="station-cq">{move || station.with(|s| {
                                s.last_cq_ms.map(|ms| format!("Last CQ: {}", age(clock.get() - ms - 12_640.0))).unwrap_or_else(|| "Last CQ: not heard this session".into())
                            })}</p>
                            <p class="station-cq-delay">{move || station.with(|s| match (s.last_cq_ms, s.last_cq_arrival_ms) {
                                (Some(start), Some(arrival)) => {
                                    let delay = ((arrival-start-12640.0)/1000.0).max(0.0);
                                    let deadline = ((start/15000.0).round()+1.0)*15000.0+300.0;
                                    if arrival > deadline { format!("Decoded {delay:.1} s after CQ ended · reply deadline missed by {:.1} s", (arrival-deadline)/1000.0) }
                                    else { format!("Decoded {delay:.1} s after CQ ended") }
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
                                    Some(hz) => format!("TX {hz:.1} Hz"),
                                    None if find_open_frequency.get() && contact.get().is_none() => "TX pending".into(),
                                    None => format!("TX {:.1} Hz", s.frequency_hz),
                                };
                                format!("{placement} · report {:+03} dB", s.snr_db.round().clamp(-50.0,49.0) as i32)
                            })}</p>
                            <p class="station-observation">{move || station.with(|s| if s.latest_text.is_empty() { "No current-session evidence yet".into() } else { format!("Last heard: {:+.1} dB · {:.0} audio Hz", s.snr_db, s.frequency_hz) })}</p>
                            <p class="station-location">{move || station.with(|s| crate::geography::description(s.location.as_ref().map(|(_, l)| l), &grid.get()))}</p>
                            <p class="message">{move || station.get().latest_text}</p>
                            <p>{move || station.get().note}</p>
                            <p class="station-attempt-calls">{move || queue.with(|q| {
                                q.active.as_ref().filter(|a| a.contact.target == attempt_call).map(|a| {
                                    if a.contact.state() == crate::transmit::QsoState::Calling {
                                        format!("Initial calls this attempt: {}/4", a.transmissions)
                                    } else {
                                        format!("Exchange transmissions this attempt: {}/8", a.transmissions)
                                    }
                                }).unwrap_or_else(|| "No active attempt".into())
                            })}</p>
                            <p>{move || format!("Total initial calls while queued: {}", station.get().contact_tries)}</p>
                            <p class="station-tx-status" role="status">{move || pending.get().map(|p| format!("{} · {}", if transmitting.get() { "TX AUDIO ON" } else { "TX queued / finishing" }, p.message)).unwrap_or_else(|| if contact.get().is_some() { "Listening · waiting for partner’s reply".into() } else { String::new() })}</p>
                            <crate::exchange_ui::ExchangeStages slots=stages transmit=Signal::derive(move || {
                                let pending = pending.get();
                                let transmitting = transmitting.get();
                                let station = station.get();
                                let contact = contact.get();
                                crate::contact_queue::transmit_stages(&station, pending.as_ref(), contact.as_ref().map(|c| c.step()), transmitting)
                            }) />
                            <p class="tx-state-legend">"Pulsing amber: transmitting audio · Steady cyan: sent audio. Local playback does not confirm RF delivery."</p>
                            <details><summary>"Received conversation history"</summary><ul>{move || queue.with(|q| q.received_history(&history_call)).into_iter().map(|heard| view! { <li class="message">{heard.message}</li> }).collect_view()}</ul></details>
                            <div class="station-actions">
                                <button class="secondary" on:click=move |_| queue.update(|q| q.to_front(&front_call))>"Move to front"</button>
                                <button class="secondary" on:click=move |_| queue.update(|q| q.to_back(&back_call))>"Move to back"</button>
                                <button class="secondary" on:click=move |_| queue.update(|q| q.remove(&remove_call))>"Remove"</button>
                            </div>
                        </article>
                    }
                } />
            </div>

        </section>
        </Show>
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
