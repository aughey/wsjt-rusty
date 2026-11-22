//! Browser map binding. No location or network dependencies enter receiver DSP.
use crate::{contact_queue::StationQueue, conversations::Conversations, geography::Location};
use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/map-runtime.js")]
extern "C" {
    #[wasm_bindgen(js_name = updateContactMap)]
    fn update_map(json: &str);
    #[wasm_bindgen(js_name = disposeContactMap)]
    fn dispose_map();
}

#[derive(Clone, PartialEq, Serialize)]
struct Marker {
    call: String,
    location: Location,
    color: String,
    detail: String,
}

#[derive(Clone, PartialEq, Serialize)]
struct Link {
    from: String,
    to: String,
    start: Location,
    end: Location,
    color: String,
    inferred: bool,
}
#[derive(Serialize)]
struct MapData {
    markers: Vec<Marker>,
    links: Vec<Link>,
}

#[component]
pub fn ContactMap(
    conversations: ReadSignal<Conversations>,
    queue: RwSignal<StationQueue>,
    grid: RwSignal<String>,
    mine: RwSignal<String>,
) -> impl IntoView {
    let shown = RwSignal::new(false);
    let markers = Memo::new(move |_| {
        // Per-station latest grid survives message trimming, bounded by retained cards.
        let mut points = std::collections::BTreeMap::new();
        conversations.with(|t| {
            for c in &t.items {
                for (call, (ms, l)) in &c.locations {
                    let marker = Marker {
                        call: call.clone(),
                        location: l.clone(),
                        color: c.color(t.now_ms).into(),
                        detail: crate::geography::description(Some(l), &grid.get()),
                    };
                    if points.get(call).is_none_or(|(old, _)| ms >= old) {
                        points.insert(call.clone(), (*ms, marker));
                    }
                }
            }
        });
        queue.with(|q| {
            for s in &q.items {
                if let Some((ms, l)) = &s.location
                    && points.get(&s.call).is_none_or(|(old, _)| ms >= old)
                {
                    points.insert(
                        s.call.clone(),
                        (
                            *ms,
                            Marker {
                                call: s.call.clone(),
                                location: l.clone(),
                                color: s.kind.color().into(),
                                detail: crate::geography::description(Some(l), &grid.get()),
                            },
                        ),
                    );
                }
            }
        });
        let home = Location::parse(&grid.get()).map(|location| Marker {
            call: "My station".into(),
            detail: crate::geography::description(Some(&location), &grid.get()),
            location,
            color: "#ffffff".into(),
        });
        let mut markers: Vec<_> = points.into_values().map(|(_, m)| m).collect();
        markers.extend(home);
        markers
    });
    let links = Memo::new(move |_| {
        let mut locations: std::collections::BTreeMap<_, _> = markers
            .get()
            .into_iter()
            .map(|m| (m.call, m.location))
            .collect();
        if let Some(home) = Location::parse(&grid.get()) {
            locations.insert(mine.get(), home);
        }
        let mut links = std::collections::BTreeMap::new();
        conversations.with(|t| {
            for c in &t.items {
                if let Some(partner) = &c.partner
                    && let (Some(start), Some(end)) =
                        (locations.get(&c.caller), locations.get(partner))
                {
                    let mut pair = [c.caller.clone(), partner.clone()];
                    pair.sort();
                    let key = pair.join("/");
                    if links.get(&key).is_none_or(|(ms, _)| c.last_ms >= *ms) {
                        links.insert(
                            key,
                            (
                                c.last_ms,
                                Link {
                                    from: c.caller.clone(),
                                    to: partner.clone(),
                                    start: start.clone(),
                                    end: end.clone(),
                                    color: c.color(t.now_ms).into(),
                                    inferred: c.inferred,
                                },
                            ),
                        );
                    }
                }
            }
        });
        queue.with(|q| {
            if let Some(a) = &q.active
                && let (Some(start), Some(end)) =
                    (locations.get(&mine.get()), locations.get(&a.contact.target))
            {
                links.insert(
                    "local-attempt".into(),
                    (
                        f64::INFINITY,
                        Link {
                            from: mine.get(),
                            to: a.contact.target.clone(),
                            start: start.clone(),
                            end: end.clone(),
                            color: "#ffd078".into(),
                            inferred: true,
                        },
                    ),
                );
            }
        });
        links
            .into_values()
            .map(|(_, link)| link)
            .collect::<Vec<_>>()
    });
    Effect::new(move |_| {
        if !shown.get() {
            return;
        }
        update_map(
            &serde_json::to_string(&MapData {
                markers: markers.get(),
                links: links.get(),
            })
            .unwrap_or_else(|_| "{}".into()),
        );
    });
    on_cleanup(dispose_map);
    view! {
        <section class="results contact-map-panel" aria-label="Contact world map">
            <div class="toolbar"><h2>"Contact world map"</h2>
                <button class="secondary" aria-expanded=move || shown.get().to_string() on:click=move |_| {
                    shown.update(|v| *v = !*v);
                    if !shown.get_untracked() { dispose_map(); }
                }>{move || if shown.get() { "Close map" } else { "Open map" }}</button>
            </div>
            <p>"Decoded grid centers, not exact station positions. Lines connect known endpoints of observed exchanges; dashed lines indicate inferred context or a local attempt, not confirmed RF delivery. Colors follow received exchange stages. Set My grid for approximate distances. Unknown grids are omitted. Opening the map requests tiles from OpenStreetMap."</p>
            <div hidden=move || !shown.get()>
                <div id="contact-map" tabindex="0" aria-label="World map of decoded stations. Arrow keys pan; plus and minus zoom." />
                <p id="contact-map-status" role="status" />
                <p id="contact-map-detail" aria-live="polite">"Select a marker for its callsign, grid, coordinates and distance."</p>
                <p>{move || format!("{} communication paths · unknown endpoints omitted", links.get().len())}</p>
                <details><summary>"Communication paths (text list)"</summary><ul>{move || links.get().into_iter().map(|l| view! { <li>{format!("{} ↔ {} · {}", l.from, l.to, if l.inferred { "Inferred context / local attempt" } else { "Received exchange" })}</li> }).collect_view()}</ul></details>
                <details><summary>"Located stations (text list)"</summary>
                    <ul>{move || markers.get().into_iter().map(|m| view! { <li>{format!("{} · {}", m.call, m.detail)}</li> }).collect_view()}</ul>
                </details>
            </div>
        </section>
    }
}
