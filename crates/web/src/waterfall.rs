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
}

pub struct FrameInfo {
    pub clock: (f64, f64),
    pub floor_db: f32,
    pub open_frequency: Option<f32>,
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
        })
    }

    pub fn update(
        &mut self,
        now_ms: f64,
        audio_ms: f64,
        samples: &[f32],
        manual_floor: f32,
        automatic: bool,
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
        self.spectrum
            .levels(samples, &mut self.levels)
            .map_err(JsValue::from_str)?;
        let floor_db = self.scale.floor(&self.levels, manual_floor, automatic);
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
        let latest = audio_ms - FFT_SIZE as f64 / self.rate * 500.0;
        let first = *self.first_ms.get_or_insert(latest);
        self.occupancy.push(latest, &self.levels);
        Ok(Some(FrameInfo {
            clock: (first, latest),
            floor_db,
            open_frequency: self.occupancy.best(),
        }))
    }
}
