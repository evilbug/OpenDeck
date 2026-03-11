use dashmap::DashMap;
use crate::events::outbound::{encoder, infobar, keypad};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use base64::{Engine as _, engine::general_purpose};
use elgato_streamdeck::{
	AsyncStreamDeck, DeviceStateUpdate,
	images::{ImageRect, convert_image_with_format_async},
	info::Kind,
};
use image::GenericImageView as _;
use tokio::sync::RwLock;

static ELGATO_DEVICES: LazyLock<RwLock<HashMap<String, AsyncStreamDeck>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
static HIDAPI: LazyLock<RwLock<Option<Arc<hidapi::HidApi>>>> = LazyLock::new(|| RwLock::new(None));

static SCROLL_TASKS: LazyLock<DashMap<(String, u8), String>> = LazyLock::new(DashMap::new);
const INFOBAR_SAFE_PADDING: i64 = 4;

fn apply_infobar_safe_padding(img: image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
	if INFOBAR_SAFE_PADDING <= 0 {
		return img;
	}

	let pad = INFOBAR_SAFE_PADDING as u32;
	if w <= pad * 2 {
		return img;
	}

	let inner_w = w - pad * 2;
	let inner_h = h;
	let resized = image::imageops::resize(&img, inner_w, inner_h, image::imageops::FilterType::Lanczos3);

	let mut padded = image::RgbaImage::new(w, h);
	for pixel in padded.pixels_mut() {
		*pixel = image::Rgba([0, 0, 0, 255]);
	}
	image::imageops::overlay(&mut padded, &resized, INFOBAR_SAFE_PADDING, 0);
	padded
}

/// Extract the average colour from an image.
fn extract_average_colour(img: &image::DynamicImage) -> (u8, u8, u8) {
	let (r_sum, g_sum, b_sum) = img
		.pixels()
		.fold((0u64, 0u64, 0u64), |(r, g, b), (_, _, pixel)| (r + pixel[0] as u64, g + pixel[1] as u64, b + pixel[2] as u64));
	let count = (img.width() * img.height()).max(1) as u64;
	((r_sum / count) as u8, (g_sum / count) as u8, (b_sum / count) as u8)
}

static INFOBAR_BASE_CACHE: LazyLock<DashMap<(String, u8), (String, image::RgbaImage)>> = LazyLock::new(DashMap::new);
static INFOBAR_OVERLAY_CACHE: LazyLock<DashMap<(String, u8, u64), (String, image::RgbaImage)>> = LazyLock::new(DashMap::new);

/// Load an image from either a Data URI or a file path.
fn load_image_raw(uri: &str) -> Option<image::DynamicImage> {
	if uri.is_empty() {
		return None;
	}
	if uri.starts_with("data:") {
		let Some((_, data)) = uri.split_once(',') else {
			log::warn!("Invalid data URI (no comma found)");
			return None;
		};
		// Some engines might add whitespace/newlines to base64 strings.
		let clean_data = data.replace(|c: char| c.is_whitespace(), "");
		match general_purpose::STANDARD.decode(clean_data) {
			Ok(bytes) => match image::load_from_memory(&bytes) {
				Ok(img) => Some(img),
				Err(e) => {
					log::warn!("Failed to load image from data URI memory: {e}");
					None
				}
			},
			Err(e) => {
				log::warn!("Failed to decode base64 from data URI: {e}");
				None
			}
		}
	} else {
		match image::open(uri) {
			Ok(img) => Some(img),
			Err(e) => {
				log::warn!("Failed to open image file {uri}: {e}");
				None
			}
		}
	}
}

