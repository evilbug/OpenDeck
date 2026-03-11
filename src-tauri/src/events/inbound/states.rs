use super::ContextAndPayloadEvent;

use crate::events::frontend::instances::update_state;
use crate::store::profiles::{acquire_locks_mut, get_instance_mut, save_profile};

use serde::Deserialize;

#[derive(Deserialize)]
pub struct SetTitlePayload {
	title: Option<String>,
	state: Option<u16>,
}

#[derive(Deserialize)]
pub struct SetImagePayload {
	image: Option<String>,
	state: Option<u16>,
}

#[derive(Deserialize)]
pub struct SetStatePayload {
	state: u16,
}

pub async fn set_title(event: ContextAndPayloadEvent<SetTitlePayload>) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;

	let (context, text, image, is_infobar) = if let Some(instance) = get_instance_mut(&event.context, &mut locks).await? {
		if let Some(state) = event.payload.state {
			if state as usize >= instance.states.len() {
				return Err(anyhow::anyhow!("State index out of bounds ({} > {})", state, instance.states.len() - 1));
			}
			instance.states[state as usize].text = event.payload.title.clone().unwrap_or(instance.action.states[state as usize].text.clone());
		} else {
			for (index, state) in instance.states.iter_mut().enumerate() {
				state.text = event.payload.title.clone().unwrap_or(instance.action.states[index].text.clone());
			}
		}
		let context = instance.context.clone();
		let text = instance.states[instance.current_state as usize].text.clone();
		let image = instance.states[instance.current_state as usize].image.clone();
		let is_infobar = instance.context.controller == "Infobar";
		(context, text, image, is_infobar)
	} else {
		return Ok(());
	};

	update_state(crate::APP_HANDLE.get().unwrap(), context.clone(), &mut locks).await?;

	if is_infobar {
		crate::shared::INFOBAR_COMPONENTS.remove(&(context.device.clone(), context.position));
		crate::elgato::clear_infobar_component_scroll_state(&context.device, context.position);
		crate::shared::INFOBAR_TEXT.insert((context.device.clone(), context.position), text);
		crate::shared::INFOBAR_IMAGES.insert((context.device.clone(), context.position), image);
		let ctx: crate::shared::Context = (&context).into();
		let _ = crate::events::outbound::devices::update_image(ctx, None).await;
	} else {
		// For non-infobar buttons, render the updated image
		let ctx: crate::shared::Context = (&context).into();
		if let Err(e) = crate::events::outbound::devices::update_image(ctx, Some(image)).await {
			log::error!("Failed to update image for button: {}", e);
		}
	}

	save_profile(&event.context.device, &mut locks).await?;

	Ok(())
}

pub async fn set_image(mut event: ContextAndPayloadEvent<SetImagePayload>) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;

	let (context, text, image, is_infobar) = if let Some(instance) = get_instance_mut(&event.context, &mut locks).await? {
		if let Some(image) = &event.payload.image {
			if image.trim().is_empty() {
				event.payload.image = None;
			} else if !image.trim().starts_with("data:") {
				event.payload.image = Some(crate::shared::convert_icon(
					crate::shared::config_dir()
						.join("plugins")
						.join(&instance.action.plugin)
						.join(image.trim())
						.to_str()
						.unwrap()
						.to_owned(),
				));
			}
		}

		if let Some(state) = event.payload.state {
			if state as usize >= instance.states.len() {
				return Err(anyhow::anyhow!("State index out of bounds ({} > {})", state, instance.states.len() - 1));
			}
			instance.states[state as usize].image = event.payload.image.clone().unwrap_or(instance.action.states[state as usize].image.clone());
		} else {
			for (index, state) in instance.states.iter_mut().enumerate() {
				state.image = event.payload.image.clone().unwrap_or(instance.action.states[index].image.clone());
			}
		}
		let context = instance.context.clone();
		let text = instance.states[instance.current_state as usize].text.clone();
		let image = instance.states[instance.current_state as usize].image.clone();
		let is_infobar = instance.context.controller == "Infobar";
		(context, text, image, is_infobar)
	} else {
		return Ok(());
	};

	update_state(crate::APP_HANDLE.get().unwrap(), context.clone(), &mut locks).await?;

	if is_infobar {
		crate::shared::INFOBAR_COMPONENTS.remove(&(context.device.clone(), context.position));
		crate::elgato::clear_infobar_component_scroll_state(&context.device, context.position);
		crate::shared::INFOBAR_TEXT.insert((context.device.clone(), context.position), text);
		crate::shared::INFOBAR_IMAGES.insert((context.device.clone(), context.position), image);
		let ctx: crate::shared::Context = (&context).into();
		let _ = crate::events::outbound::devices::update_image(ctx, None).await;
	} else {
		// For non-infobar buttons, render the updated image
		let ctx: crate::shared::Context = (&context).into();
		if let Err(e) = crate::events::outbound::devices::update_image(ctx, Some(image)).await {
			log::error!("Failed to update image for button: {}", e);
		}
	}

	save_profile(&event.context.device, &mut locks).await?;

	Ok(())
}

pub async fn set_state(event: ContextAndPayloadEvent<SetStatePayload>) -> Result<(), anyhow::Error> {
	let mut locks = acquire_locks_mut().await;

	let (context, text, image, is_infobar) = if let Some(instance) = get_instance_mut(&event.context, &mut locks).await? {
		if event.payload.state >= instance.states.len() as u16 {
			return Ok(());
		}
		instance.current_state = event.payload.state;
		let context = instance.context.clone();
		let text = instance.states[instance.current_state as usize].text.clone();
		let image = instance.states[instance.current_state as usize].image.clone();
		let is_infobar = instance.context.controller == "Infobar";
		(context, text, image, is_infobar)
	} else {
		return Ok(());
	};

	update_state(crate::APP_HANDLE.get().unwrap(), context.clone(), &mut locks).await?;

	if is_infobar {
		crate::shared::INFOBAR_COMPONENTS.remove(&(context.device.clone(), context.position));
		crate::elgato::clear_infobar_component_scroll_state(&context.device, context.position);
		crate::shared::INFOBAR_TEXT.insert((context.device.clone(), context.position), text);
		crate::shared::INFOBAR_IMAGES.insert((context.device.clone(), context.position), image);
		let ctx: crate::shared::Context = (&context).into();
		let _ = crate::events::outbound::devices::update_image(ctx, None).await;
	} else {
		// For non-infobar buttons, render the updated image
		let ctx: crate::shared::Context = (&context).into();
		if let Err(e) = crate::events::outbound::devices::update_image(ctx, Some(image)).await {
			log::error!("Failed to update image for button after set_state: {}", e);
		}
	}

	save_profile(&event.context.device, &mut locks).await?;

	Ok(())
}
