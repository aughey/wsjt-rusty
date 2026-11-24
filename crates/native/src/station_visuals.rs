//! egui rendering only. Local audio never changes a received stage's state.
use eframe::egui::{self, Color32, RichText, Stroke};
use wsjt_receiver::{
    contact_queue::TransmitStage,
    conversations::{ExchangeSlot, ExchangeStep, SlotState},
    message_kind::MessageKind,
};

pub const AMBER: Color32 = Color32::from_rgb(255, 208, 120);
pub const CYAN: Color32 = Color32::from_rgb(104, 222, 200);

pub fn color(hex: &str) -> Color32 {
    let rgb = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xc7d1dc);
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}
fn pulse(time: f64) -> f32 {
    (0.5 - 0.5 * (time * std::f64::consts::TAU / 1.6).cos()) as f32
}
fn shadow(tint: Color32, blur: u8, alpha: u8) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 0],
        blur,
        spread: 2,
        color: Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), alpha),
    }
}
pub fn card_frame(
    active: bool,
    playing: bool,
    sent: bool,
    tint: Color32,
    time: f64,
) -> egui::Frame {
    let p = pulse(time);
    egui::Frame::new()
        .inner_margin(12.0)
        .corner_radius(7.0)
        .fill(if playing {
            Color32::from_rgb(50, 39, 25)
        } else {
            Color32::from_rgb(8, 19, 30)
        })
        .stroke(Stroke::new(
            if active || playing { 2.0_f32 } else { 1.0_f32 },
            if playing {
                AMBER
            } else if active {
                CYAN
            } else {
                tint
            },
        ))
        .shadow(if playing {
            shadow(AMBER, (14.0 + p * 14.0) as u8, (90.0 + p * 90.0) as u8)
        } else if sent {
            shadow(CYAN, 15, 65)
        } else {
            egui::epaint::Shadow::NONE
        })
}

pub fn stage_color(slot: &ExchangeSlot) -> Color32 {
    let kind = match slot.step {
        ExchangeStep::Cq => MessageKind::Cq,
        ExchangeStep::Callback => MessageKind::Reply,
        ExchangeStep::Report => MessageKind::Report,
        ExchangeStep::RogerReport => MessageKind::RogerReport,
        ExchangeStep::Acknowledgment
            if slot
                .evidence
                .as_ref()
                .is_some_and(|e| e.latest.message.ends_with("RR73")) =>
        {
            MessageKind::Signoff
        }
        ExchangeStep::Acknowledgment => MessageKind::Acknowledgment,
        ExchangeStep::Farewell => MessageKind::Signoff,
    };
    color(kind.color())
}

