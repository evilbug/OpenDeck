use super::PayloadEvent;

use crate::infobar_popover::InfobarComponent;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct ShowInfobarPopoverPayload {
	pub device: String,
	pub position: u8,
	pub priority: u8,
	/// How long to show the popover, in milliseconds.  0 = permanent until
	/// the screen is cleared or another overlay replaces it.
	pub duration_ms: u64,
	pub component: InfobarComponent,
}

pub async fn show_infobar_popover(event: PayloadEvent<ShowInfobarPopoverPayload>) -> Result<(), anyhow::Error> {
	let p = event.payload;

	let image = crate::infobar_popover::render_component(&p.component)?;

	let overlay_id = crate::infobar_overlay::push_overlay(&p.device, p.position, image, p.priority, p.duration_ms);

	if p.duration_ms > 0 {
		let device = p.device.clone();
		let priority = p.priority;
		let position = p.position;
		let duration = p.duration_ms;
		tokio::spawn(async move {
			tokio::time::sleep(std::time::Duration::from_millis(duration)).await;
			crate::infobar_overlay::expire_and_rerender(device, position, priority, overlay_id).await;
		});
	}

	// Trigger an immediate render.  The overlay is now stored so update_image
	// will pick it up as the effective image for this slot.
	let context = crate::shared::Context {
		device: p.device,
		profile: String::new(),
		controller: "Infobar".to_string(),
		position: p.position,
	};
	let _ = crate::elgato::update_image(&context, None).await;

	Ok(())
}
