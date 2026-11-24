//! Native adapter over the exact station queue shared with Leptos.
use crate::{
    hardware::{self, Hardware, Request, Snapshot},
    output,
    preferences::Preferences,
    runtime::utc_ms,
};
use eframe::egui::{self, Color32, RichText};
use std::{collections::BTreeSet, time::Instant};
use wsjt_receiver::{
    contact_queue::StationQueue,
    conversations::Conversation,
    history::History,
    open_frequency::{Occupancy, recent_choice},
    receiver::TimedDecode,
};
use wsjt_rig_native::{Command, Mode};

pub struct Stations {
    pub prefs: Preferences,
    pub queue: StationQueue,
    pub snapshot: Snapshot,
    hardware: Hardware,
    pub armed: bool,
    preparing: bool,
    preparation: u64,
    pub playing: bool,
    current: Option<u64>,
    canceling: bool,
    pub status: String,
    pub storage: String,
    outputs: Vec<String>,
    ports: Vec<String>,
    pub log: crate::logbook::Logbook,
    frequency_queries: BTreeSet<String>,
    frequency_query: Option<String>,
    occupancy: Occupancy,
    choice: Option<(f64, f32)>,
    last_tick: Instant,
    last_wall: f64,
}
impl Stations {
    pub fn new() -> Self {
        let (prefs, error) = Preferences::load();
        let mut queue = StationQueue::default();
        queue.require_receive_progress();
        queue.log_session = format!(
            "native-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        Self {
            prefs,
            queue,
            snapshot: Snapshot::default(),
            hardware: Hardware::new(),
            armed: false,
            preparing: false,
            preparation: 0,
            playing: false,
            current: None,
            canceling: false,
            status: "Select conversations and configure your station before Start contacting."
                .into(),
            storage: error.unwrap_or_default(),
            outputs: output::outputs().unwrap_or_default(),
            ports: ports(),
            log: crate::logbook::Logbook::new(
                crate::preferences::directory().map(|p| p.join("contacts.sqlite3")),
            ),
            frequency_queries: BTreeSet::new(),
            frequency_query: None,
            occupancy: Occupancy::new(100.0, 4000.0, crate::runtime::WIDTH),
            choice: None,
            last_tick: Instant::now(),
            last_wall: utc_ms(),
        }
    }
    pub fn stop(&mut self, reason: &str) {
        self.hardware.disarm();
        self.queue.stop_with_reason(reason);
        self.armed = false;
        self.preparing = false;
        self.preparation = 0;
        self.playing = false;
        self.current = None;
        self.canceling = false;
        self.status = reason.into();
    }
    pub fn reset(&mut self) {
        self.stop("Receive restarted; contacting paused.");
        self.queue.reset_observations();
        self.queue.require_receive_progress();
        self.occupancy = Occupancy::new(100.0, 4000.0, crate::runtime::WIDTH);
        self.choice = None;
    }
    fn send(&mut self, request: Request) {
        if let Err(error) = self.hardware.send(request) {
            self.stop(&error);
        }
    }
    pub fn hear(&mut self, row: &TimedDecode, arrival: Option<f64>) {
        self.queue.hear(
            &self.prefs.callsign,
            row.start_utc_ms,
            &row.decode.message,
            &row.decode.payload,
            row.decode.snr_db,
            row.decode.frequency_hz,
        );
        self.queue.record_arrival(
            row.start_utc_ms,
            &row.decode.message,
            arrival.unwrap_or_else(utc_ms),
        );
    }
    pub fn spectrum(&mut self, ms: f64, levels: &[f32], receive_only: bool) {
        if receive_only {
            self.occupancy.push(ms, levels);
        } else {
            self.occupancy.expire(ms);
        }
        self.choice = self.occupancy.choice();
    }
    pub fn select(&mut self, c: &Conversation, history: &History) {
        let target = if c.caller == self.prefs.callsign {
            c.partner.as_ref().unwrap_or(&c.caller)
        } else {
            &c.caller
        };
        self.queue.add(target.clone());
        self.queue.to_front(target);
        self.queue.remember_conversation(c);
        for row in history.decodes.iter().rev() {
            self.hear(row, history.arrived_at(row));
        }
        self.status = format!(
            "{} selected; adding a station does not start transmission.",
            target
        );
    }
    fn add_cqs(&mut self, history: &History) {
        if !self.log.ready {
            self.status = "Load the contact log before bulk/automatic additions.".into();
            return;
        }
        let added = self.queue.add_cq_stations(
            &history.conversations.items,
            utc_ms(),
            &self.log.page.contacted,
            wsjt_log::exclusion_since(self.prefs.exclude_days, utc_ms()),
            &self.prefs.callsign,
        );
        if added > 0 {
            for row in history.decodes.iter().rev() {
                self.hear(row, history.arrived_at(row));
            }
        }
    }
    pub fn was_contacted(&self, call: &str) -> bool {
        self.queue.was_contacted(
            call,
            &self.log.page.contacted,
            wsjt_log::exclusion_since(self.prefs.exclude_days, utc_ms()),
        )
    }
    pub fn configuration_locked(&self) -> bool {
        self.armed || self.preparing
    }
    pub fn priorities(&self) -> Vec<f32> {
        self.queue.priorities()
    }
    pub fn tick(&mut self, running: bool, replay: bool, history: &History) {
        self.log.tick(&mut self.queue);
        let events: Vec<_> = self.hardware.events.try_iter().collect();
        for event in events {
            match event {
                hardware::Event::Radio(snapshot) => {
                    self.snapshot = snapshot;
                    if self.prefs.use_radio
                        && !self.snapshot.ptt_ready
                        && (self.armed || self.preparing)
                    {
                        self.stop("Radio disconnected; contacting paused.");
                    }
                }
                hardware::Event::ContactFrequency(id, sample) => {
                    self.frequency_query = None;
                    if let Some(sample) = sample {
                        self.queue.record_completed_frequency(&id, sample);
                    }
                }
                hardware::Event::Armed(token) => {
                    if self.preparing && self.preparation == token && running && !replay {
                        self.preparing = false;
                        self.armed = true;
                        self.queue.enabled = true;
                        self.status =
                            "Contacting enabled; listening for an eligible fresh CQ.".into();
                    }
                }
                hardware::Event::Disarmed(token) => {
                    if token == self.preparation {
                        self.armed = false;
                        self.queue.stop();
                    }
                }
                hardware::Event::Playing(id) => {
                    if self.current == Some(id) {
                        self.playing = true;
                        self.status = "Playing FT8 audio · RF delivery is not confirmed.".into();
                    }
                }
                hardware::Event::Complete(id) => {
                    if self.current == Some(id) {
                        self.queue.complete(id, true);
                        self.current = None;
                        self.playing = false;
                        self.canceling = false;
                        self.hardware.authority.revoke();
                        self.status = "Audio completed; receive requested.".into();
                    }
                }
                hardware::Event::Canceled(id) => {
                    if self.current == Some(id) {
                        self.current = None;
                        self.playing = false;
                        self.queue.revoke_pending(id);
                        if !self.canceling {
                            self.stop("Transmit authorization interrupted; press Start contacting to resume.");
                        }
                        self.canceling = false;
                    }
                }
                hardware::Event::Error(error) => self.stop(&error),
                hardware::Event::Status(status) => self.status = status,
            }
        }
        let now = utc_ms();
        let monotonic = self.last_tick.elapsed().as_secs_f64() * 1000.0;
        if (self.armed || self.preparing)
            && ((now - self.last_wall - monotonic).abs() > 500.0 || monotonic > 1000.0)
        {
            self.stop("Clock or UI interruption; contacting disarmed.");
        }
        self.last_tick = Instant::now();
        self.last_wall = now;
        if (!running || replay) && (self.armed || self.preparing) {
            self.stop("Live receive stopped; contacting disarmed.");
        }
        if running {
            self.queue.sort_preferences = self.prefs.sort;
            self.queue.find_open_frequency = self.prefs.open_frequency;
            self.queue.sort_cycle(now, &self.prefs.grid);
            if self.prefs.expire {
                self.queue.expire_waiting(now, self.prefs.timeout);
            }
            if self.prefs.auto_add {
                self.add_cqs(history);
            }
        }
        self.queue.advance_time(now);
        self.queue.refresh_pending(now);
        if let Some(id) = self.current {
            let pending = self.queue.pending.as_ref().is_some_and(|p| p.id == id);
            if pending
                && self.queue.enabled
                && (self.playing || self.queue.can_transmit(id))
                && !self.canceling
            {
                self.hardware.authority.approve(id);
            } else if !self.canceling {
                self.hardware.authority.revoke();
                self.canceling = true;
                self.send(Request::Cancel);
            }
        }
        let completed = self.queue.completed_record_ids();
        self.frequency_queries.retain(|id| completed.contains(id));
        if self.current.is_none()
            && self.frequency_query.is_none()
            && self.snapshot.connected
            && now.rem_euclid(15000.0) < 12000.0
            && let Some(id) = completed
                .into_iter()
                .find(|id| !self.frequency_queries.contains(id))
        {
            self.frequency_queries.insert(id.clone());
            self.frequency_query = Some(id.clone());
            self.send(Request::ReadContactFrequency(id));
        }
        if self.armed && !self.queue.enabled {
            self.stop("Contact queue paused.");
        }
        if self.armed
            && self.current.is_none()
            && self.frequency_query.is_none()
            && running
            && !replay
        {
            self.queue.open_frequency_hz = recent_choice(self.choice, now);
            if let Some(plan) = self.queue.plan(now, &self.prefs.callsign, &self.prefs.grid) {
                if !self.queue.can_transmit(plan.id) {
                    self.queue.revoke_pending(plan.id);
                    return;
                }
                let sample = self
                    .snapshot
                    .frequency
                    .zip(Some(self.snapshot.frequency_read_ms))
                    .filter(|(_, read)| (0.0..=5000.0).contains(&(now - read)));
                self.queue.record_frequency(plan.id, sample);
                self.current = Some(plan.id);
                self.hardware.authority.approve(plan.id);
                self.status = format!("{} · scheduled {}", plan.target, plan.message);
                self.send(Request::Schedule(
                    plan,
                    self.prefs.attenuation,
                    self.prefs.use_radio,
                ));
            }
        }
    }
    pub fn panel(&mut self, ui: &mut egui::Ui, history: &History, running: bool, replay: bool) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Stations to contact");
            if self.armed || self.preparing {
                if ui.button("Stop contacting").clicked() {
                    self.stop("Contacting paused by operator.");
                }
            } else if ui
                .add_enabled(
                    running && !replay && !self.queue.items.is_empty() && self.log.ready,
                    egui::Button::new("Start contacting"),
                )
                .clicked()
            {
                match self.prefs.validate_station().and_then(|_| {
                    if self.prefs.use_radio && !self.snapshot.ptt_ready {
                        Err(
                            "Connect/configure radio PTT first, or explicitly choose audio only."
                                .into(),
                        )
                    } else {
                        Ok(())
                    }
                }) {
                    Ok(()) => {
                        self.preparing = true;
                        self.status = "Preparing selected native audio output…".into();
                        match self.hardware.prepare(self.prefs.output.clone()) {
                            Ok(token) => self.preparation = token,
                            Err(error) => self.stop(&error),
                        }
                    }
                    Err(error) => self.status = error,
                }
            }
            if ui.button("Add all CQ stations").clicked() {
                self.add_cqs(history);
            }
            if ui.button("Remove all").clicked() {
                self.stop("Queue cleared by operator.");
                self.queue.clear();
            }
            ui.label(format!("{} / 100 queued", self.queue.items.len()));
        });
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.prefs.auto_add, "Automatically add CQ stations");
            ui.label("Sort each slot:");
            for (i, label) in ["CQ first", "Strongest", "Nearest"].into_iter().enumerate() {
                ui.checkbox(&mut self.prefs.sort[i], label);
            }
        });
        ui.label(RichText::new(self.queue.state_label()).strong());
        ui.label(self.queue.policy_label());
        ui.colored_label(
            if self.playing {
                Color32::YELLOW
            } else {
                Color32::LIGHT_BLUE
            },
            &self.status,
        );
        if replay {
            ui.label("WAV playback is receive-only. Select live input for contacting.");
        }
        ui.label("Click a conversation to queue its caller. Successful local audio is separate from received evidence and RF/QSO confirmation.");
        if self.queue.items.is_empty() {
            ui.add_space(30.0);
            ui.label("No stations selected. Open Conversations and select a calling station.");
        }
        let mut action = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let columns = (ui.available_width() / 340.0).floor().max(1.0) as usize;
            egui::Grid::new("stations")
                .spacing(egui::vec2(12.0, 12.0))
                .show(ui, |ui| {
                    for (i, s) in self.queue.items.iter().enumerate() {
                        let active = self.queue.active.as_ref().filter(|a| a.contact.target == s.call);
                        let pending = self.queue.pending.as_ref().filter(|p| p.target == s.call);
                        let transmitting = self.playing && pending.is_some();
                        let slots = self.queue.exchange_slots(&s.call, utc_ms());
                        let local_audio = wsjt_receiver::contact_queue::transmit_stages(s, pending, active.map(|a| a.contact.step()), transmitting);
                        let tint = slots.iter().rev().find(|slot| slot.evidence.is_some()).map(crate::station_visuals::stage_color).unwrap_or(crate::ui::color(s.kind.color()));
                        crate::station_visuals::card_frame(active.is_some(), transmitting, s.sent_messages.iter().any(Option::is_some), tint, ui.input(|i|i.time))
                            .show(ui, |ui| {
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.set_width(310.0);
                                    ui.horizontal(|ui| {
                                        ui.heading(&s.call);
                                        ui.label(format!("Priority {}", i + 1));
                                    });
                                    if let Some(plan) = pending {
                                        ui.label(RichText::new(if transmitting { "TX AUDIO ON" } else { "TX queued / finishing" }).size(17.0).strong().color(crate::station_visuals::AMBER));
                                        ui.label(RichText::new(&plan.message).monospace().color(crate::station_visuals::AMBER));
                                    } else if active.is_some() {
                                        ui.colored_label(crate::station_visuals::CYAN, "Listening · waiting for partner's reply");
                                    }
                                    ui.label(s.state(utc_ms()));
                                    ui.label(self.queue.expected_action(&s.call));
                                    let now = utc_ms();
                                    ui.label(
                                        s.last_cq_ms
                                            .map(|ms| {
                                                format!(
                                                    "Last CQ {:.0}s ago",
                                                    ((now - ms - 12640.0) / 1000.0).max(0.0)
                                                )
                                            })
                                            .unwrap_or_else(|| "CQ not heard this session".into()),
                                    );
                                    if let (Some(start), Some(arrival)) =
                                        (s.last_cq_ms, s.last_cq_arrival_ms)
                                    {
                                        let deadline =
                                            ((start / 15000.0).round() + 1.0) * 15000.0 + 300.0;
                                        ui.label(format!(
                                            "First decode delay {:.1}s · {}",
                                            ((arrival - start - 12640.0) / 1000.0).max(0.0),
                                            if arrival > deadline {
                                                "reply deadline missed"
                                            } else {
                                                "within reply deadline"
                                            }
                                        ));
                                    }
                                    ui.label(format!(
                                        "RX {:.1} Hz · report {:+03} dB · {} initial calls sent",
                                        s.frequency_hz,
                                        s.snr_db.round().clamp(-50.0, 49.0) as i32,
                                        s.contact_tries
                                    ));
                                    let active = self
                                        .queue
                                        .active
                                        .as_ref()
                                        .filter(|a| a.contact.target == s.call);
                                    let placement = active
                                        .and_then(|a| a.transmit_frequency_hz)
                                        .or_else(|| {
                                            if self.prefs.open_frequency && active.is_none() {
                                                None
                                            } else {
                                                Some(s.frequency_hz)
                                            }
                                        });
                                    ui.label(
                                        placement
                                            .map(|hz| format!("TX lowest tone {hz:.1} Hz"))
                                            .unwrap_or_else(|| {
                                                "TX tone pending spectrum placement".into()
                                            }),
                                    );
                                    if let Some(a) = active {
                                        ui.label(format!(
                                            "{} · {} / 8 transmissions",
                                            a.contact.label(),
                                            a.transmissions
                                        ));
                                    }
                                    ui.label(&s.latest_text);
                                    for (slot, tx) in slots.iter().zip(&local_audio) {
                                        crate::station_visuals::stage(ui, slot, tx);
                                    }
                                    ui.label(RichText::new("Pulsing amber: transmitting · Steady cyan: sent audio. RX stages remain received evidence only.").small().weak());
                                    ui.horizontal(|ui| {
                                        for (key, label) in
                                            [(0, "Front"), (1, "Back"), (2, "Remove")]
                                        {
                                            if ui.button(label).clicked() {
                                                action = Some((key, s.call.clone()));
                                            }
                                        }
                                    });
                                });
                            });
                        if (i + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
        if let Some((key, call)) = action {
            if key != 0
                && self
                    .queue
                    .active
                    .as_ref()
                    .is_some_and(|a| a.contact.target == call)
            {
                self.hardware.authority.revoke();
            }
            match key {
                0 => self.queue.to_front(&call),
                1 => self.queue.to_back(&call),
                _ => self.queue.remove(&call),
            }
        }
    }
    pub fn configuration(&mut self, ui: &mut egui::Ui) {
        ui.heading("Contact configuration");
        ui.add_enabled_ui(!self.armed && !self.preparing, |ui| {
            ui.horizontal(|ui| {
                ui.label("My standard base callsign");
                ui.text_edit_singleline(&mut self.prefs.callsign);
            });
            self.prefs.callsign = self.prefs.callsign.trim().to_ascii_uppercase();
            ui.horizontal(|ui| {
                ui.label("My four-character grid");
                ui.text_edit_singleline(&mut self.prefs.grid);
            });
            self.prefs.grid = self.prefs.grid.trim().to_ascii_uppercase();
            egui::ComboBox::from_label("Transmit audio output")
                .selected_text(if self.prefs.output.is_empty() {
                    "Choose output"
                } else {
                    &self.prefs.output
                })
                .show_ui(ui, |ui| {
                    for name in &self.outputs {
                        ui.selectable_value(&mut self.prefs.output, name.clone(), name);
                    }
                });
            if ui.button("Refresh audio outputs").clicked() {
                match output::outputs() {
                    Ok(outputs) => self.outputs = outputs,
                    Err(error) => self.status = error,
                }
            }
            ui.checkbox(&mut self.prefs.use_radio, "Use connected radio PTT");
            ui.add(
                egui::Slider::new(&mut self.prefs.attenuation, 0.0..=60.0)
                    .text("TX attenuation dB"),
            );
            ui.checkbox(
                &mut self.prefs.open_frequency,
                "Find an open frequency from received spectra (estimate)",
            );
        });
        ui.checkbox(
            &mut self.prefs.expire,
            "Expire waiting stations after last CQ",
        );
        ui.add(
            egui::DragValue::new(&mut self.prefs.timeout)
                .range(1..=86400)
                .suffix(" seconds"),
        );
        ui.label("Frequency and outgoing report follow received evidence. Open-frequency selection holds its estimate through the attempt.");
        ui.horizontal(|ui| {
            ui.label("Exclude logged completed contacts");
            egui::ComboBox::from_id_salt("exclude-logged-contacts")
                .selected_text(match self.prefs.exclude_days {
                    None => "Always".into(),
                    Some(0) => "Off".into(),
                    Some(days) => format!("Last {days} days"),
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.prefs.exclude_days, None, "Always");
                    ui.selectable_value(
                        &mut self.prefs.exclude_days,
                        Some(7),
                        "For a number of days",
                    );
                    ui.selectable_value(&mut self.prefs.exclude_days, Some(0), "Off");
                });
            if let Some(days) = &mut self.prefs.exclude_days
                && *days > 0
            {
                ui.add(egui::DragValue::new(days).range(1..=36500).suffix(" days"));
            }
        });
        ui.label("Completed software contacts are read from the full SQLite history. Manual selection overrides exclusion. Changing the interval does not cancel queued stations.");
        self.save_ui(ui);
    }
    fn save_ui(&mut self, ui: &mut egui::Ui) {
        if ui.button("Save preferences").clicked() {
            self.storage = match self.prefs.save() {
                Ok(()) => "Preferences saved. Restoration never connects or arms TX.".into(),
                Err(e) => format!("Could not save preferences: {e}"),
            };
        }
        ui.label(&self.storage);
    }
    pub fn radio(&mut self, ui: &mut egui::Ui) {
        ui.heading("IC-718 radio control");
        ui.label("CI-V control · raw RF power 0–255 · CAT PTT requires the newer #82/#83 revision");
        ui.add_enabled_ui(!self.snapshot.connected && !self.preparing, |ui| {
            ui.horizontal(|ui| {
                ui.label("CI-V port");
                ui.text_edit_singleline(&mut self.prefs.port);
            });
            egui::ComboBox::from_label("Available serial ports")
                .selected_text(&self.prefs.port)
                .show_ui(ui, |ui| {
                    for port in &self.ports {
                        ui.selectable_value(&mut self.prefs.port, port.clone(), port);
                    }
                });
            if ui.button("Refresh serial ports").clicked() {
                self.ports = ports();
            }
            egui::ComboBox::from_label("Baud")
                .selected_text(self.prefs.baud.to_string())
                .show_ui(ui, |ui| {
                    for &baud in wsjt_rig_native::BAUD_RATES {
                        ui.selectable_value(&mut self.prefs.baud, baud, baud.to_string());
                    }
                });
            ui.horizontal(|ui| {
                ui.label("CI-V address (hex)");
                ui.text_edit_singleline(&mut self.prefs.address);
            });
            egui::ComboBox::from_label("PTT method")
                .selected_text(self.prefs.ptt.label())
                .show_ui(ui, |ui| {
                    for ptt in crate::preferences::PttMethod::ALL {
                        ui.selectable_value(&mut self.prefs.ptt, ptt, ptt.label());
                    }
                });
            if matches!(
                self.prefs.ptt,
                crate::preferences::PttMethod::SeparateRts
                    | crate::preferences::PttMethod::SeparateDtr
            ) {
                ui.horizontal(|ui| {
                    ui.label("Separate PTT port");
                    ui.text_edit_singleline(&mut self.prefs.ptt_port);
                });
            }
            if ui.button("Connect radio").clicked() {
                match self.prefs.validate_radio() {
                    Ok(()) => self.send(Request::Connect(self.prefs.clone())),
                    Err(e) => self.status = e,
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Disconnect / receive / unkey").clicked() {
                self.stop("Radio disconnected by operator.");
                self.send(Request::Disconnect);
            }
            if ui
                .add_enabled(
                    self.snapshot.ptt_ready && !self.armed && !self.preparing,
                    egui::Button::new("Transmit (30 s max)"),
                )
                .clicked()
            {
                self.send(Request::ManualTransmit(
                    self.hardware.authority.generation(),
                ));
            }
        });
        ui.label(format!(
            "{} · dial {} · mode {} · raw RF power {}",
            if self.snapshot.connected {
                "Connected"
            } else {
                "Disconnected"
            },
            self.snapshot
                .frequency
                .map(|f| format!("{f} Hz"))
                .unwrap_or_else(|| "unknown".into()),
            self.snapshot
                .mode
                .map(|m| m.to_string())
                .unwrap_or_else(|| "unknown".into()),
            self.snapshot
                .power
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".into())
        ));
        ui.add_enabled_ui(self.snapshot.connected && self.current.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.prefs.frequency)
                        .range(30000..=29999999)
                        .suffix(" Hz"),
                );
                egui::ComboBox::from_id_salt("band")
                    .selected_text("FT8 band preset")
                    .show_ui(ui, |ui| {
                        for (name, hz) in [
                            ("80 m", 3573000),
                            ("40 m", 7074000),
                            ("30 m", 10136000),
                            ("20 m", 14074000),
                            ("17 m", 18100000),
                            ("15 m", 21074000),
                            ("12 m", 24915000),
                            ("10 m", 28074000),
                        ] {
                            if ui.button(name).clicked() {
                                self.prefs.frequency = hz;
                            }
                        }
                    });
                if ui.button("Set frequency").clicked() {
                    self.send(Request::Execute(Command::SetFrequency(
                        self.prefs.frequency,
                    )));
                }
                if ui.button("Read frequency").clicked() {
                    self.send(Request::Execute(Command::ReadFrequency));
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Mode")
                    .selected_text(
                        Mode::from_byte(self.prefs.mode)
                            .unwrap_or(Mode::Usb)
                            .to_string(),
                    )
                    .show_ui(ui, |ui| {
                        for mode in Mode::ALL {
                            ui.selectable_value(&mut self.prefs.mode, mode as u8, mode.to_string());
                        }
                    });
                if ui.button("Set mode").clicked() {
                    self.send(Request::Execute(Command::SetMode(
                        Mode::from_byte(self.prefs.mode).unwrap_or(Mode::Usb),
                    )));
                }
                if ui.button("Read mode").clicked() {
                    self.send(Request::Execute(Command::ReadMode));
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.prefs.power, 0..=255).text("Raw RF power"));
                if ui.button("Set RF power").clicked() {
                    self.send(Request::Execute(Command::SetPower(self.prefs.power)));
                }
                if ui.button("Read RF power").clicked() {
                    self.send(Request::Execute(Command::ReadPower));
                }
            });
        });
        ui.label(&self.status);
        self.save_ui(ui);
    }
}
fn ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}
