use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use base64::{Engine as _, engine::general_purpose};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut};
use imageproc::rect::Rect;
use serde::Deserialize;
use std::io::Cursor;

static FONT_DATA: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");

const WIDTH: u32 = 248;
const HEIGHT: u32 = 58;
const BG: Rgba<u8> = Rgba([20, 20, 20, 255]);
const FG: Rgba<u8> = Rgba([255, 255, 255, 255]);
/// Steel-blue accent used for the progress bar fill.
const ACCENT: Rgba<u8> = Rgba([70, 130, 180, 255]);
/// Dark-grey used for the progress bar track.
const TRACK: Rgba<u8> = Rgba([60, 60, 60, 255]);

/// A component descriptor that plugins send to the backend to request a
/// rendered popover image for the infobar LCD.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InfobarComponent {
	/// A single line of text centred vertically.
	Text { text: String },

	/// A compact rounded pill with centered text.
	Pill { text: String },

	/// A thumbnail image on the left with text to the right of it.
	ImageText {
		/// Base-64 data URI of the thumbnail (`data:<mime>;base64,…`).
		image: String,
		text: String,
	},

	/// A label + numeric value on the top row with a filled progress bar on
	/// the bottom row.  `min` and `max` define the range; `value` is clamped
	/// to that range.
	ProgressBar { label: String, value: f32, min: f32, max: f32 },
}

/// Returns the rendered pixel width of `text` at the given scale.
fn text_width(font: &FontRef, scale: PxScale, text: &str) -> f32 {
	let scaled = font.as_scaled(scale);
	let mut width = 0.0f32;
	let mut prev: Option<ab_glyph::GlyphId> = None;
	for c in text.chars() {
		let id = scaled.glyph_id(c);
		width += prev.map_or(0.0, |p| scaled.kern(p, id));
		width += scaled.h_advance(id);
		prev = Some(id);
	}
	width
}

/// Returns `text` truncated with a trailing `…` if it exceeds `max_px` wide.
fn truncate(font: &FontRef, scale: PxScale, text: &str, max_px: f32) -> String {
	if text_width(font, scale, text) <= max_px {
		return text.to_owned();
	}
	let ellipsis = "…";
	let available = (max_px - text_width(font, scale, ellipsis)).max(0.0);
	let scaled = font.as_scaled(scale);
	let mut result = String::new();
	let mut used = 0.0f32;
	let mut prev: Option<ab_glyph::GlyphId> = None;
	for c in text.chars() {
		let id = scaled.glyph_id(c);
		let kern = prev.map_or(0.0, |p| scaled.kern(p, id));
		let advance = scaled.h_advance(id);
		if used + kern + advance > available {
			break;
		}
		used += kern + advance;
		result.push(c);
		prev = Some(id);
	}
	result.push_str(ellipsis);
	result
}

