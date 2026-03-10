use dashmap::DashMap;
use crate::events::outbound::{encoder, infobar, keypad};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;

use base64::Engine as _;
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

			let base_uri = crate::shared::INFOBAR_IMAGES.get(&(context.device.clone(), context.position)).map(|v| v.clone());
			
			let mut base_img = image::RgbaImage::new(w, h);
			if let Some(uri) = base_uri {
				if INFOBAR_BASE_CACHE.len() > 100 {
					INFOBAR_BASE_CACHE.clear();
				}
				let cached = INFOBAR_BASE_CACHE.get(&(context.device.clone(), context.position));
				if let Some(c) = cached && c.0 == uri {
					base_img = c.1.clone();
				} else {
					let bytes = base64::engine::general_purpose::STANDARD.decode(uri.split_once(',').unwrap().1)?;
					let dynamic = image::load_from_memory(&bytes)?;
					let processed = if dynamic.width() == w && dynamic.height() == h {
						dynamic.into_rgba8()
					} else {
						dynamic.resize_exact(w, h, image::imageops::FilterType::Lanczos3).into_rgba8()
					};
					INFOBAR_BASE_CACHE.insert((context.device.clone(), context.position), (uri, processed.clone()));
					base_img = processed;
				}
			}

			// Render text from INFOBAR_TEXT if present.
			if let Some(text) = crate::shared::INFOBAR_TEXT.get(&(context.device.clone(), context.position)) && !text.is_empty() {
				let component = crate::infobar_popover::InfobarComponent::Text { text: text.clone() };
				let scroll_width = crate::infobar_popover::get_scroll_width(&component);
				
				// Render initial (truncated) text.
				if let Ok(text_uri) = crate::infobar_popover::render_component(&component) {
					let bytes = base64::engine::general_purpose::STANDARD.decode(text_uri.split_once(',').unwrap().1)?;
					let text_img = image::load_from_memory(&bytes)?.into_rgba8();
					image::imageops::overlay(&mut base_img, &text_img, 0, 0);
				}

				// If it's too long, start/restart a scroll task if not already scrolling this text.
				if scroll_width > 0.0 {
					let current_scroll_text = SCROLL_TASKS.get(&(context.device.clone(), context.position)).map(|v| v.clone());
					if current_scroll_text.as_ref() != Some(&*text) {
						SCROLL_TASKS.insert((context.device.clone(), context.position), text.clone());
						let ctx = context.clone();
						let text_val = text.clone();
						tokio::spawn(async move {
							tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
							let step_size = 4.0f32;
							let mut offset = 0.0f32;
							loop {
								// Check if we should still be scrolling this text.
								if SCROLL_TASKS.get(&(ctx.device.clone(), ctx.position)).as_deref() != Some(&text_val) {
									break;
								}

								offset += step_size;
								if offset > scroll_width {
									tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
									offset = 0.0;
								}

								let frame_res = crate::infobar_popover::render_component_at(&component, Some(offset));
								if let Ok(frame_uri) = frame_res {
									// We bypass update_image to avoid recursion and just push to LCD.
									if let Some(device) = ELGATO_DEVICES.read().await.get(&ctx.device) {
										// Re-render base + this text frame + other overlays.
										let mut scroll_img = image::RgbaImage::new(w, h);
										let base_uri = crate::shared::INFOBAR_IMAGES.get(&(ctx.device.clone(), ctx.position)).map(|v| v.clone());
										if let Some(uri) = base_uri {
											if let Some(c) = INFOBAR_BASE_CACHE.get(&(ctx.device.clone(), ctx.position)) && c.0 == uri {
												scroll_img = c.1.clone();
											}
										}

										// The scrolling text frame
										let text_bytes = base64::engine::general_purpose::STANDARD.decode(frame_uri.split_once(',').unwrap().1).unwrap();
										let text_frame = image::load_from_memory(&text_bytes).unwrap().into_rgba8();
										image::imageops::overlay(&mut scroll_img, &text_frame, 0, 0);

										// Overlays (Popovers) on top
										if let Some(entries) = crate::infobar_overlay::OVERLAYS.get(&(ctx.device.clone(), ctx.position)) {
											let now = std::time::Instant::now();
											for entry in entries.values() {
												if entry.expires_at.is_none_or(|exp| exp > now) {
													if let Some(c) = INFOBAR_OVERLAY_CACHE.get(&(ctx.device.clone(), ctx.position, entry.id)) && c.0 == entry.image {
														image::imageops::overlay(&mut scroll_img, &c.1, 0, 0);
														break;
													}
												}
											}
										}

										if device.kind() == Kind::Plus {
											let _ = device.write_lcd(ctx.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::ImageRgba8(scroll_img)).unwrap()).await;
										} else if device.kind() == Kind::Neo {
											let format = device.kind().lcd_image_format().unwrap();
											let data = convert_image_with_format_async(format, image::DynamicImage::ImageRgba8(scroll_img)).unwrap();
											let _ = device.write_lcd_fill(&data).await;
										}
										let _ = device.flush().await;
									}
								}
								tokio::time::sleep(std::time::Duration::from_millis(50)).await;
							}
						});
					}
				} else {
					SCROLL_TASKS.remove(&(context.device.clone(), context.position));
				}
			} else {
				SCROLL_TASKS.remove(&(context.device.clone(), context.position));
			}

			if let Some(entries) = crate::infobar_overlay::OVERLAYS.get(&(context.device.clone(), context.position)) {
				let now = std::time::Instant::now();
				for entry in entries.values() {
					if entry.expires_at.is_none_or(|exp| exp > now) {
						if INFOBAR_OVERLAY_CACHE.len() > 200 {
							INFOBAR_OVERLAY_CACHE.clear();
						}
						let cached = INFOBAR_OVERLAY_CACHE.get(&(context.device.clone(), context.position, entry.id));
						let overlay_img = if let Some(c) = cached && c.0 == entry.image {
							c.1.clone()
						} else {
							let bytes = base64::engine::general_purpose::STANDARD.decode(entry.image.split_once(',').unwrap().1)?;
							let dynamic = image::load_from_memory(&bytes)?;
							let processed = if dynamic.width() == w && dynamic.height() == h {
								dynamic.into_rgba8()
							} else {
								dynamic.resize_exact(w, h, image::imageops::FilterType::Lanczos3).into_rgba8()
							};
							INFOBAR_OVERLAY_CACHE.insert((context.device.clone(), context.position, entry.id), (entry.image.clone(), processed.clone()));
							processed
						};
						image::imageops::overlay(&mut base_img, &overlay_img, 0, 0);
						break;
					}
				}
			}

			if kind == Kind::Plus {
				device.write_lcd(context.position as u16 * 200, 0, &ImageRect::from_image_async(image::DynamicImage::ImageRgba8(base_img))?).await?;
			} else if kind == Kind::Neo {
				let format = kind.lcd_image_format().unwrap();
				let data = convert_image_with_format_async(format, image::DynamicImage::ImageRgba8(base_img))?;
				device.write_lcd_fill(&data).await?;
			}
		} else if let Some(image) = image {
			let data = image.split_once(',').unwrap().1;
			let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
			if context.controller == "Encoder" {
				device
					.write_lcd(
						(context.position as u16 * 200) + 64,
						14,
						&ImageRect::from_image_async(image::load_from_memory(&bytes)?.resize(72, 72, image::imageops::FilterType::Nearest))?,
					)
					.await?;
			} else if is_touch_point {
				let (r, g, b) = extract_average_colour(&image::load_from_memory(&bytes)?);
				device.set_touchpoint_color(context.position - key_count, r, g, b).await?;
			} else {
				device.set_button_image(context.position, image::load_from_memory(&bytes)?).await?;
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
