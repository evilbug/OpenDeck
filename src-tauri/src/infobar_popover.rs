use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use base64::{Engine as _, engine::general_purpose};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut};
use imageproc::rect::Rect;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

use dashmap::DashMap;
use std::sync::LazyLock;

static THUMBNAIL_CACHE: LazyLock<DashMap<String, image::RgbaImage>> = LazyLock::new(DashMap::new);
static FONT_DATA: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");

const WIDTH: u32 = 248;
const HEIGHT: u32 = 58;
const BG: Rgba<u8> = Rgba([0, 0, 0, 0]);
const FG: Rgba<u8> = Rgba([255, 255, 255, 255]);
/// Steel-blue accent used for the progress bar fill.
const ACCENT: Rgba<u8> = Rgba([70, 130, 180, 255]);
/// Dark-grey used for the progress bar track.
const TRACK: Rgba<u8> = Rgba([60, 60, 60, 255]);

/// A component descriptor that plugins send to the backend to request a
/// rendered popover image for the infobar LCD.
#[derive(Deserialize, Serialize, Clone)]
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

	/// A permanent-style infobar layout with an image scaled to the infobar
	/// height on the left, a large title on the first line, and a smaller
	/// subtitle on the second line.
	ImageTitleSubtitle {
		image: String,
		title: String,
		subtitle: String,
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

/// Returns the scrollable width of a component (0 if no scrolling is possible).
pub fn get_scroll_width(component: &InfobarComponent) -> f32 {
	let font_data = FONT_DATA;
	let font = match FontRef::try_from_slice(font_data) {
		Ok(f) => f,
		Err(_) => return 0.0,
	};

	match component {
		InfobarComponent::Text { text } => {
			let scale = PxScale { x: 26.0, y: 26.0 };
			let full_w = text_width(&font, scale, text);
			let max_w = (WIDTH - 12) as f32;
			(full_w - max_w).max(0.0)
		}
		InfobarComponent::Pill { text } => {
			let scale = PxScale { x: 24.0, y: 24.0 };
			let horizontal_padding = 10.0f32;
			let max_pill_width = (WIDTH as f32) - 20.0;
			let max_text_width = (max_pill_width - horizontal_padding * 2.0).max(0.0);
			let full_w = text_width(&font, scale, text);
			(full_w - max_text_width).max(0.0)
		}
		InfobarComponent::ImageText { text, .. } => {
			let scale = PxScale { x: 24.0, y: 24.0 };
			let max_text_width = (WIDTH - 58 - 6) as f32;
			let full_w = text_width(&font, scale, text);
			(full_w - max_text_width).max(0.0)
		}
		InfobarComponent::ImageTitleSubtitle { title, subtitle, .. } => {
			let title_scale = PxScale { x: 22.0, y: 22.0 };
			let subtitle_scale = PxScale { x: 14.0, y: 14.0 };
			let max_text_width = (WIDTH - HEIGHT - 6) as f32;
			let title_overflow = (text_width(&font, title_scale, title) - max_text_width).max(0.0);
			let subtitle_overflow = (text_width(&font, subtitle_scale, subtitle) - max_text_width).max(0.0);
			title_overflow.max(subtitle_overflow)
		}
		InfobarComponent::ProgressBar { label, value, .. } => {
			let scale = PxScale { x: 22.0, y: 22.0 };
			let val_text = format!("{value:.0}");
			let val_w = text_width(&font, scale, &val_text);
			let val_x = ((WIDTH as f32) - 6.0 - val_w).max(0.0);
			let label_max = val_x - 6.0 - 8.0;
			let full_w = text_width(&font, scale, label);
			(full_w - label_max).max(0.0)
		}
	}
}

/// Render an [`InfobarComponent`] to a 248×58 PNG and return it as a
/// `data:image/png;base64,…` data URI.
///
/// If `offset` is `Some(x)`, it will render the full text shifted left by `x`
/// instead of truncating with an ellipsis.
pub fn render_component_at(component: &InfobarComponent, offset: Option<f32>) -> Result<String, anyhow::Error> {
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
			if let Some(off) = offset {
				draw_text_mut(&mut img, FG, 6 - (off as i32), 16, scale, &font, text);
			} else {
				let display = truncate(&font, scale, text, (WIDTH - 12) as f32);
				draw_text_mut(&mut img, FG, 6, 16, scale, &font, &display);
			}
		}

		InfobarComponent::Pill { text } => {
			let pill_bg = Rgba([55, 55, 65, 255]);
			let scale = PxScale { x: 24.0, y: 24.0 };
			let horizontal_padding = 10i32;
			let vertical_padding = 6i32;
			let max_pill_width = (WIDTH as i32) - 20;
			let max_text_width = (max_pill_width - horizontal_padding * 2).max(0) as f32;

			let (w, tw) = if offset.is_some() {
				(max_pill_width, text_width(&font, scale, text).ceil() as i32)
			} else {
				let d = truncate(&font, scale, text, max_text_width);
				let tw = text_width(&font, scale, &d).ceil() as i32;
				((tw + horizontal_padding * 2).clamp(2, max_pill_width), tw)
			};

			let h = (scale.y as i32) + vertical_padding * 2; // ~36-38px
			let radius = (h / 2).min(w / 2).max(1);
			let x = (WIDTH as i32 - w) / 2;
			let y = (HEIGHT as i32 - h) / 2;

			let mid_w = (w - radius * 2).max(1) as u32;
			let mid_h = (h - radius * 2).max(1) as u32;
			draw_filled_rect_mut(&mut img, Rect::at(x + radius, y).of_size(mid_w, h as u32), pill_bg);
			draw_filled_rect_mut(&mut img, Rect::at(x, y + radius).of_size(w as u32, mid_h), pill_bg);
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

			if let Some(off) = offset {
				// Clip the text to the pill's interior width.
				let mut text_img = RgbaImage::new(max_text_width as u32, scale.y as u32 + 4);
				draw_text_mut(&mut text_img, FG, -(off as i32), 0, scale, &font, text);
				image::imageops::overlay(&mut img, &text_img, (x + horizontal_padding) as i64, (y + vertical_padding) as i64);
			} else {
				let display = truncate(&font, scale, text, max_text_width);
				let tx = x + (w - tw) / 2;
				let ty = y + vertical_padding;
				draw_text_mut(&mut img, FG, tx, ty, scale, &font, &display);
			}
		}

		InfobarComponent::ImageText { image: image_data, text } => {
			if THUMBNAIL_CACHE.len() > 100 {
				THUMBNAIL_CACHE.clear();
			}
			let thumb = if let Some(cached) = THUMBNAIL_CACHE.get(image_data) {
				cached.clone()
			} else {
				let raw = image_data.split_once(',').map(|(_, b)| b).unwrap_or(image_data.as_str());
				let bytes = general_purpose::STANDARD.decode(raw)?;
				let processed = image::load_from_memory(&bytes)?.resize_exact(50, 50, image::imageops::FilterType::Lanczos3).into_rgba8();
				THUMBNAIL_CACHE.insert(image_data.clone(), processed.clone());
				processed
			};
			image::imageops::overlay(&mut img, &thumb, 4, 4);

			let scale = PxScale { x: 24.0, y: 24.0 };
			let max_text_width = (WIDTH - 58 - 6) as f32;
			if let Some(off) = offset {
				let mut text_img = RgbaImage::new(max_text_width as u32, scale.y as u32 + 8);
				draw_text_mut(&mut text_img, FG, -(off as i32), 4, scale, &font, text);
				image::imageops::overlay(&mut img, &text_img, 60, 13);
			} else {
				let display = truncate(&font, scale, text, max_text_width);
				draw_text_mut(&mut img, FG, 60, 17, scale, &font, &display);
			}
		}

		InfobarComponent::ImageTitleSubtitle { image: image_data, title, subtitle } => {
			if THUMBNAIL_CACHE.len() > 100 {
				THUMBNAIL_CACHE.clear();
			}
			let thumb = if let Some(cached) = THUMBNAIL_CACHE.get(image_data) {
				cached.clone()
			} else {
				let raw = image_data.split_once(',').map(|(_, b)| b).unwrap_or(image_data.as_str());
				let bytes = general_purpose::STANDARD.decode(raw)?;
				let dynamic = image::load_from_memory(&bytes)?;
				let processed = dynamic
					.resize(u32::MAX, HEIGHT, image::imageops::FilterType::Lanczos3)
					.into_rgba8();
				THUMBNAIL_CACHE.insert(image_data.clone(), processed.clone());
				processed
			};
			image::imageops::overlay(&mut img, &thumb, 0, ((HEIGHT as i64 - thumb.height() as i64) / 2).max(0));

			let text_x = (thumb.width() as i32 + 6).min(WIDTH as i32 - 10);
			let max_text_width = ((WIDTH as i32) - text_x - 4).max(10) as f32;
			let title_scale = PxScale { x: 22.0, y: 22.0 };
			let subtitle_scale = PxScale { x: 14.0, y: 14.0 };

			if let Some(off) = offset {
				let mut title_img = RgbaImage::new(max_text_width as u32, title_scale.y as u32 + 8);
				draw_text_mut(&mut title_img, FG, -(off as i32), 0, title_scale, &font, title);
				image::imageops::overlay(&mut img, &title_img, text_x as i64, 5);

				let mut subtitle_img = RgbaImage::new(max_text_width as u32, subtitle_scale.y as u32 + 8);
				draw_text_mut(&mut subtitle_img, FG, -(off as i32), 0, subtitle_scale, &font, subtitle);
				image::imageops::overlay(&mut img, &subtitle_img, text_x as i64, 31);
			} else {
				let display_title = truncate(&font, title_scale, title, max_text_width);
				let display_subtitle = truncate(&font, subtitle_scale, subtitle, max_text_width);
				draw_text_mut(&mut img, FG, text_x, 5, title_scale, &font, &display_title);
				draw_text_mut(&mut img, FG, text_x, 31, subtitle_scale, &font, &display_subtitle);
			}
		}

		InfobarComponent::ProgressBar { label, value, min, max } => {
			let scale = PxScale { x: 22.0, y: 22.0 };
			let val_text = format!("{value:.0}");
			let val_w = text_width(&font, scale, &val_text);
			let val_x = ((WIDTH as f32) - 6.0 - val_w).max(0.0) as i32;
			draw_text_mut(&mut img, FG, val_x, 3, scale, &font, &val_text);

			let label_max = (val_x as f32) - 6.0 - 8.0;
			if let Some(off) = offset {
				let mut text_img = RgbaImage::new(label_max as u32, scale.y as u32 + 8);
				draw_text_mut(&mut text_img, FG, -(off as i32), 0, scale, &font, label);
				image::imageops::overlay(&mut img, &text_img, 6, 3);
			} else {
				let label_display = truncate(&font, scale, label, label_max.max(0.0));
				draw_text_mut(&mut img, FG, 6, 3, scale, &font, &label_display);
			}

			let bar_x = 6i32;
			let bar_y = 32i32;
			let bar_w = WIDTH - 12;
			let bar_h = 18u32;
			draw_filled_rect_mut(&mut img, Rect::at(bar_x, bar_y).of_size(bar_w, bar_h), TRACK);
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

pub fn render_component(component: &InfobarComponent) -> Result<String, anyhow::Error> {
	render_component_at(component, None)
}
