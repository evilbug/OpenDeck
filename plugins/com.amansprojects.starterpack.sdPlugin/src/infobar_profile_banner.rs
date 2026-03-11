use ab_glyph::{FontRef, PxScale};
use base64::{Engine as _, engine::general_purpose};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use openaction::*;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ProfileBannerSettings {
	/// Which infobar position to push the overlay to.
	pub target_position: u8,
	/// How long to show the banner, in milliseconds.  Defaults to 3000.
	pub duration_ms: u64,
}

impl Default for ProfileBannerSettings {
	fn default() -> Self {
		Self {
			target_position: 0,
			duration_ms: 3000,
		}
	}
}

pub struct ProfileBannerAction;

/// Extract the profile name from an openaction instance ID.
///
/// Instance IDs are formatted as `device.profile.controller.position.index`.
/// Since device IDs never contain dots (e.g. `sd-SERIAL`) and the last three
/// segments are always `controller`, `position` (number), and `index`
/// (number), the profile name is everything in between.
fn extract_profile_name(instance_id: &str) -> String {
	let parts: Vec<&str> = instance_id.split('.').collect();
	if parts.len() >= 5 {
		parts[1..parts.len() - 3].join(".")
	} else {
		"Profile".to_string()
	}
}

fn make_infobar_context(instance_id: &str, position: u8) -> String {
	let mut parts: Vec<String> = instance_id.split('.').map(|part| part.to_owned()).collect();
	if parts.len() >= 5 {
		let index = parts.len() - 1;
		let position_index = parts.len() - 2;
		let controller_index = parts.len() - 3;
		parts[controller_index] = "Infobar".to_owned();
		parts[position_index] = position.to_string();
		parts[index] = "0".to_owned();
		parts.join(".")
	} else {
		instance_id.to_owned()
	}
}

fn generate_banner_image(profile_name: &str) -> Result<String, String> {
	// 248×58 matches the Stream Deck Neo infobar dimensions.
	let width = 248u32;
	let height = 58u32;
	let mut image = RgbaImage::new(width, height);

	// Dark blue background.
	for pixel in image.pixels_mut() {
		*pixel = Rgba([0, 0, 80, 255]);
	}

	let font_data = include_bytes!("../assets/fonts/Roboto-Regular.ttf");
	let font = FontRef::try_from_slice(font_data).map_err(|e| e.to_string())?;

	let scale = PxScale { x: 36.0, y: 36.0 };
	draw_text_mut(
		&mut image,
		Rgba([255, 255, 255, 255]),
		10,
		11,
		scale,
		&font,
		profile_name,
	);

	let mut buffer = Cursor::new(Vec::new());
	image
		.write_to(&mut buffer, image::ImageFormat::Png)
		.map_err(|e| e.to_string())?;
	let b64 = general_purpose::STANDARD.encode(buffer.into_inner());
	Ok(format!("data:image/png;base64,{b64}"))
}

#[async_trait]
impl Action for ProfileBannerAction {
	const UUID: ActionUuid = "com.amansprojects.starterpack.infobar.profilebanner";
	type Settings = ProfileBannerSettings;

	async fn will_appear(
		&self,
		instance: &Instance,
		settings: &Self::Settings,
	) -> OpenActionResult<()> {
		let profile_name = extract_profile_name(&instance.instance_id.to_string());

		let banner_image = match generate_banner_image(&profile_name) {
			Ok(img) => img,
			Err(e) => {
				log::error!("ProfileBanner: failed to generate image: {e}");
				return Ok(());
			}
		};

		let duration = if settings.duration_ms == 0 {
			3000
		} else {
			settings.duration_ms
		};

		send_arbitrary_json(serde_json::json!({
			"event": "setInfobarImage",
			"payload": {
				"context": make_infobar_context(&instance.instance_id.to_string(), settings.target_position),
				"image": banner_image,
				"priority": 100u8,
				"duration_ms": duration,
			}
		}))
		.await?;

		Ok(())
	}

	async fn key_down(
		&self,
		_instance: &Instance,
		_settings: &Self::Settings,
	) -> OpenActionResult<()> {
		Ok(())
	}
}
