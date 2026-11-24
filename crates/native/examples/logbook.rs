//! Device-free preview of the real native log UI, isolated from operator history.
#[path = "../src/logbook.rs"]
mod logbook;
use eframe::egui;
use wsjt_log::{ContactRecord, ContactStore, Evidence, Request};
use wsjt_receiver::contact_queue::StationQueue;

struct Preview {
    log: logbook::Logbook,
    queue: StationQueue,
}
impl eframe::App for Preview {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.log.tick(&mut self.queue);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Sample records · no audio, serial ports or transmit authorization");
            self.log.panel(ui, self.queue.log_pending.len());
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
fn main() -> eframe::Result {
    let directory = std::env::temp_dir().join(format!("wsjt-log-preview-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("contacts.sqlite3");
    let mut store = wsjt_log_sqlite::open(&path).unwrap();
    let start = 1_795_391_995_000.0;
    let record = ContactRecord {
        id: "preview-contact".into(),
        revision: 1,
        station_call: "W9XYZ".into(),
        station_grid: "EN37".into(),
        target: "K1ABC".into(),
        grid: Some("FN42".into()),
        start_ms: start,
        end_ms: start + 60_000.0,
        outcome: "Completed".into(),
        dial_hz: None,
        frequency_read_ms: None,
        evidence: vec![
            Evidence {
                start_ms: start,
                end_ms: start + 12_640.0,
                direction: "RX".into(),
                message: "W9XYZ K1ABC -12".into(),
                payload: None,
                snr_db: Some(-8.0),
                frequency_hz: 1200.0,
                received_at_ms: Some(start + 13_500.0),
            },
            Evidence {
                start_ms: start + 15_000.0,
                end_ms: start + 27_640.0,
                direction: "TX audio".into(),
                message: "K1ABC W9XYZ R-08".into(),
                payload: None,
                snr_db: None,
                frequency_hz: 1200.0,
                received_at_ms: None,
            },
        ],
    };
    store
        .request(Request::Save {
            records: vec![record],
            offset: 0,
        })
        .unwrap();
    drop(store);
    eframe::run_native(
        "FT8 log preview — no hardware",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 700.0]),
            ..Default::default()
        },
        Box::new(move |_| {
            Ok(Box::new(Preview {
                log: logbook::Logbook::new(Ok(path)),
                queue: StationQueue::default(),
            }))
        }),
    )
}