/// Helper to get the base image for a slot, handling caching and resizing.
fn get_base_image(device_id: &str, position: u8, w: u32, h: u32) -> image::RgbaImage {
	let base_uri = crate::shared::INFOBAR_IMAGES.get(&(device_id.to_owned(), position)).map(|v| v.clone());
	if let Some(uri) = base_uri {
		if INFOBAR_BASE_CACHE.len() > 100 {
			INFOBAR_BASE_CACHE.clear();
		}
		if let Some(cached) = INFOBAR_BASE_CACHE.get(&(device_id.to_owned(), position)) && cached.0 == uri {
			return cached.1.clone();
		}
		
		let actual_uri = if uri == "actionDefaultImage" || uri.starts_with("opendeck/") {
			// Find the instance to get the icon or internal path
			let mut locks = tokio::task::block_in_place(|| {
				tokio::runtime::Handle::current().block_on(crate::store::profiles::acquire_locks_mut())
			});
			let selected_profile = locks.device_stores.get_selected_profile(device_id).unwrap_or_default();
			let context = crate::shared::Context {
				device: device_id.to_owned(),
				profile: selected_profile,
				controller: "Infobar".to_owned(),
				position,
			};
			let instance_icon = tokio::task::block_in_place(|| {
				tokio::runtime::Handle::current().block_on(async {
					if let Ok(Some(instance)) = crate::store::profiles::get_slot_mut(&context, &mut locks).await {
						Some(instance.action.icon.clone())
					} else {
						None
					}
				})
			});

			if uri == "actionDefaultImage" {
				instance_icon.unwrap_or_else(|| uri.clone())
			} else {
				uri.clone()
			}
		} else {
			uri.clone()
		};

		if let Some(dynamic) = load_image_raw(&actual_uri) {
			let processed = if dynamic.width() == w && dynamic.height() == h {
				dynamic.into_rgba8()
			} else {
				dynamic.resize_exact(w, h, image::imageops::FilterType::Lanczos3).into_rgba8()
			};
			INFOBAR_BASE_CACHE.insert((device_id.to_owned(), position), (uri, processed.clone()));
			return processed;
		}
	}
	image::RgbaImage::new(w, h)
}

/// Helper to get an overlay image, handling caching and resizing.
fn get_overlay_image(device_id: &str, position: u8, entry_id: u64, image_uri: &str, w: u32, h: u32) -> Option<image::RgbaImage> {
	if INFOBAR_OVERLAY_CACHE.len() > 200 {
		INFOBAR_OVERLAY_CACHE.clear();
	}
	if let Some(cached) = INFOBAR_OVERLAY_CACHE.get(&(device_id.to_owned(), position, entry_id)) && cached.0 == image_uri {
		return Some(cached.1.clone());
	}
	
	if let Some(dynamic) = load_image_raw(image_uri) {
		let processed = if dynamic.width() == w && dynamic.height() == h {
			dynamic.into_rgba8()
		} else {
			dynamic.resize_exact(w, h, image::imageops::FilterType::Lanczos3).into_rgba8()
		};
		INFOBAR_OVERLAY_CACHE.insert((device_id.to_owned(), position, entry_id), (image_uri.to_owned(), processed.clone()));
		return Some(processed);
	}
	None
}

/// Re-compose the full infobar image from base, title text (at offset), and overlays.
fn compose_infobar_image(device_id: &str, position: u8, w: u32, h: u32, text_offset: Option<f32>) -> image::RgbaImage {
	let mut img = get_base_image(device_id, position, w, h);

	// 1. Component (from setInfobarComponent)
	if let Some(component) = crate::shared::INFOBAR_COMPONENTS.get(&(device_id.to_owned(), position)) {
		if let Ok(comp_uri) = crate::infobar_popover::render_component_at(&component, text_offset) {
			if let Some(comp_img) = load_image_raw(&comp_uri) {
				image::imageops::overlay(&mut img, &comp_img, 0, 0);
			}
		}
	}

	// 2. Title Text (rendered behind popovers)
	if let Some(text) = crate::shared::INFOBAR_TEXT.get(&(device_id.to_owned(), position)) && !text.is_empty() {
		let component = crate::infobar_popover::InfobarComponent::Text { text: text.clone() };
		if let Ok(text_uri) = crate::infobar_popover::render_component_at(&component, text_offset) {
			if let Some(text_img) = load_image_raw(&text_uri) {
				image::imageops::overlay(&mut img, &text_img, 0, 0);
			}
		}
	}

	// 3. Overlays (Popovers) on top
	if let Some(entries) = crate::infobar_overlay::OVERLAYS.get(&(device_id.to_owned(), position)) {
		let now = std::time::Instant::now();
		for entry in entries.values() {
			if entry.expires_at.is_none_or(|exp| exp > now) {
				if let Some(overlay_img) = get_overlay_image(device_id, position, entry.id, &entry.image, w, h) {
					image::imageops::overlay(&mut img, &overlay_img, 0, 0);
					break;
				}
			}
		}
	}

	apply_infobar_safe_padding(img, w, h)
}

