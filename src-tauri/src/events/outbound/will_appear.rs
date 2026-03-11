use super::{GenericInstancePayload, send_to_plugin};

use crate::shared::{ActionContext, ActionInstance};

fn clear_infobar_slot_state(context: &ActionContext) {
	if context.controller != "Infobar" {
		return;
	}

	let key = (context.device.clone(), context.position);
	crate::shared::INFOBAR_IMAGES.remove(&key);
	crate::shared::INFOBAR_TEXT.remove(&key);
	crate::shared::INFOBAR_COMPONENTS.remove(&key);
	crate::infobar_overlay::OVERLAYS.remove(&key);
	crate::elgato::clear_infobar_component_scroll_state(&context.device, context.position);
}

#[derive(serde::Serialize)]
struct AppearEvent {
	event: &'static str,
	action: String,
	context: ActionContext,
	device: String,
	payload: GenericInstancePayload,
}

pub async fn will_appear(instance: &ActionInstance) -> Result<(), anyhow::Error> {
	send_to_plugin(
		&instance.action.plugin,
		&AppearEvent {
			event: "willAppear",
			action: instance.action.uuid.clone(),
			context: instance.context.clone(),
			device: instance.context.device.clone(),
			payload: GenericInstancePayload::new(instance),
		},
	)
	.await?;

	super::states::title_parameters_did_change(instance, instance.current_state).await?;

	Ok(())
}

pub async fn will_disappear(instance: &ActionInstance, clear_on_device: bool) -> Result<(), anyhow::Error> {
	crate::infobar_stack::clear_visibility(&instance.context);
	crate::infobar_stack::clear_component(&instance.context);

	send_to_plugin(
		&instance.action.plugin,
		&AppearEvent {
			event: "willDisappear",
			action: instance.action.uuid.clone(),
			context: instance.context.clone(),
			device: instance.context.device.clone(),
			payload: GenericInstancePayload::new(instance),
		},
	)
	.await?;

	if clear_on_device {
		clear_infobar_slot_state(&instance.context);
	}

	if clear_on_device && let Err(error) = crate::events::outbound::devices::update_image((&instance.context).into(), None).await {
		log::warn!("Failed to clear device image: {}", error);
	}

	Ok(())
}

pub async fn will_appear_tree(instance: &ActionInstance) -> Result<(), anyhow::Error> {
	if crate::infobar_stack::is_infobar_stack(instance) {
		if let Some(children) = &instance.children {
			for child in children {
				will_appear(child).await?;
			}
		}
	} else {
		will_appear(instance).await?;
	}

	Ok(())
}

pub async fn will_disappear_tree(instance: &ActionInstance, clear_on_device: bool) -> Result<(), anyhow::Error> {
	if crate::infobar_stack::is_infobar_stack(instance) {
		if let Some(children) = &instance.children {
			for child in children {
				will_disappear(child, false).await?;
			}
		}
		if clear_on_device {
			clear_infobar_slot_state(&instance.context);
		}
		if clear_on_device && let Err(error) = crate::events::outbound::devices::update_image((&instance.context).into(), None).await {
			log::warn!("Failed to clear device image: {}", error);
		}
	} else {
		will_disappear(instance, clear_on_device).await?;
	}

	Ok(())
}
