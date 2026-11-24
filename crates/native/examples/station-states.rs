//! Visual QA only: no CPAL streams, serial ports, hardware owner or TX authority.
//! cargo run -p wsjt-native --example station-states --release
#[path = "../src/station_visuals.rs"]
mod station_visuals;
use eframe::egui::{self, RichText};
use wsjt_receiver::contact_queue::{StationQueue, transmit_stages};

struct Preview {
    queue: StationQueue,
    playing: bool,
}
impl Preview {
    fn new() -> Self {
        let mut queue = StationQueue::default();
        queue.add("K1ABC".into());
        queue.enabled = true;
        let payload = |text: &str| {
            wsjt_rusty::ft8::transmit::pack(text)
                .unwrap()
                .iter()
                .map(|b| char::from(b'0' + b))
                .collect::<String>()
        };
        queue.hear(
            "W9XYZ",
            500.0,
            "CQ K1ABC FN42",
            &payload("CQ K1ABC FN42"),
            -12.0,
            1200.0,
        );
        let p = queue.plan(14900.0, "W9XYZ", "EN37").unwrap();
        queue.complete(p.id, true);
        queue.hear(
            "W9XYZ",
            30500.0,
            "W9XYZ K1ABC -12",
            &payload("W9XYZ K1ABC -12"),
            -8.0,
            1200.0,
        );
        queue.plan(44900.0, "W9XYZ", "EN37").unwrap();
        Self {
            queue,
            playing: true,
        }
    }
}
impl eframe::App for Preview {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.set_visuals(egui::Visuals::dark());
        egui::CentralPanel::default().show(ctx,|ui|{
            ui.heading("VISUAL PREVIEW — no audio or radio devices");
            ui.checkbox(&mut self.playing,"Preview transmitting appearance");
            if self.playing{station_visuals::banner(ui,"K1ABC","K1ABC W9XYZ R-08");}
            ui.add_space(16.0);
            ui.horizontal_top(|ui|{
                let s=&self.queue.items[0];let slots=self.queue.exchange_slots(&s.call,44900.0);
                for playing in [false,self.playing]{
                    ui.vertical(|ui|{
                        station_visuals::card_frame(true,playing,true,station_visuals::CYAN,ui.input(|i|i.time)).show(ui,|ui|{
                            ui.set_width(310.0);
                            ui.heading("K1ABC");
                            ui.label(RichText::new(if playing{"TX AUDIO ON"}else{"Listening · waiting for reply"}).strong().color(if playing{station_visuals::AMBER}else{station_visuals::CYAN}));
                            ui.monospace("Grid FN42 · RX -8.0 dB SNR");
                            let tx=transmit_stages(s,if playing{self.queue.pending.as_ref()}else{None},Some(2),playing);
                            for (slot,tx) in slots.iter().zip(tx){station_visuals::stage(ui,slot,&tx);}
                        });
                    });
                    ui.add_space(14.0);
                }
            });
            ui.label("RX labels describe decoded evidence. Local audio is displayed separately; it does not confirm RF delivery.");
        });
        if self.playing {
            station_visuals::window_glow(ctx);
        }
    }
}
fn main() -> eframe::Result {
    eframe::run_native(
        "FT8 station appearance preview — no hardware",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 900.0]),
            ..Default::default()
        },
        Box::new(|_| Ok(Box::new(Preview::new()))),
    )
}