pub fn clear_infobar_component_scroll_state(device_id: &str, position: u8) {
	SCROLL_TASKS.remove(&(device_id.to_owned(), position));
}

pub async fn update_image(context: &crate::shared::Context, image: Option<&str>) -> Result<(), anyhow::Error> {
	if let Some(device) = ELGATO_DEVICES.read().await.get(&context.device) {
		let kind = device.kind();
		if !kind.is_visual() {
			return Ok(());
		}
		let key_count = kind.key_count();
		let is_touch_point = context.controller == "Keypad" && context.position >= key_count;

		if context.controller == "Infobar" {
			let (w, h) = if kind == Kind::Plus { (200, 100) } else { (248, 58) };
			
			if let Some(uri) = image {
				crate::shared::INFOBAR_IMAGES.insert((context.device.clone(), context.position), uri.to_owned());
			}

			let final_img = compose_infobar_image(&context.device, context.position, w, h, None);

			// Check if we need to start a scroll task (prefer Component over plain Text)
			let (scroll_width, text_to_scroll, component) = if let Some(comp) = crate::shared::INFOBAR_COMPONENTS.get(&(context.device.clone(), context.position)) {
				let sw = crate::infobar_popover::get_scroll_width(&comp);
				// We use a JSON representation of the component as the scroll key to detect changes.
				let text = serde_json::to_string(&*comp).unwrap_or_default();
				(sw, text, Some(comp.clone()))
			} else if let Some(text) = crate::shared::INFOBAR_TEXT.get(&(context.device.clone(), context.position)) && !text.is_empty() {
				let comp = crate::infobar_popover::InfobarComponent::Text { text: text.clone() };
				let sw = crate::infobar_popover::get_scroll_width(&comp);
				(sw, text.clone(), Some(comp))
			} else {
				(0.0, String::new(), None)
			};

			if scroll_width > 0.0 && let Some(_component) = component {
				let current_scroll_text = SCROLL_TASKS.get(&(context.device.clone(), context.position)).map(|v| v.clone());
				if current_scroll_text.as_ref() != Some(&text_to_scroll) {
					SCROLL_TASKS.insert((context.device.clone(), context.position), text_to_scroll.clone());
					let ctx = context.clone();
					let scroll_key = text_to_scroll.clone();
					tokio::spawn(async move {
						tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
						let step_size = 4.0f32;
						let steps = (scroll_width / step_size).ceil() as u32;
						for step in 1..=steps {
							if SCROLL_TASKS.get(&(ctx.device.clone(), ctx.position)).as_deref() != Some(&scroll_key) {
								return;
							}

							let offset = (step as f32 * step_size).min(scroll_width);
							let scroll_img = compose_infobar_image(&ctx.device, ctx.position, w, h, Some(offset));
							if let Some(device) = ELGATO_DEVICES.read().await.get(&ctx.device) {
								if device.kind() == Kind::Plus {
									let _ = device.write_lcd(ctx.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::ImageRgba8(scroll_img)).unwrap()).await;
								} else if device.kind() == Kind::Neo {
									let format = device.kind().lcd_image_format().unwrap();
									let data = convert_image_with_format_async(format, image::DynamicImage::ImageRgba8(scroll_img)).unwrap();
									let _ = device.write_lcd_fill(&data).await;
								}
								let _ = device.flush().await;
							}
							tokio::time::sleep(std::time::Duration::from_millis(50)).await;
						}

						if SCROLL_TASKS.get(&(ctx.device.clone(), ctx.position)).as_deref() == Some(&scroll_key) {
							tokio::time::sleep(std::time::Duration::from_millis(400)).await;
							let reset_img = compose_infobar_image(&ctx.device, ctx.position, w, h, None);
							if let Some(device) = ELGATO_DEVICES.read().await.get(&ctx.device) {
								if device.kind() == Kind::Plus {
									let _ = device.write_lcd(ctx.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::ImageRgba8(reset_img)).unwrap()).await;
								} else if device.kind() == Kind::Neo {
									let format = device.kind().lcd_image_format().unwrap();
									let data = convert_image_with_format_async(format, image::DynamicImage::ImageRgba8(reset_img)).unwrap();
									let _ = device.write_lcd_fill(&data).await;
								}
								let _ = device.flush().await;
							}
							SCROLL_TASKS.remove(&(ctx.device.clone(), ctx.position));
						}
					});
				}
			} else {
				SCROLL_TASKS.remove(&(context.device.clone(), context.position));
			}

			if kind == Kind::Plus {
				device.write_lcd(context.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::ImageRgba8(final_img))?).await?;
			} else if kind == Kind::Neo {
				let format = kind.lcd_image_format().unwrap();
				let data = convert_image_with_format_async(format, image::DynamicImage::ImageRgba8(final_img))?;
				device.write_lcd_fill(&data).await?;
			}
		} else if let Some(image) = image {
			let dynamic = load_image_raw(image);
			if let Some(dynamic) = dynamic {
				if context.controller == "Encoder" {
					device
						.write_lcd(
							(context.position as u16 * 200) + 64,
							14,
							&ImageRect::from_image_async(dynamic.resize(72, 72, image::imageops::FilterType::Nearest))?,
						)
						.await?;
				} else if is_touch_point {
					let (r, g, b) = extract_average_colour(&dynamic);
					device.set_touchpoint_color(context.position - key_count, r, g, b).await?;
				} else {
					if let Err(e) = device.set_button_image(context.position, dynamic).await {
						log::error!("Failed to set button image for device {} position {}: {e}", context.device, context.position);
					}
				}
			} else {
				// If image was provided but failed to load, clear it or do nothing? 
				// To be safe and consistent with previous behavior, let's clear it if it's an empty string.
				if image.is_empty() {
					let _ = device.clear_button_image(context.position).await;
				}
			}
		} else if context.controller == "Encoder" {
			device
				.write_lcd(context.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::new_rgb8(200, 100))?)
				.await?;
		} else if is_touch_point {
			device.set_touchpoint_color(context.position - key_count, 0, 0, 0).await?;
		} else {
			device.clear_button_image(context.position).await?;
		}
		device.flush().await?;
	}
	Ok(())
}

