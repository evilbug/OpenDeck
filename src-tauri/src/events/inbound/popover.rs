use super::PayloadEvent;

use crate::infobar_popover::InfobarComponent;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct ShowInfobarPopoverPayload {
	pub context: crate::shared::ActionContext,
	pub priority: u8,
	/// How long to show the popover, in milliseconds.  0 = permanent until
	/// the screen is cleared or another overlay replaces it.
	pub duration_ms: u64,
	pub component: InfobarComponent,
}

pub async fn show_infobar_popover(event: PayloadEvent<ShowInfobarPopoverPayload>) -> Result<(), anyhow::Error> {
	let p = event.payload;

	let image = crate::infobar_popover::render_component(&p.component)?;
	let scroll_width = crate::infobar_popover::get_scroll_width(&p.component);

	let overlay_id = crate::infobar_overlay::push_overlay(&p.context.device, p.context.position, image, p.priority, p.duration_ms);

	if scroll_width > 0.0 {
		log::info!("Scrolling infobar popover ({}px) on device {} at position {}", scroll_width, p.context.device, p.context.position);
		let device = p.context.device.clone();
		let priority = p.priority;
		let position = p.context.position;
		let profile = p.context.profile.clone();
		let controller = p.context.controller.clone();
		let component = p.component.clone();
		tokio::spawn(async move {
			// 1. Initial pause at truncated position (already pushed above).
			tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

			// 2. Scroll to the end.
			let step_size = 5.0f32;
			let steps = (scroll_width / step_size).ceil() as u32;
			for step in 1..=steps {
				let offset = (step as f32 * step_size).min(scroll_width);
				if let Ok(frame) = crate::infobar_popover::render_component_at(&component, Some(offset)) {
					if !crate::infobar_overlay::update_overlay_image(&device, position, priority, overlay_id, frame) {
						return; // Overlay was replaced.
					}
					let context = crate::shared::Context {
						device: device.clone(),
						profile: profile.clone(),
						controller: controller.clone(),
						position,
					};
					let _ = crate::events::outbound::devices::update_image(context, None).await;
				}
				tokio::time::sleep(std::time::Duration::from_millis(40)).await;
			}

			// 3. Pause at the end.
			tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

			// 4. Revert to truncated position.
			if let Ok(final_frame) = crate::infobar_popover::render_component(&component) {
				if crate::infobar_overlay::update_overlay_image(&device, position, priority, overlay_id, final_frame) {
					let context = crate::shared::Context {
						device: device.clone(),
						profile: profile.clone(),
						controller: controller.clone(),
						position,
					};
					let _ = crate::events::outbound::devices::update_image(context, None).await;
				}
			}
		});
	}

	if p.duration_ms > 0 {
		let device = p.context.device.clone();
		let priority = p.priority;
		let position = p.context.position;
		let duration = p.duration_ms;
		tokio::spawn(async move {
			tokio::time::sleep(std::time::Duration::from_millis(duration)).await;
			crate::infobar_overlay::expire_and_rerender(device, position, priority, overlay_id).await;
		});
	}

	// Trigger an immediate render.  The overlay is now stored so update_image
	// will pick it up as the effective image for this slot.
	let context = crate::shared::Context {
		device: p.context.device,
		profile: p.context.profile,
		controller: p.context.controller,
		position: p.context.position,
	};
	let _ = crate::elgato::update_image(&context, None).await;

	Ok(())
}