pub fn stage(ui: &mut egui::Ui, slot: &ExchangeSlot, tx: &TransmitStage) {
    let received = slot.state == SlotState::Received;
    let missing = matches!(slot.state, SlotState::Missing | SlotState::NotHeard);
    let tint = if tx.transmitting {
        AMBER
    } else if tx.sent {
        CYAN
    } else if received {
        stage_color(slot)
    } else if matches!(slot.state, SlotState::Expected) || missing {
        Color32::from_rgb(225, 194, 143)
    } else {
        Color32::from_rgb(131, 150, 172)
    };
    let fill = if tx.transmitting {
        Color32::from_rgb(50, 39, 25)
    } else if tx.sent {
        Color32::from_rgb(18, 49, 47)
    } else if received {
        tint.gamma_multiply(0.10)
    } else {
        Color32::from_rgb(16, 26, 39)
    };
    let frame = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(
            if tx.transmitting { 2.0_f32 } else { 1.0_f32 },
            tint.gamma_multiply(if received || tx.sent || tx.transmitting {
                1.0
            } else {
                0.55
            }),
        ))
        .corner_radius(3.0)
        .inner_margin(7.0)
        .shadow(if tx.transmitting {
            shadow(AMBER, 16, (80.0 + pulse(ui.input(|i| i.time)) * 70.0) as u8)
        } else if tx.sent {
            shadow(CYAN, 8, 45)
        } else {
            egui::epaint::Shadow::NONE
        });
    let response = frame
        .show(ui, |ui| {
            ui.set_min_width(286.0);
            let mark = if received {
                "RX"
            } else if missing {
                "//"
            } else if slot.state == SlotState::Expected {
                ">"
            } else {
                "·"
            };
            ui.label(
                RichText::new(format!(
                    "{mark} {} · {}",
                    slot.step.short_label(),
                    slot.state.label()
                ))
                .small()
                .strong()
                .color(tint),
            );
            if let Some(e) = &slot.evidence {
                ui.add(
                    egui::Label::new(
                        RichText::new(&e.latest.message)
                            .monospace()
                            .color(stage_color(slot)),
                    )
                    .truncate(),
                )
                .on_hover_text(&e.latest.message);
            }
            if !tx.status.is_empty() {
                ui.label(
                    RichText::new(format!("Local audio: {}", tx.status))
                        .strong()
                        .color(if tx.transmitting { AMBER } else { CYAN }),
                );
                if !tx.message.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&tx.message)
                                .monospace()
                                .color(if tx.transmitting { AMBER } else { CYAN }),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&tx.message);
                }
            }
        })
        .response;
    if slot.current {
        ui.painter().vline(
            response.rect.left() + 2.0,
            response.rect.y_range(),
            Stroke::new(3.0_f32, tint),
        );
    }
    if missing && !tx.sent && !tx.transmitting {
        // A hatched edge also distinguishes missing evidence without color.
        let rect = response.rect;
        let mut y = rect.top() + 2.0;
        while y + 5.0 < rect.bottom() {
            ui.painter().line_segment(
                [
                    egui::pos2(rect.right() - 7.0, y + 5.0),
                    egui::pos2(rect.right() - 2.0, y),
                ],
                Stroke::new(1.0_f32, tint.gamma_multiply(0.45)),
            );
            y += 8.0;
        }
    }
    response.on_hover_text(format!(
        "{} · {}\n{}\n{}",
        slot.step.label(),
        slot.state.label(),
        slot.direction,
        slot.evidence
            .as_ref()
            .map(|e| format!(
                "Received: {} · {:+.1} dB SNR",
                e.latest.message, e.latest.snr_db
            ))
            .unwrap_or_else(|| "No decoded evidence for this stage".into())
    ));
}

pub fn banner(ui: &mut egui::Ui, target: &str, message: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(61, 39, 15))
        .stroke(Stroke::new(2.0_f32, AMBER))
        .corner_radius(5.0)
        .inner_margin(10.0)
        .shadow(shadow(AMBER, 18, 100))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!("TX AUDIO ON  |  {target}"))
                    .size(20.0)
                    .strong()
                    .color(AMBER),
            );
            ui.label(
                RichText::new(message)
                    .monospace()
                    .size(16.0)
                    .color(Color32::from_rgb(255, 240, 206)),
            );
        });
}

/// A visible, non-interactive amber edge on EVERY page while audio is playing.
pub fn window_glow(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("tx-window-glow"),
    ));
    let rect = ctx.content_rect();
    let p = pulse(ctx.input(|i| i.time));
    for i in (0..14).rev() {
        let alpha = ((1.0 - i as f32 / 14.0) * (25.0 + p * 24.0)) as u8;
        painter.rect_stroke(
            rect.shrink(i as f32),
            0.0,
            Stroke::new(
                2.0_f32,
                Color32::from_rgba_unmultiplied(255, 190, 75, alpha),
            ),
            egui::StrokeKind::Inside,
        );
    }
    painter.rect_stroke(
        rect.shrink(1.0),
        0.0,
        Stroke::new(3.0_f32, AMBER),
        egui::StrokeKind::Inside,
    );
    ctx.request_repaint();
}