/// Clear all touchpoint LEDs on a device by setting them to black.
async fn clear_all_touchpoints(device: &AsyncStreamDeck) {
	for i in 0..device.kind().touchpoint_count() {
		let _ = device.set_touchpoint_color(i, 0, 0, 0).await;
	}
}

pub async fn clear_screen(id: &str) -> Result<(), anyhow::Error> {
	crate::infobar_overlay::clear_device_overlays(id);
	if let Some(device) = ELGATO_DEVICES.read().await.get(id) {
		device.clear_all_button_images().await?;
		if device.kind() == Kind::Plus {
			device
				.write_lcd_fill(&convert_image_with_format_async(device.kind().lcd_image_format().unwrap(), image::DynamicImage::new_rgb8(800, 100))?)
				.await?;
		} else if device.kind() == Kind::Neo {
			device
				.write_lcd_fill(&convert_image_with_format_async(device.kind().lcd_image_format().unwrap(), image::DynamicImage::new_rgb8(248, 58))?)
				.await?;
		}
		clear_all_touchpoints(device).await;
		device.flush().await?;
	}
	Ok(())
}

pub async fn set_brightness(brightness: u8) {
	for (_id, device) in ELGATO_DEVICES.read().await.iter() {
		let _ = device.set_brightness(brightness.clamp(0, 100)).await;
		let _ = device.flush().await;
	}
}

pub async fn reset_devices() {
	for (_id, device) in ELGATO_DEVICES.read().await.iter() {
		let _ = device.reset().await;
		let _ = device.flush().await;
	}
}

