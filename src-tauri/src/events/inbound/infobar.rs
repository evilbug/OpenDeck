use super::PayloadEvent;

use crate::events::frontend::instances::update_state;
use crate::store::profiles::acquire_locks_mut;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct SetInfobarImagePayload {
	pub context: crate::shared::ActionContext,
	pub image: String,
	pub priority: u8,
	/// How long to show the overlay, in milliseconds.  0 = permanent until cleared.
	pub duration_ms: u64,
}

pub async fn set_infobar_image(event: PayloadEvent<SetInfobarImagePayload>) -> Result<(), anyhow::Error> {
	let p = event.payload;

	let overlay_id = crate::infobar_overlay::push_overlay(&p.context.device, p.context.position, p.image, p.priority, p.duration_ms);

	// Schedule removal and rerender after the TTL elapses.
	if p.duration_ms > 0 {
		let device = p.context.device.clone();
		let duration = p.duration_ms;
		let priority = p.priority;
		let position = p.context.position;
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

#[derive(Deserialize)]
pub struct ClearInfobarOverlayPayload {
	pub context: crate::shared::ActionContext,
}

#[derive(Deserialize)]
pub struct SetInfobarComponentPayload {
	pub context: crate::shared::ActionContext,
	pub component: crate::infobar_popover::InfobarComponent,
}

pub async fn set_infobar_component(event: PayloadEvent<SetInfobarComponentPayload>) -> Result<(), anyhow::Error> {
	let p = event.payload;
	crate::infobar_stack::set_component(p.context.clone(), p.component.clone());
	crate::shared::INFOBAR_COMPONENTS.insert((p.context.device.clone(), p.context.position), p.component);

	let mut locks = acquire_locks_mut().await;
	update_state(crate::APP_HANDLE.get().unwrap(), p.context.clone(), &mut locks).await?;
	if let Some(parent_context) = crate::infobar_stack::sync_parent_display(&(&p.context).into(), &mut locks).await? {
		update_state(crate::APP_HANDLE.get().unwrap(), parent_context, &mut locks).await?;
	}

	let context = crate::shared::Context {
		device: p.context.device,
		profile: p.context.profile,
		controller: p.context.controller,
		position: p.context.position,
	};
	let _ = crate::elgato::update_image(&context, None).await;
	Ok(())
}

/// Remove all overlays at a position and rerender from the stored action image.
pub async fn clear_infobar_overlay(event: PayloadEvent<ClearInfobarOverlayPayload>) -> Result<(), anyhow::Error> {
	let p = event.payload;
	crate::infobar_overlay::clear_position_overlays(p.context.device, p.context.position).await;
	Ok(())
}
