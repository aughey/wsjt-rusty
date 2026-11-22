//! Fixed-size scrolling canvas; FFT math is in the independently tested spectrum module.
use crate::spectrum::{FFT_SIZE, LevelScale, Spectrum, color};
use wasm_bindgen::{Clamped, JsValue};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

pub const WIDTH: u32 = 1200;
pub const HEIGHT: u32 = 600;
const ROW_MS: f64 = 100.0;

pub struct Waterfall {
    canvas: HtmlCanvasElement,
    context: CanvasRenderingContext2d,
    spectrum: Spectrum,
    levels: Vec<f32>,
    rgba: Vec<u8>,
    last_row: Option<f64>,
    first_ms: Option<f64>,
    rate: f64,
    scale: LevelScale,
    occupancy: crate::open_frequency::Occupancy,
    receive_gate: crate::spectrum::ReceiveSnapshotGate,
    last_floor: f32,
}

pub struct FrameInfo {
    pub clock: (f64, f64),
    pub floor_db: f32,
    pub open_frequency: Option<(f64, f32)>,
}

pub fn clear(canvas: &HtmlCanvasElement, context: &CanvasRenderingContext2d) {
    context.set_fill_style_str("#0b1325");
    context.fill_rect(0.0, 0.0, canvas.width().into(), canvas.height().into());
}

impl Waterfall {
    pub fn new(
        canvas: HtmlCanvasElement,
        context: CanvasRenderingContext2d,
        rate: f32,
        low: f32,
        high: f32,
    ) -> Result<Self, &'static str> {
        clear(&canvas, &context);
        Ok(Self {
            spectrum: Spectrum::new(rate, low, high, WIDTH as usize)?,
            canvas,
            context,
            levels: vec![0.0; WIDTH as usize],
            rgba: vec![0; WIDTH as usize * 4],
            last_row: None,
            first_ms: None,
            rate: rate.into(),
            scale: LevelScale::default(),
            occupancy: crate::open_frequency::Occupancy::new(low, high, WIDTH as usize),
            receive_gate: crate::spectrum::ReceiveSnapshotGate::default(),
            last_floor: -100.0,
        })
    }

    pub fn update(
        &mut self,
        now_ms: f64,
        audio_ms: f64,
        samples: &[f32],
        manual_floor: f32,
        automatic: bool,
        transmitting: bool,
    ) -> Result<Option<FrameInfo>, JsValue> {
        let rows = match self.last_row {
            Some(last) => ((now_ms - last) / ROW_MS).floor() as u32,
            None => 1,
        };
        if rows == 0 {
            return Ok(None);
        }
        self.last_row = Some(match self.last_row {
            Some(last) => last + f64::from(rows) * ROW_MS,
            None => now_ms,
        });
        let shift = rows.min(HEIGHT);
        if shift < HEIGHT {
            self.context
                .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                    &self.canvas,
                    0.0,
                    0.0,
                    WIDTH.into(),
                    (HEIGHT - shift).into(),
                    0.0,
                    shift.into(),
                    WIDTH.into(),
                    (HEIGHT - shift).into(),
                )?;
        }
        self.context.set_fill_style_str("#0b1325");
        // Blank skipped animation rows, so a throttled tab never invents spectra.
        self.context.fill_rect(0.0, 0.0, WIDTH.into(), shift.into());
        let window_ms = FFT_SIZE as f64 / self.rate * 1000.0;
        let latest = audio_ms - window_ms / 2.0;
        let first = *self.first_ms.get_or_insert(latest);
        self.occupancy.expire(latest);
        if !self.receive_gate.allow(audio_ms, window_ms, transmitting) {
            return Ok(Some(FrameInfo {
                clock: (first, latest),
                floor_db: if automatic {
                    self.last_floor
                } else {
                    manual_floor
                },
                open_frequency: self.occupancy.choice(),
            }));
        }
        self.spectrum
            .levels(samples, &mut self.levels)
            .map_err(JsValue::from_str)?;
        let floor_db = self.scale.floor(&self.levels, manual_floor, automatic);
        self.last_floor = floor_db;
        for (pixel, &db) in self
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&self.levels)
        {
            pixel.copy_from_slice(&color(db, floor_db));
        }
        let image = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&self.rgba), WIDTH, 1)?;
        self.context.put_image_data(&image, 0.0, 0.0)?;
        // FFT pixels describe the center of the Hann snapshot, not its arrival.
        self.occupancy.push(latest, &self.levels);
        Ok(Some(FrameInfo {
            clock: (first, latest),
            floor_db,
            open_frequency: self.occupancy.choice(),
        }))
    }
}
