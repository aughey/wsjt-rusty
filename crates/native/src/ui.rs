use crate::runtime::{self, Event, Session, WIDTH};
use cpal::traits::DeviceTrait;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use std::{collections::VecDeque, path::Path, time::Duration};
use wsjt_receiver::{
    annotations,
    geography::{self, CardSort},
    history::History,
    message_kind::{self, MessageKind},
    receiver::ReceiveRole,
    spectrum::{self, LevelScale},
};

const NAVY: Color32 = Color32::from_rgb(11, 19, 37);
const MUTED: Color32 = Color32::from_rgb(150, 168, 190);
const ACCENT: Color32 = Color32::from_rgb(117, 237, 145);
const HEIGHT: usize = 600;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Spectrum,
    Messages,
    Conversations,
}

pub struct App {
    tab: Tab,
    devices: Vec<cpal::Device>,
    selected: usize,
    wav: String,
    session: Option<Session>,
    stopping: bool,
    error: Option<String>,
    history: History,
    levels: Vec<f32>,
    rows: VecDeque<Vec<Color32>>,
    clock: Option<(f64, f64)>,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
    scale: LevelScale,
    auto: bool,
    manual_floor: f32,
    floor: f32,
    peak: f32,
    progress: [f64; 2],
    durations: [f64; 2],
    origin: Option<f64>,
    grid: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, wav: Option<String>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.visuals.panel_fill = NAVY;
        style.visuals.selection.bg_fill = Color32::from_rgb(35, 83, 75);
        cc.egui_ctx.set_style(style);
        let devices = runtime::devices();
        let error = devices.as_ref().err().cloned();
        Self {
            tab: Tab::Spectrum,
            devices: devices.unwrap_or_default(),
            selected: 0,
            wav: wav.unwrap_or_default(),
            session: None,
            stopping: false,
            error,
            history: History::default(),
            levels: vec![-160.0; WIDTH],
            rows: VecDeque::new(),
            clock: None,
            texture: None,
            dirty: true,
            scale: LevelScale::default(),
            auto: true,
            manual_floor: -100.0,
            floor: -100.0,
            peak: 0.0,
            progress: [0.0; 2],
            durations: [0.0; 2],
            origin: None,
            grid: String::new(),
        }
    }

    fn start(&mut self) {
        let result = if self.wav.trim().is_empty() {
            self.devices
                .get(self.selected)
                .ok_or_else(|| "Select an available audio input.".to_string())
                .and_then(Session::live)
        } else {
            Session::wav(Path::new(self.wav.trim()))
        };
        match result {
            Ok(session) => {
                self.session = Some(session);
                self.stopping = false;
                self.error = None;
                self.history = History::default();
                self.rows.clear();
                self.clock = None;
                self.levels.fill(-160.0);
                self.dirty = true;
                self.scale = LevelScale::default();
                self.progress = [0.0; 2];
                self.durations = [0.0; 2];
                self.origin = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn poll(&mut self) {
        let events: Vec<_> = self
            .session
            .as_ref()
            .map(|s| s.events.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Partial(decode) => {
                    self.history
                        .ingest(self.progress[0].min(self.progress[1]), vec![decode]);
                }
                Event::Decoded(role, update, duration) => {
                    let lane = usize::from(role == ReceiveRole::Recovery);
                    self.origin = Some(update.capture_start_ms);
                    self.progress[lane] = update.capture_end_ms;
                    if duration.as_secs_f64() > 0.02 {
                        self.durations[lane] = duration.as_secs_f64();
                    }
                    // Age only to the slower processed lane. A busy recovery
                    // search cannot prematurely expire its own pending evidence.
                    let processed = self.progress[0].min(self.progress[1]);
                    self.history.ingest(processed, update.decodes);
                }
                Event::Spectrum {
                    center_ms,
                    levels,
                    peak,
                } => {
                    self.floor = self.scale.floor(&levels, self.manual_floor, self.auto);
                    self.rows.push_front(
                        levels
                            .iter()
                            .map(|&v| {
                                let c = spectrum::color(v, self.floor);
                                Color32::from_rgb(c[0], c[1], c[2])
                            })
                            .collect(),
                    );
                    self.rows.truncate(HEIGHT);
                    self.clock = Some((self.clock.map_or(center_ms, |c| c.0), center_ms));
                    self.levels = levels;
                    self.peak = peak;
                    self.dirty = true;
                }
                Event::Error(error) => {
                    self.error = Some(error);
                    self.stopping = true;
                    if let Some(session) = &mut self.session {
                        session.stop();
                    }
                }
                Event::Finished => {
                    self.session = None;
                    self.stopping = false;
                }
            }
        }
        if let Some(session) = &mut self.session
            && session.failed()
        {
            session.stop();
            self.stopping = true;
            self.error
                .get_or_insert_with(|| "Receive transport failed; input stopped.".into());
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("wsjt-rusty").strong());
            ui.label(RichText::new("NATIVE / FT8").color(MUTED).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(
                    if self.session.is_some() {
                        ACCENT
                    } else {
                        MUTED
                    },
                    if self.stopping {
                        "● Flushing receiver tails"
                    } else if self.session.is_some() {
                        "● Listening"
                    } else {
                        "○ Input stopped"
                    },
                );
            });
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let busy = self.session.is_some();
            ui.add_enabled_ui(!busy, |ui| {
                ui.label("Audio input");
                egui::ComboBox::from_id_salt("input")
                    .selected_text(
                        self.devices
                            .get(self.selected)
                            .and_then(|d| d.name().ok())
                            .unwrap_or_else(|| "No input device".into()),
                    )
                    .width(270.0)
                    .show_ui(ui, |ui| {
                        for (i, device) in self.devices.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected,
                                i,
                                device.name().unwrap_or_else(|_| "Unavailable input".into()),
                            );
                        }
                    });
                if ui.button("Refresh").clicked() {
                    match runtime::devices() {
                        Ok(devices) => {
                            self.devices = devices;
                            self.selected = 0;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
            });
            if !busy
                && ui
                    .button(RichText::new("▶ Start listening").color(ACCENT))
                    .clicked()
            {
                self.start();
            }
            if busy
                && ui
                    .add_enabled(!self.stopping, egui::Button::new("■ Stop listening"))
                    .clicked()
            {
                if let Some(session) = &mut self.session {
                    session.stop();
                }
                self.stopping = true;
            }
            if let Some(session) = &self.session {
                ui.label(format!("{} Hz", session.rate));
            }
            if !self.wav.trim().is_empty() {
                ui.colored_label(ACCENT, "WAV playback")
                    .on_hover_text(&self.wav);
            }
        });
        ui.collapsing("Recorded input", |ui| {
            ui.label("Optional WAV path — played at capture speed through the same receiver. Leave empty for live audio.");
            ui.add_enabled(self.session.is_none(), egui::TextEdit::singleline(&mut self.wav).desired_width(f32::INFINITY).hint_text("Path to recording.wav"));
        });
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (tab, name) in [
                (Tab::Spectrum, "Spectrum analyzer"),
                (Tab::Messages, "Decoded messages"),
                (Tab::Conversations, "Conversations"),
            ] {
                ui.selectable_value(&mut self.tab, tab, name);
            }
        });
    }

    fn spectrum(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.auto, "Automatic display level");
            ui.add_enabled(
                !self.auto,
                egui::Slider::new(&mut self.manual_floor, -150.0..=-20.0).text("Floor dBFS"),
            );
            ui.label(format!(
                "Floor {:.0} dBFS  ·  Input peak {:.1} dBFS",
                self.floor,
                20.0 * self.peak.max(1e-8).log10()
            ));
        });
        ui.label(
            RichText::new(
                "100–4000 audio Hz · 60 seconds · newest at top · display levels are FFT-bin dBFS",
            )
            .color(MUTED)
            .small(),
        );
        let width = ui.available_width();
        let (graph, _) = ui.allocate_exact_size(Vec2::new(width, 90.0), egui::Sense::hover());
        let painter = ui.painter_at(graph);
        painter.rect_filled(graph, 3.0, Color32::from_rgb(7, 13, 25));
        for db in [-120.0, -90.0, -60.0, -30.0, 0.0] {
            let y = graph.bottom() - (db + 160.0) / 160.0 * graph.height();
            painter.hline(
                graph.x_range(),
                y,
                Stroke::new(0.5_f32, Color32::from_gray(40)),
            );
        }
        let points: Vec<_> = self
            .levels
            .iter()
            .enumerate()
            .map(|(i, &db)| {
                Pos2::new(
                    graph.left() + i as f32 / (WIDTH - 1) as f32 * graph.width(),
                    graph.bottom() - (db + 160.0).clamp(0.0, 160.0) / 160.0 * graph.height(),
                )
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(1.0_f32, ACCENT)));
        let (axis, _) = ui.allocate_exact_size(Vec2::new(width, 18.0), egui::Sense::hover());
        for hz in [100, 500, 1000, 1500, 2000, 2500, 3000, 3500, 4000] {
            let x = axis.left() + (hz as f32 - 100.0) / 3900.0 * axis.width();
            let align = if hz == 100 {
                egui::Align2::LEFT_TOP
            } else if hz == 4000 {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::CENTER_TOP
            };
            ui.painter().text(
                Pos2::new(x, axis.top()),
                align,
                format!("{hz}"),
                egui::FontId::monospace(11.0),
                MUTED,
            );
        }
        if self.dirty {
            let mut pixels = vec![NAVY; WIDTH * HEIGHT];
            for (y, row) in self.rows.iter().enumerate() {
                pixels[y * WIDTH..(y + 1) * WIDTH].copy_from_slice(row);
            }
            let image = egui::ColorImage::new([WIDTH, HEIGHT], pixels);
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    "waterfall",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
            self.dirty = false;
        }
        let height = (ui.available_height() - 44.0).max(120.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
        if let Some(texture) = &self.texture {
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        for (top, start) in annotations::timing_guides(self.clock) {
            let y = rect.top() + top as f32 / 100.0 * rect.height();
            ui.painter().hline(
                rect.x_range(),
                y,
                Stroke::new(
                    0.5_f32,
                    Color32::from_white_alpha(if start { 60 } else { 25 }),
                ),
            );
        }
        for (i, row) in self.history.decodes.iter().enumerate() {
            let Some(b) = annotations::bounds(
                self.clock,
                100.0,
                4000.0,
                row.start_utc_ms,
                row.end_utc_ms,
                row.decode.frequency_hz,
            ) else {
                continue;
            };
            let box_rect = Rect::from_min_size(
                Pos2::new(
                    rect.left() + b.left as f32 / 100.0 * rect.width(),
                    rect.top() + b.top as f32 / 100.0 * rect.height(),
                ),
                Vec2::new(
                    b.width as f32 / 100.0 * rect.width(),
                    b.height as f32 / 100.0 * rect.height(),
                ),
            );
            let kind = message_kind::classify(&row.decode.message, &row.decode.payload);
            ui.painter().rect_stroke(
                box_rect,
                1.0,
                Stroke::new(1.0_f32, color(kind.color())),
                egui::StrokeKind::Inside,
            );
            let response = ui.interact(box_rect, ui.id().with(("decode", i)), egui::Sense::click());
            details(
                ui,
                response,
                format!(
                    "{}\n{} · {:.1} dB SNR · {:.1} audio Hz\nUTC {} – {}\nPayload {}",
                    row.decode.message,
                    kind.label(),
                    row.decode.snr_db,
                    row.decode.frequency_hz,
                    utc(row.start_utc_ms),
                    utc(row.end_utc_ms),
                    row.decode.payload
                ),
            );
        }
        ui.horizontal_wrapped(|ui| {
            for kind in MessageKind::ALL {
                ui.colored_label(
                    color(kind.color()),
                    format!("{} {}", kind.icon(), kind.label()),
                );
            }
        });
    }

    fn messages(&self, ui: &mut egui::Ui) {
        ui.heading(format!(
            "{} decoded transmissions",
            self.history.decodes.len()
        ));
        ui.label(
            RichText::new("UTC start / end · SNR · audio frequency · payload available on hover")
                .color(MUTED),
        );
        if self.history.decodes.is_empty() {
            empty(ui, "Decoded FT8 messages will appear here while listening.");
            return;
        }
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("messages")
                .striped(true)
                .min_col_width(90.0)
                .show(ui, |ui| {
                    for heading in [
                        "Start UTC",
                        "End UTC",
                        "SNR dB",
                        "Audio Hz",
                        "Kind",
                        "Message",
                    ] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    for row in &self.history.decodes {
                        let d = &row.decode;
                        let kind = message_kind::classify(&d.message, &d.payload);
                        ui.monospace(utc(row.start_utc_ms));
                        ui.monospace(utc(row.end_utc_ms));
                        ui.monospace(format!("{:+.1}", d.snr_db));
                        ui.monospace(format!("{:.1}", d.frequency_hz));
                        ui.colored_label(color(kind.color()), kind.label());
                        ui.label(
                            RichText::new(&d.message)
                                .monospace()
                                .color(color(kind.color())),
                        )
                        .on_hover_text(format!("77-bit payload\n{}", d.payload));
                        ui.end_row();
                    }
                });
        });
    }

    fn conversations(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Conversations");
            ui.label("Sort");
            for (name, by) in [
                ("Signal", CardSort::Signal),
                ("State", CardSort::State),
                ("Distance", CardSort::Distance),
            ] {
                if ui.button(name).clicked() {
                    self.history.conversations.sort(by, &self.grid);
                }
            }
            ui.label("My grid");
            ui.add(egui::TextEdit::singleline(&mut self.grid).desired_width(65.0));
        });
        ui.label(RichText::new("Received evidence and expected stages · inferred context is labeled · sign-off is not QSO confirmation").color(MUTED).small());
        let tracker = &self.history.conversations;
        if tracker.items.is_empty() {
            empty(
                ui,
                "Heard exchanges will appear here with six receive stages.",
            );
            return;
        }
        let columns = ((ui.available_width() + 10.0) / 290.0).floor().max(1.0) as usize;
        let max_slot = tracker
            .items
            .iter()
            .map(|c| c.display_slot)
            .max()
            .unwrap_or(0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("conversation-grid")
                .spacing(Vec2::splat(10.0))
                .show(ui, |ui| {
                    for slot in 0..=(max_slot / columns + 1) * columns - 1 {
                        ui.allocate_ui_with_layout(
                            Vec2::new(280.0, 230.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_min_size(Vec2::new(280.0, 230.0));
                                ui.set_max_size(Vec2::new(280.0, 230.0));
                                if let Some(c) =
                                    tracker.items.iter().find(|c| c.display_slot == slot)
                                {
                                    let tint = color(c.color(tracker.now_ms));
                                    let frame = egui::Frame::new()
                                        .fill(Color32::from_rgb(18, 31, 49))
                                        .stroke(Stroke::new(1.0_f32, tint))
                                        .inner_margin(10.0)
                                        .corner_radius(5.0)
                                        .show(ui, |ui| {
                                            ui.set_width(258.0);
                                            ui.label(RichText::new(c.label()).strong().color(tint));
                                            ui.label(
                                                RichText::new(c.status(tracker.now_ms))
                                                    .small()
                                                    .color(tint),
                                            );
                                            for stage in c.exchange_slots(tracker.now_ms) {
                                                let received = stage.evidence.as_ref();
                                                let text = received
                                                    .map(|e| e.latest.message.as_str())
                                                    .unwrap_or(stage.state.label());
                                                let prefix =
                                                    if received.is_some() { "●" } else { "▧" };
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(format!(
                                                            "{prefix} {:8} {text}",
                                                            stage.step.short_label()
                                                        ))
                                                        .monospace()
                                                        .size(11.0)
                                                        .color(if received.is_some() {
                                                            tint
                                                        } else {
                                                            MUTED
                                                        }),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(format!(
                                                    "{} · {}\n{}\nDirection: {}",
                                                    stage.step.label(),
                                                    stage.state.label(),
                                                    text,
                                                    stage.direction
                                                ));
                                            }
                                            ui.label(
                                                RichText::new(geography::description(
                                                    c.location(),
                                                    &self.grid,
                                                ))
                                                .small()
                                                .color(MUTED),
                                            );
                                            ui.label(
                                                RichText::new(if c.inferred {
                                                    "Late entry / inferred context"
                                                } else {
                                                    "Opening heard"
                                                })
                                                .small()
                                                .color(MUTED),
                                            );
                                        })
                                        .response;
                                    let response = ui.interact(
                                        frame.rect,
                                        ui.id().with(("conversation", c.id)),
                                        egui::Sense::click(),
                                    );
                                    details(
                                        ui,
                                        response,
                                        format!(
                                            "{}\n{}\n{}",
                                            c.inference_note(),
                                            c.expectation(tracker.now_ms),
                                            c.received_history()
                                                .iter()
                                                .map(|h| format!(
                                                    "{}  {}  {:+.0} dB",
                                                    utc(h.start_ms),
                                                    h.message,
                                                    h.snr_db
                                                ))
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        ),
                                    );
                                }
                            },
                        );
                        if (slot + 1) % columns == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(8.0);
            self.header(ui);
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let elapsed = self.origin.map_or(0.0, |o| {
                    (self.progress[0].max(self.progress[1]) - o) / 1000.0
                });
                ui.label(format!("Captured {:.1}s", elapsed.max(0.0)));
                ui.separator();
                ui.label(format!("Slot attempt {:.2}s", self.durations[0]));
                ui.separator();
                ui.label(format!(
                    "Recovery attempt {:.2}s · lag {:.1}s",
                    self.durations[1],
                    ((self.progress[0] - self.progress[1]) / 1000.0).max(0.0)
                ));
                ui.separator();
                ui.colored_label(MUTED, "Receive only · shared Rust FT8 decoder");
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Spectrum => self.spectrum(ui),
            Tab::Messages => self.messages(ui),
            Tab::Conversations => self.conversations(ui),
        });
        if self.session.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn empty(ui: &mut egui::Ui, text: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).color(MUTED).size(17.0));
    });
}
fn details(ui: &egui::Ui, response: egui::Response, text: String) {
    let dismissed = response.id.with("details-dismissed");
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        response.surrender_focus();
        ui.ctx().data_mut(|d| d.insert_temp(dismissed, true));
        return;
    }
    if response.gained_focus() || !response.hovered() {
        ui.ctx().data_mut(|d| d.remove::<bool>(dismissed));
    }
    if ui
        .ctx()
        .data(|d| d.get_temp::<bool>(dismissed))
        .unwrap_or(false)
    {
        return;
    }
    if response.has_focus() {
        response.show_tooltip_text(&text);
    }
    response.on_hover_text(text);
}
fn color(hex: &str) -> Color32 {
    let rgb = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xc7d1dc);
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
fn utc(ms: f64) -> String {
    let ms = (ms.round() as i64).rem_euclid(86_400_000);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}
