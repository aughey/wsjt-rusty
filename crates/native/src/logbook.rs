//! Native log resource owner. SQLite and export I/O never run on the egui/audio thread.
use eframe::egui::{self, Color32, RichText};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};
use wsjt_log::{ContactRecord, ContactStore, Page, Request, Response};
use wsjt_receiver::contact_queue::StationQueue;

enum Job {
    Page(usize),
    Save(Vec<ContactRecord>, usize),
    Export(PathBuf, bool),
    Shutdown,
}
enum Event {
    Loaded(Page),
    Saved(Vec<ContactRecord>, Page),
    Exported(PathBuf, usize),
    Failed(String),
    ExportFailed(String),
}
pub struct Logbook {
    jobs: SyncSender<Job>,
    events: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
    pub ready: bool,
    pub busy: bool,
    pub failed: bool,
    pub status: String,
    pub page: Page,
    pub path: PathBuf,
    offset: usize,
    selected: Option<String>,
    evidence_tab: bool,
    include_incomplete: bool,
    export_path: String,
    refresh: bool,
}
impl Logbook {
    pub fn new(path: Result<PathBuf, String>) -> Self {
        let (jobs, rx) = mpsc::sync_channel(8);
        let (events, inbox) = mpsc::sync_channel(8);
        let display = path.as_ref().cloned().unwrap_or_default();
        let worker = thread::spawn(move || owner(path, rx, events));
        let export_path = display
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(format!(
                "wsjt-rusty-contacts-{}.adi",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ))
            .display()
            .to_string();
        Self {
            jobs,
            events: inbox,
            worker: Some(worker),
            ready: false,
            busy: false,
            failed: false,
            status: "Opening local SQLite contact database…".into(),
            page: Page::default(),
            path: display,
            offset: 0,
            selected: None,
            evidence_tab: false,
            include_incomplete: false,
            export_path,
            refresh: true,
        }
    }
    fn send(&mut self, job: Job) {
        match self.jobs.try_send(job) {
            Ok(()) => self.busy = true,
            Err(_) => {
                self.failed = true;
                self.status =
                    "Contact database worker unavailable; unsaved updates remain in memory.".into();
            }
        }
    }
    pub fn retry(&mut self) {
        self.failed = false;
        self.refresh = true;
    }
    pub fn tick(&mut self, queue: &mut StationQueue) {
        for event in self.events.try_iter() {
            self.busy = false;
            match event {
                Event::Loaded(page) => {
                    self.page = page;
                    self.ready = true;
                    self.failed = false;
                    self.status = "Stored locally in SQLite. No upload.".into();
                }
                Event::Saved(records, page) => {
                    queue.acknowledge_log(&records);
                    self.page = page;
                    self.ready = true;
                    self.failed = false;
                    self.status =
                        "Saved locally; RX evidence and successful local TX audio remain separate."
                            .into();
                }
                Event::Exported(path, count) => {
                    self.status = format!("Exported {count} contact records to {}", path.display());
                }
                Event::Failed(error) => {
                    self.failed = true;
                    self.status = format!(
                        "{error} Unsaved log updates are retained. Retry saving before closing."
                    );
                }
                Event::ExportFailed(error) => {
                    self.status = format!("ADIF export failed: {error}");
                }
            }
        }
        if self.busy || self.failed {
            return;
        }
        let records: Vec<_> = queue.log_pending.values().take(200).cloned().collect();
        if !records.is_empty() {
            self.send(Job::Save(records, self.offset));
        } else if self.refresh {
            self.refresh = false;
            self.send(Job::Page(self.offset));
        }
    }
    pub fn panel(&mut self, ui: &mut egui::Ui, pending: usize) {
        ui.heading("Contact log");
        ui.label(format!(
            "{} attempts · {pending} unsaved updates",
            self.page.total
        ));
        ui.label("Completed means the software exchange ended; local audio does not confirm RF delivery or a mutual QSO.");
        ui.label(format!("SQLite database: {}", self.path.display()));
        ui.colored_label(
            if self.failed {
                Color32::LIGHT_RED
            } else {
                Color32::LIGHT_BLUE
            },
            &self.status,
        );
        if pending >= 200 {
            ui.colored_label(
                Color32::YELLOW,
                "Log backlog is full; new attempts wait until saving succeeds.",
            );
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(if self.failed {
                        "Retry saving / reload log"
                    } else {
                        "Refresh log"
                    }),
                )
                .clicked()
            {
                self.retry();
            }
            ui.checkbox(
                &mut self.include_incomplete,
                "Include incomplete attempts in ADIF",
            );
        });
        ui.horizontal(|ui| {
            ui.label("ADIF file");
            ui.add(egui::TextEdit::singleline(&mut self.export_path).desired_width(520.0));
            if ui
                .add_enabled(
                    !self.busy && !self.failed && self.ready && pending == 0 && self.page.total > 0,
                    egui::Button::new("Export ADIF"),
                )
                .clicked()
            {
                self.send(Job::Export(
                    PathBuf::from(self.export_path.trim()),
                    self.include_incomplete,
                ));
            }
        });
        ui.label(
            RichText::new(
                "Exports create a new file. Choose a different path to keep an existing file.",
            )
            .small()
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.evidence_tab, false, "Contacts");
            ui.selectable_value(&mut self.evidence_tab, true, "Evidence");
        });
        if self.evidence_tab {
            self.evidence(ui);
        } else {
            self.contacts(ui);
        }
    }
    fn contacts(&mut self, ui: &mut egui::Ui) {
        let mut selected = None;
        egui::ScrollArea::both().id_salt("contact-log-records").show(ui,|ui|{
            egui::Grid::new("contact-log-rows").striped(true).show(ui,|ui|{
                for label in ["Start UTC","Last event UTC","Station","Our station","Grid","RF dial MHz","Outcome","Evidence"]{ui.strong(label);}ui.end_row();
                for record in &self.page.records{
                    ui.monospace(utc(record.start_ms));ui.monospace(utc(record.end_ms));ui.label(&record.target);ui.label(&record.station_call);
                    ui.label(record.grid.as_deref().unwrap_or("Unknown"));
                    ui.label(record.dial_hz.map(|hz|format!("{:.6}",hz as f64/1_000_000.0)).unwrap_or_else(||"Unknown".into())).on_hover_text(record.frequency_read_ms.map(utc).unwrap_or_else(||"No radio readback; audio Hz and form values are never substituted.".into()));
                    ui.label(&record.outcome);
                    if ui.button(format!("View {} events",record.evidence.len())).clicked(){selected=Some(record.id.clone());}
                    ui.end_row();
                }
            });
            if self.page.records.is_empty(){ui.label("No contact attempts recorded yet. Selecting a station alone does not create a record.");}
        });
        if let Some(id) = selected {
            self.selected = Some(id);
            self.evidence_tab = true;
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy && self.offset > 0, egui::Button::new("Newer"))
                .clicked()
            {
                self.offset = self.offset.saturating_sub(25);
                self.selected = None;
                self.refresh = true;
            }
            ui.label(format!(
                "{}–{} of {}",
                if self.page.total == 0 {
                    0
                } else {
                    self.offset + 1
                },
                (self.offset + 25).min(self.page.total),
                self.page.total
            ));
            if ui
                .add_enabled(
                    !self.busy && self.offset + 25 < self.page.total,
                    egui::Button::new("Older"),
                )
                .clicked()
            {
                self.offset += 25;
                self.selected = None;
                self.refresh = true;
            }
        });
    }
    fn evidence(&self, ui: &mut egui::Ui) {
        let Some(record) = self
            .page
            .records
            .iter()
            .find(|r| Some(&r.id) == self.selected.as_ref())
        else {
            ui.label(
                "Choose View events on a contact to inspect received evidence and local audio.",
            );
            return;
        };
        ui.heading(format!(
            "{} ↔ {} · {}",
            record.station_call, record.target, record.outcome
        ));
        ui.label("RX times are transmission estimates; Decode available UTC is first arrival. TX audio records successful local playback, not RF confirmation.");
        egui::ScrollArea::both()
            .id_salt("contact-evidence")
            .show(ui, |ui| {
                egui::Grid::new("contact-evidence-rows")
                    .striped(true)
                    .show(ui, |ui| {
                        for label in [
                            "Start UTC",
                            "End UTC",
                            "Decode available UTC",
                            "Evidence",
                            "Message",
                            "SNR dB",
                            "Audio Hz",
                            "Payload",
                        ] {
                            ui.strong(label);
                        }
                        ui.end_row();
                        for event in &record.evidence {
                            ui.monospace(utc(event.start_ms));
                            ui.monospace(utc(event.end_ms));
                            ui.monospace(event.received_at_ms.map(utc).unwrap_or_default());
                            ui.colored_label(
                                if event.direction == "RX" {
                                    Color32::LIGHT_GREEN
                                } else {
                                    Color32::LIGHT_BLUE
                                },
                                &event.direction,
                            );
                            ui.monospace(&event.message);
                            ui.label(
                                event
                                    .snr_db
                                    .map(|snr| format!("{snr:+.1}"))
                                    .unwrap_or_default(),
                            );
                            ui.label(format!("{:.1}", event.frequency_hz));
                            ui.monospace(event.payload.as_deref().unwrap_or(""));
                            ui.end_row();
                        }
                    });
            });
    }
}
impl Drop for Logbook {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn utc(ms: f64) -> String {
    wsjt_log::utc(ms).unwrap_or_else(|_| "Invalid UTC".into())
}
fn owner(path: Result<PathBuf, String>, jobs: Receiver<Job>, events: SyncSender<Event>) {
    let mut store = None;
    for job in jobs {
        if matches!(job, Job::Shutdown) {
            break;
        }
        let exporting = matches!(&job, Job::Export(..));
        let result = (|| {
            if store.is_none() {
                let path = path.as_ref().map_err(Clone::clone)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                store = Some(wsjt_log_sqlite::open(path).map_err(|e| e.to_string())?);
            }
            let store = store.as_mut().unwrap();
            match job {
                Job::Page(offset) => match store
                    .request(Request::List { offset })
                    .map_err(|e| e.to_string())?
                {
                    Response::Page(page) => Ok(Event::Loaded(page)),
                    _ => Err("Invalid database response".into()),
                },
                Job::Save(records, offset) => match store
                    .request(Request::Save {
                        records: records.clone(),
                        offset,
                    })
                    .map_err(|e| e.to_string())?
                {
                    Response::Page(page) => Ok(Event::Saved(records, page)),
                    _ => Err("Invalid database response".into()),
                },
                Job::Export(path, incomplete) => {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|e| format!("ADIF file {}: {e}", path.display()))?;
                    match store
                        .write_adif(incomplete, &mut file)
                        .map_err(|e| e.to_string())
                        .and_then(|count| file.sync_all().map(|_| count).map_err(|e| e.to_string()))
                    {
                        Ok(count) => Ok(Event::Exported(path, count)),
                        Err(error) => {
                            drop(file);
                            let _ = std::fs::remove_file(&path);
                            Err(error)
                        }
                    }
                }
                Job::Shutdown => unreachable!(),
            }
        })();
        let _ = events.try_send(result.unwrap_or_else(|e| {
            if exporting {
                Event::ExportFailed(e)
            } else {
                Event::Failed(e)
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "wsjt-native-log-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn queued() -> StationQueue {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        q.enabled = true;
        let payload = wsjt_rusty::ft8::transmit::pack("CQ K1ABC FN42")
            .unwrap()
            .iter()
            .map(|b| char::from(b'0' + b))
            .collect::<String>();
        q.hear("W9XYZ", 500.0, "CQ K1ABC FN42", &payload, -12.0, 1200.0);
        q.plan(14900.0, "W9XYZ", "EN37").unwrap();
        q
    }
    fn settle(log: &mut Logbook, q: &mut StationQueue) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            log.tick(q);
            if !log.busy && (log.failed || q.log_pending.is_empty()) {
                break;
            }
            assert!(Instant::now() < deadline, "{}", log.status);
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn native_persistence_acknowledges_only_committed_revisions_and_survives_restart() {
        let dir = directory();
        let path = dir.join("contacts.sqlite3");
        let mut q = queued();
        let mut log = Logbook::new(Ok(path.clone()));
        log.tick(&mut q);
        assert!(log.busy);
        assert_eq!(q.log_pending.len(), 1);
        let pending = q.log_pending.values_mut().next().unwrap();
        pending.revision += 1;
        pending.outcome = "Receive stopped: input gap".into();
        settle(&mut log, &mut q);
        assert!(!log.failed, "{}", log.status);
        assert_eq!(log.page.total, 1);
        assert!(q.log_pending.is_empty());
        assert_eq!(log.page.records[0].outcome, "Receive stopped: input gap");
        drop(log);
        let mut log = Logbook::new(Ok(path.clone()));
        let mut q = StationQueue::default();
        settle(&mut log, &mut q);
        assert_eq!(log.page.total, 1);
        assert_eq!(log.page.records[0].evidence.len(), 1);
        drop(log);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn failed_save_retains_records_and_retry_recovers_without_new_tx() {
        let dir = directory();
        std::fs::create_dir(&dir).unwrap();
        let blocker = dir.join("blocked");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let path = blocker.join("contacts.sqlite3");
        let mut log = Logbook::new(Ok(path.clone()));
        let mut q = queued();
        settle(&mut log, &mut q);
        assert!(log.failed);
        assert_eq!(q.log_pending.len(), 1);
        q.stop();
        std::fs::remove_file(&blocker).unwrap();
        log.retry();
        settle(&mut log, &mut q);
        assert!(!log.failed, "{}", log.status);
        assert!(!q.enabled);
        assert!(q.log_pending.is_empty());
        assert_eq!(log.page.records[0].outcome, "Interrupted");
        let existing = dir.join("backup.adi");
        std::fs::write(&existing, b"keep this backup").unwrap();
        log.send(Job::Export(existing.clone(), true));
        settle(&mut log, &mut q);
        assert!(
            !log.failed,
            "an export filename error must not stop contact saving"
        );
        assert_eq!(std::fs::read(&existing).unwrap(), b"keep this backup");
        drop(log);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(blocker).unwrap();
        std::fs::remove_file(existing).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