async fn init(device: AsyncStreamDeck, device_id: String) {
	if ELGATO_DEVICES.read().await.contains_key(&device_id) {
		return;
	}

	let kind = device.kind();
	let device_type = match kind {
		Kind::Original | Kind::OriginalV2 | Kind::Mk2 | Kind::Mk2Scissor | Kind::Mk2Module => 0,
		Kind::Mini | Kind::MiniMk2 | Kind::MiniDiscord | Kind::MiniMk2Module => 1,
		Kind::Xl | Kind::XlV2 | Kind::XlV2Module => 2,
		Kind::Pedal => 5,
		Kind::Plus => 7,
		Kind::Neo => 9,
	};
	let _ = device.clear_all_button_images().await;
	clear_all_touchpoints(&device).await;
	if let Ok(settings) = crate::store::get_settings() {
		let _ = device.set_brightness(settings.value.brightness).await;
	}
	let _ = device.flush().await;
	let infobar_count = if kind == Kind::Neo {
		1
	} else if kind == Kind::Plus {
		4
	} else {
		0
	};

	crate::events::inbound::devices::register_device(
		"",
		crate::events::inbound::PayloadEvent {
			payload: crate::shared::DeviceInfo {
				id: device_id.clone(),
				plugin: String::new(),
				name: device.product().await.unwrap(),
				rows: kind.row_count(),
				columns: kind.column_count(),
				encoders: kind.encoder_count(),
				touchpoints: kind.touchpoint_count(),
				infobar: infobar_count,
				r#type: device_type,
			},
		},
	)
	.await
	.unwrap();

	let reader = device.get_reader();
	ELGATO_DEVICES.write().await.insert(device_id.clone(), device);
	loop {
		let updates = match reader.read(100.0).await {
			Ok(updates) => updates,
			Err(_) => break,
		};
		for update in updates {
			match match update {
				DeviceStateUpdate::ButtonDown(key) => keypad::key_down(&device_id, key).await,
				DeviceStateUpdate::ButtonUp(key) => keypad::key_up(&device_id, key).await,
				DeviceStateUpdate::TouchPointDown(point) => keypad::key_down(&device_id, kind.key_count() + point).await,
				DeviceStateUpdate::TouchPointUp(point) => keypad::key_up(&device_id, kind.key_count() + point).await,
				DeviceStateUpdate::EncoderTwist(dial, ticks) => encoder::dial_rotate(&device_id, dial, ticks.into()).await,
				DeviceStateUpdate::EncoderDown(dial) => encoder::dial_press(&device_id, "dialDown", dial).await,
				DeviceStateUpdate::EncoderUp(dial) => encoder::dial_press(&device_id, "dialUp", dial).await,
				// Plus LCD segment tap: map x coordinate to segment index and
				// send keyDown + keyUp to the corresponding Infobar action.
				DeviceStateUpdate::TouchScreenPress(x, _) | DeviceStateUpdate::TouchScreenLongPress(x, _) => {
					let segment = (x / 200).min(3) as u8;
					let _ = infobar::key_down(&device_id, segment).await;
					infobar::key_up(&device_id, segment).await
				}
				_ => Ok(()),
			} {
				Ok(_) => (),
				Err(error) => log::warn!("Failed to process device event {update:?}: {error}"),
			}
		}
	}

	ELGATO_DEVICES.write().await.remove(&device_id);
	crate::events::inbound::devices::deregister_device("", crate::events::inbound::PayloadEvent { payload: device_id })
		.await
		.unwrap();
}

/// Attempt to initialise all connected devices.
pub async fn initialise_devices() {
	if let Ok(settings) = crate::store::get_settings() {
		if settings.value.disableelgato {
			crate::plugins::DEVICE_NAMESPACES
				.write()
				.await
				.insert("sd".to_owned(), "opendeck_alternative_elgato_implementation".to_owned());
			return;
		} else {
			crate::plugins::DEVICE_NAMESPACES.write().await.remove("sd");
		}
	}

	// Iterate through detected Elgato devices and attempt to register them.
	let current = HIDAPI.read().await.as_ref().cloned();
	let hid = match current {
		Some(arc) => arc,
		None => match elgato_streamdeck::new_hidapi() {
			Ok(hid) => {
				let arc = Arc::new(hid);
				HIDAPI.write().await.replace(arc.clone());
				arc
			}
			Err(error) => {
				log::warn!("Failed to initialise hidapi: {error}");
				return;
			}
		},
	};
	for (kind, serial) in elgato_streamdeck::asynchronous::list_devices_async(&hid) {
		let device_id = format!("sd-{serial}");
		if ELGATO_DEVICES.read().await.contains_key(&device_id) {
			continue;
		}
		match elgato_streamdeck::AsyncStreamDeck::connect(&hid, kind, &serial) {
			Ok(device) => {
				tokio::spawn(init(device, device_id));
			}
			Err(error) => log::warn!("Failed to connect to Elgato device: {error}"),
		}
	}
}