/// Render an [`InfobarComponent`] to a 248×58 PNG and return it as a
/// `data:image/png;base64,…` data URI.
pub fn render_component(component: &InfobarComponent) -> Result<String, anyhow::Error> {
	let mut img = RgbaImage::new(WIDTH, HEIGHT);

	// Fill background.
	for pixel in img.pixels_mut() {
		*pixel = BG;
	}

	let font = FontRef::try_from_slice(FONT_DATA)?;

	match component {
		InfobarComponent::Text { text } => {
			// Vertically centre a 26pt font: (58 - 26) / 2 ≈ 16.
			let scale = PxScale { x: 26.0, y: 26.0 };
			let display = truncate(&font, scale, text, (WIDTH - 12) as f32);
			draw_text_mut(&mut img, FG, 6, 16, scale, &font, &display);
		}

		InfobarComponent::Pill { text } => {
			let pill_bg = Rgba([55, 55, 65, 255]);
			let radius = 14i32;
			let x = 10i32;
			let y = 9i32;
			let w = (WIDTH as i32) - 20;
			let h = (HEIGHT as i32) - 18;

			draw_filled_rect_mut(&mut img, Rect::at(x + radius, y).of_size((w - radius * 2) as u32, h as u32), pill_bg);
			draw_filled_rect_mut(&mut img, Rect::at(x, y + radius).of_size(w as u32, (h - radius * 2) as u32), pill_bg);
			for cy in [y + radius, y + h - radius - 1] {
				for cx in [x + radius, x + w - radius - 1] {
					for dy in -radius..=radius {
						for dx in -radius..=radius {
							if dx * dx + dy * dy <= radius * radius {
								let px = cx + dx;
								let py = cy + dy;
								if px >= 0 && py >= 0 && px < WIDTH as i32 && py < HEIGHT as i32 {
									img.put_pixel(px as u32, py as u32, pill_bg);
								}
							}
						}
					}
				}
			}

			let scale = PxScale { x: 24.0, y: 24.0 };
			let display = truncate(&font, scale, text, (WIDTH - 28) as f32);
			let text_w = text_width(&font, scale, &display);
			let tx = ((WIDTH as f32 - text_w) / 2.0).max(10.0) as i32;
			draw_text_mut(&mut img, FG, tx, 17, scale, &font, &display);
		}

		InfobarComponent::ImageText { image: image_data, text } => {
			// Decode thumbnail and paste it at the left edge (4px padding,
			// full height minus padding = 50px).
			let raw = image_data.split_once(',').map(|(_, b)| b).unwrap_or(image_data.as_str());
			let bytes = general_purpose::STANDARD.decode(raw)?;
			let thumb = image::load_from_memory(&bytes)?.resize_exact(50, 50, image::imageops::FilterType::Lanczos3).into_rgba8();
			image::imageops::overlay(&mut img, &thumb, 4, 4);

			// Text to the right of the thumbnail, vertically centred.
			let scale = PxScale { x: 24.0, y: 24.0 };
			let display = truncate(&font, scale, text, (WIDTH - 58 - 6) as f32);
			draw_text_mut(&mut img, FG, 60, 17, scale, &font, &display);
		}

		InfobarComponent::ProgressBar { label, value, min, max } => {
			// ── Top row: label on the left, value on the right ─────────────
			let scale = PxScale { x: 22.0, y: 22.0 };

			let val_text = format!("{value:.0}");
			let val_w = text_width(&font, scale, &val_text);
			// Leave 6px right margin.
			let val_x = ((WIDTH as f32) - 6.0 - val_w).max(0.0) as i32;
			draw_text_mut(&mut img, FG, val_x, 3, scale, &font, &val_text);

			// Truncate the label so it never overlaps the value (8px gap).
			let label_max = (val_x as f32) - 6.0 - 8.0;
			let label_display = truncate(&font, scale, label, label_max.max(0.0));
			draw_text_mut(&mut img, FG, 6, 3, scale, &font, &label_display);

			// ── Bottom row: progress bar ────────────────────────────────────
			let bar_x = 6i32;
			let bar_y = 32i32;
			let bar_w = WIDTH - 12;
			let bar_h = 18u32;

			// Track background.
			draw_filled_rect_mut(&mut img, Rect::at(bar_x, bar_y).of_size(bar_w, bar_h), TRACK);

			// Filled portion.
			let range = (max - min).abs().max(1.0);
			let ratio = ((value - min) / range).clamp(0.0, 1.0);
			let fill_w = ((bar_w as f32) * ratio) as u32;
			if fill_w > 0 {
				draw_filled_rect_mut(&mut img, Rect::at(bar_x, bar_y).of_size(fill_w, bar_h), ACCENT);
			}
		}
	}

	let mut buf = Cursor::new(Vec::new());
	img.write_to(&mut buf, image::ImageFormat::Png)?;
	let b64 = general_purpose::STANDARD.encode(buf.into_inner());
	Ok(format!("data:image/png;base64,{b64}"))
}
