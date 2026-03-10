use super::{GenericInstancePayload, send_to_plugin};

use crate::shared::Context;
use crate::store::profiles::{acquire_locks_mut, get_slot_mut};

use serde::Serialize;

#[derive(Serialize)]
struct KeyEvent {
	event: &'static str,
	action: String,
	context: crate::shared::ActionContext,
	device: String,
	payload: GenericInstancePayload,
}

/// Send a `keyDown` event to the plugin whose action occupies the given
/// infobar segment.  Called when the user taps the Plus LCD strip.
pub async fn key_down(device: &str, position: u8) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	let selected_profile = locks.device_stores.get_selected_profile(device)?;
	let context = Context {
		device: device.to_owned(),
		profile: selected_profile,
		controller: "Infobar".to_owned(),
		position,
	};

	let Some(instance) = get_slot_mut(&context, &mut locks).await? else {
		return Ok(());
	};
	let target = if crate::infobar_stack::is_infobar_stack(instance) {
		match crate::infobar_stack::active_child(instance) {
			Some(child) => child,
			None => return Ok(()),
		}
	} else {
		instance
	};

	send_to_plugin(
		&target.action.plugin,
		&KeyEvent {
			event: "keyDown",
			action: target.action.uuid.clone(),
			context: target.context.clone(),
			device: target.context.device.clone(),
			payload: GenericInstancePayload::new(target),
		},
	)
	.await?;

	Ok(())
}

/// Send a `keyUp` event to the plugin whose action occupies the given
/// infobar segment.
pub async fn key_up(device: &str, position: u8) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;
	let selected_profile = locks.device_stores.get_selected_profile(device)?;
	let context = Context {
		device: device.to_owned(),
		profile: selected_profile,
		controller: "Infobar".to_owned(),
		position,
	};

	let Some(instance) = get_slot_mut(&context, &mut locks).await? else {
		return Ok(());
	};
	let target = if crate::infobar_stack::is_infobar_stack(instance) {
		match crate::infobar_stack::active_child(instance) {
			Some(child) => child,
			None => return Ok(()),
		}
	} else {
		instance
	};

	send_to_plugin(
		&target.action.plugin,
		&KeyEvent {
			event: "keyUp",
			action: target.action.uuid.clone(),
			context: target.context.clone(),
			device: target.context.device.clone(),
			payload: GenericInstancePayload::new(target),
		},
	)
	.await?;

	Ok(())
}
