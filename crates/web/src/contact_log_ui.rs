//! Tabbed, paginated local log. Database writes are acknowledged only after commit.
use crate::{contact_log::CompletedContact, contact_queue::StationQueue};
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

#[wasm_bindgen(module = "/src/contact-log-runtime.js")]
extern "C" {
    #[wasm_bindgen(js_name = loadContactExclusion)]
    fn load_exclusion() -> String;
    #[wasm_bindgen(js_name = saveContactExclusion)]
    fn save_exclusion(json: &str) -> String;
    #[wasm_bindgen(js_name = listContacts)]
    fn list_contacts(offset: usize) -> js_sys::Promise;
    #[wasm_bindgen(js_name = saveContacts)]
    fn save_contacts(json: &str, offset: usize) -> js_sys::Promise;
    #[wasm_bindgen(js_name = downloadAdif)]
    fn download_adif(include_incomplete: bool) -> js_sys::Promise;
    #[wasm_bindgen(js_name = disposeContactLog)]
    fn dispose_log();
}

use wsjt_log::Page;
fn parse(value: JsValue) -> Result<Page, String> {
    serde_json::from_str(&value.as_string().unwrap_or_default()).map_err(|e| e.to_string())
}
fn error(value: JsValue) -> String {
    js_sys::Reflect::get(&value, &"message".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| format!("{value:?}"))
}
fn utc(ms: f64) -> String {
    js_sys::Date::new(&ms.into()).to_iso_string().into()
}

#[component]
pub fn ContactLog(
    visible: Signal<bool>,
    queue: RwSignal<StationQueue>,
    contacted: RwSignal<Vec<CompletedContact>>,
    ready: RwSignal<bool>,
) -> impl IntoView {
    let page = RwSignal::new(Page::default());
    let busy = RwSignal::new(false);
    let failed = RwSignal::new(false);
    let status = RwSignal::new("Opening local contact database…".to_owned());
    let offset = RwSignal::new(0_usize);
    let refresh = RwSignal::new(0_usize);
    let evidence_tab = RwSignal::new(false);
    let selected_id = RwSignal::new(None::<String>);
    let incomplete = RwSignal::new(false);
    let selected = Signal::derive(move || {
        page.with(|p| {
            p.records
                .iter()
                .find(|r| Some(&r.id) == selected_id.read().as_ref())
                .cloned()
        })
    });
    Effect::new(move |_| {
        refresh.get();
        let start = offset.get();
        busy.set(true);
        spawn_local(async move {
            let result = JsFuture::from(list_contacts(start))
                .await
                .map_err(error)
                .and_then(parse);
            match result {
                Ok(p) => {
                    contacted.try_set(p.contacted.clone());
                    ready.try_set(true);
                    page.try_set(p);
                    failed.try_set(false);
                    status.try_set("Stored on this browser/site using SQLite. No upload.".into());
                }
                Err(e) => {
                    failed.try_set(true);
                    status.try_set(format!(
                        "{e} Unsaved records remain in this tab. Retry before closing."
                    ));
                }
            }
            busy.try_set(false);
        });
    });
    Effect::new(move |_| {
        if busy.get() || failed.get() {
            return;
        }
        let updates = queue.with(|q| q.log_pending.values().cloned().collect::<Vec<_>>());
        if updates.is_empty() {
            return;
        }
        busy.set(true);
        let start = offset.get_untracked();
        spawn_local(async move {
            let result = match serde_json::to_string(&updates) {
                Ok(json) => JsFuture::from(save_contacts(&json, start))
                    .await
                    .map_err(error)
                    .and_then(parse),
                Err(e) => Err(e.to_string()),
            };
            match result {
                Ok(p) => {
                    queue.try_update(|q| q.acknowledge_log(&updates));
                    contacted.try_set(p.contacted.clone());
                    ready.try_set(true);
                    page.try_set(p);
                    status.try_set("Saved locally in SQLite · received evidence and local TX audio kept separate.".into());
                }
                Err(e) => {
                    failed.try_set(true);
                    status.try_set(format!(
                        "{e} Unsaved records remain in this tab. Retry before closing."
                    ));
                }
            }
            busy.try_set(false);
        });
    });
    on_cleanup(dispose_log);
    let export = move |_| {
        busy.set(true);
        spawn_local(async move {
            let result = JsFuture::from(download_adif(incomplete.get_untracked())).await;
            match result {
                Ok(message) => {
                    status.try_set(message.as_string().unwrap_or_default());
                }
                Err(e) => {
                    status.try_set(format!("ADIF export failed: {}", error(e)));
                }
            }
            busy.try_set(false);
        });
    };
    let key_tab = move |event: web_sys::KeyboardEvent| {
        match event.key().as_str() {
            "ArrowLeft" | "ArrowRight" => evidence_tab.update(|v| *v = !*v),
            "Home" => evidence_tab.set(false),
            "End" => evidence_tab.set(true),
            _ => return,
        }
        event.prevent_default();
        let id = if evidence_tab.get_untracked() {
            "log-evidence-tab"
        } else {
            "log-contacts-tab"
        };
        if let Some(element) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(id))
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = element.focus();
        }
    };
    view! {
        <Show when=move || visible.get()>
        <section class="results contact-log" aria-label="Local contact log">
            <div class="toolbar"><h2>"Contact log"</h2><span>{move || format!("{} attempts · {} unsaved updates", page.with(|p| p.total), queue.with(|q| q.log_pending.len()))}</span></div>
            <p>"Operator contact attempts, stored locally in this browser/site. Completed means the software exchange ended; local playback does not confirm RF delivery or a mutual QSO. Download backups before clearing browser data."</p>
            <p role="status" class:error=move || failed.get()>{move || status.get()}</p>
            <p class="transmit-validation" hidden=move || queue.with(|q| q.log_pending.len() < 200)>"Contact log backlog is full. New attempts are blocked until records are saved. Retry saving."</p>
            <div class="toolbar">
                <button class="secondary" disabled=move || busy.get() on:click=move |_| { failed.set(false); refresh.update(|n| *n += 1); }>{move || if failed.get() { "Retry saving / reload log" } else { "Refresh log" }}</button>
                <label><input type="checkbox" prop:checked=move || incomplete.get() on:change=move |ev| incomplete.set(event_target_checked(&ev)) />"Include incomplete attempts in ADIF"</label>
                <button disabled=move || busy.get() || failed.get() || queue.with(|q| !q.log_pending.is_empty()) || page.with(|p| p.total == 0) on:click=export>"Download ADIF"</button>
            </div>
            <div class="log-tabs" role="tablist" aria-label="Contact log views">
                <button id="log-contacts-tab" role="tab" aria-controls="log-contacts" aria-selected=move || (!evidence_tab.get()).to_string() tabindex=move || if evidence_tab.get() { -1 } else { 0 } on:keydown=key_tab on:click=move |_| evidence_tab.set(false)>"Contacts"</button>
                <button id="log-evidence-tab" role="tab" aria-controls="log-evidence" aria-selected=move || evidence_tab.get().to_string() tabindex=move || if evidence_tab.get() { 0 } else { -1 } on:keydown=key_tab on:click=move |_| evidence_tab.set(true)>"Evidence"</button>
            </div>
            <div id="log-contacts" role="tabpanel" aria-labelledby="log-contacts-tab" hidden=move || evidence_tab.get()>
                <p hidden=move || page.with(|p| !p.records.is_empty())>"No contact attempts recorded yet. Records start when an armed queue schedules a contact."</p>
                <div class="table-scroll"><table><thead><tr><th>"Start UTC"</th><th>"Last event UTC"</th><th>"Station"</th><th>"Our station"</th><th>"Grid"</th><th>"RF dial MHz"</th><th>"Outcome"</th><th>"Evidence"</th></tr></thead><tbody>
                    {move || page.get().records.into_iter().map(|r| {
                        let id = r.id.clone();
                        view! { <tr><td>{utc(r.start_ms)}</td><td>{utc(r.end_ms)}</td><td>{r.target}</td><td>{r.station_call}</td><td>{r.grid.unwrap_or_else(|| "Unknown".into())}</td><td title=r.frequency_read_ms.map(|ms| format!("Radio queried at {}",utc(ms))).unwrap_or_default()>{r.dial_hz.map(|hz| format!("{:.6}", hz as f64/1_000_000.0)).unwrap_or_else(|| "Unknown".into())}</td><td>{r.outcome}</td><td><button class="secondary" on:click=move |_| { selected_id.set(Some(id.clone())); evidence_tab.set(true); }>{format!("View {} events", r.evidence.len())}</button></td></tr> }
                    }).collect_view()}
                </tbody></table></div>
                <div class="toolbar"><button class="secondary" disabled=move || busy.get() || offset.get() == 0 on:click=move |_| { selected_id.set(None); offset.update(|n| *n = n.saturating_sub(25)); }>"Newer"</button>
                    <span>{move || format!("{}–{} of {}", if page.with(|p| p.total) == 0 { 0 } else { offset.get()+1 }, (offset.get()+25).min(page.with(|p| p.total)), page.with(|p| p.total))}</span>
                    <button class="secondary" disabled={move || busy.get() || offset.get()+25 >= page.with(|p| p.total)} on:click=move |_| { selected_id.set(None); offset.update(|n| *n += 25); }>"Older"</button></div>
            </div>
            <div id="log-evidence" role="tabpanel" aria-labelledby="log-evidence-tab" hidden=move || !evidence_tab.get()>
                {move || selected.get().map(|r| view! {
                    <h3>{format!("{} ↔ {} · {}", r.station_call, r.target, r.outcome)}</h3>
                    <p>"RX times are captured transmission estimates; Decode available UTC is when the result reached this app. TX audio times describe successful local playback only. Earlier received messages provide context. RF dial frequency uses a fresh radio query when connected; unknown readbacks are left blank in ADIF."</p>
                    <div class="table-scroll"><table><thead><tr><th>"Start UTC"</th><th>"End UTC"</th><th>"Decode available UTC"</th><th>"Evidence"</th><th>"Message"</th><th>"SNR dB"</th><th>"Audio Hz"</th><th>"Payload"</th></tr></thead><tbody>
                        {r.evidence.into_iter().map(|e| view! { <tr><td>{utc(e.start_ms)}</td><td>{utc(e.end_ms)}</td><td>{e.received_at_ms.map(utc).unwrap_or_default()}</td><td>{e.direction}</td><td class="message">{e.message}</td><td>{e.snr_db.map(|n| format!("{n:+.1}")).unwrap_or_default()}</td><td>{format!("{:.1}",e.frequency_hz)}</td><td class="log-payload">{e.payload.unwrap_or_default()}</td></tr> }).collect_view()}
                    </tbody></table></div>
                }.into_any()).unwrap_or_else(|| view! { <p>"Choose View events on a contact to inspect its timestamped evidence."</p> }.into_any())}
            </div>
        </section>
        </Show>
    }
}

#[derive(Deserialize, Default)]
struct ExclusionPreference {
    days: Option<u32>,
    error: String,
}
pub fn saved_exclusion() -> (Option<u32>, String) {
    match serde_json::from_str::<ExclusionPreference>(&load_exclusion()) {
        Ok(p) => (p.days, p.error),
        Err(e) => (
            None,
            format!("Recontact setting could not be restored: {e}"),
        ),
    }
}
pub fn save_exclusion_days(days: Option<u32>) -> String {
    save_exclusion(&serde_json::to_string(&days).expect("serializable interval"))
}
