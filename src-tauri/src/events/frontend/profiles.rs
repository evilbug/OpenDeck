use super::Error;

use crate::shared::DEVICES;
use crate::store::profiles::{PROFILE_STORES, acquire_locks_mut, get_device_profiles};

use tauri::{AppHandle, Emitter, Manager, command};

#[command]
pub fn get_profiles(device: &str) -> Result<Vec<String>, Error> {
	Ok(get_device_profiles(device)?)
}

#[command]
pub async fn get_selected_profile(device: String) -> Result<crate::shared::Profile, Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let selected_profile = locks.device_stores.get_selected_profile(&device)?;
	let context_template = crate::shared::Context {
		device: device.clone(),
		profile: selected_profile.clone(),
		controller: "Infobar".to_owned(),
		position: 0,
	};
	let infobar_len = DEVICES.get(&device).unwrap().infobar;
	for position in 0..infobar_len {
		let _ = crate::infobar_stack::sync_parent_display(&crate::shared::Context { position, ..context_template.clone() }, &mut locks).await;
	}
	let profile = locks.profile_stores.get_profile_store(&DEVICES.get(&device).unwrap(), &selected_profile)?;

	Ok(profile.value.clone())
}

#[allow(clippy::flat_map_identity)]
#[command]
pub async fn set_selected_profile(device: String, id: String) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	let device_info = DEVICES.get(&device).unwrap().clone();
	let infobar_segments = device_info.infobar;
	let mut old_instances: Vec<crate::shared::ActionInstance> = vec![];
	let mut new_instances: Vec<crate::shared::ActionInstance> = vec![];
	let mut old_infobar_action_uuids = vec![None; infobar_segments as usize];
	let mut new_infobar_action_uuids = vec![None; infobar_segments as usize];
	let old_infobar_components: Vec<Option<String>> = (0..infobar_segments)
		.map(|position| {
			crate::shared::INFOBAR_COMPONENTS
				.get(&(device.clone(), position))
				.and_then(|component| serde_json::to_string(&*component).ok())
		})
		.collect();

	let selected_profile = locks.device_stores.get_selected_profile(&device)?;

	if selected_profile != id {
		let old_profile = &locks.profile_stores.get_profile_store(&device_info, &selected_profile)?.value;
		old_instances = old_profile
			.keys
			.iter()
			.flatten()
			.chain(old_profile.sliders.iter().flatten())
			.chain(old_profile.infobar.iter().flatten())
			.cloned()
			.collect();
		old_infobar_action_uuids = old_profile
			.infobar
			.iter()
			.map(|slot| slot.as_ref().map(|instance| instance.action.uuid.clone()))
			.collect();
		crate::infobar_overlay::clear_device_overlays(&device);
	}

	let store = locks.profile_stores.get_profile_store_mut(&device_info, &id).await?;
	let new_profile = &store.value;
	new_instances = new_profile
		.keys
		.iter()
		.flatten()
		.chain(new_profile.sliders.iter().flatten())
		.chain(new_profile.infobar.iter().flatten())
		.cloned()
		.collect();
	new_infobar_action_uuids = new_profile
		.infobar
		.iter()
		.map(|slot| slot.as_ref().map(|instance| instance.action.uuid.clone()))
		.collect();
	store.save()?;

	locks.device_stores.set_selected_profile(&device, id.clone())?;

	for position in 0..infobar_segments {
		let preserve_existing_render = selected_profile != id
			&& old_infobar_action_uuids.get(position as usize) == new_infobar_action_uuids.get(position as usize)
			&& new_infobar_action_uuids.get(position as usize).and_then(|v| v.as_ref()).is_some();
		if preserve_existing_render {
			continue;
		}

		let context = crate::shared::Context {
			device: device.clone(),
			profile: id.clone(),
			controller: "Infobar".to_owned(),
			position,
		};
		let _ = crate::infobar_stack::sync_parent_display(&context, &mut locks).await;

		if let Ok(Some(instance)) = crate::store::profiles::get_slot_mut(&context, &mut locks).await {
			if !crate::infobar_stack::is_infobar_stack(instance) {
				let state = &instance.states[instance.current_state as usize];
				crate::shared::INFOBAR_IMAGES.insert(
					(device.clone(), position),
					crate::shared::resolve_state_image(&state.image, &instance.action.icon),
				);
				crate::shared::INFOBAR_TEXT.insert((device.clone(), position), state.text.clone());
			}
		} else {
			crate::shared::INFOBAR_IMAGES.remove(&(device.clone(), position));
			crate::shared::INFOBAR_TEXT.remove(&(device.clone(), position));
			crate::shared::INFOBAR_COMPONENTS.remove(&(device.clone(), position));
		}
	}

	let new_infobar_components: Vec<Option<String>> = (0..infobar_segments)
		.map(|position| {
			crate::shared::INFOBAR_COMPONENTS
				.get(&(device.clone(), position))
				.and_then(|component| serde_json::to_string(&*component).ok())
		})
		.collect();
	let new_infobar_scrollable: Vec<bool> = (0..infobar_segments)
		.map(|position| {
			crate::shared::INFOBAR_COMPONENTS
				.get(&(device.clone(), position))
				.map(|component| crate::infobar_popover::get_scroll_width(&component) > 0.0)
				.unwrap_or(false)
		})
		.collect();

	let mut slot_updates: Vec<(crate::shared::Context, Option<String>)> = vec![];
	if selected_profile != id {
		for position in 0..(device_info.rows * device_info.columns + device_info.touchpoints) {
			let context = crate::shared::Context {
				device: device.clone(),
				profile: id.clone(),
				controller: "Keypad".to_owned(),
				position,
			};
			let image = if let Ok(Some(instance)) = crate::store::profiles::get_slot_mut(&context, &mut locks).await {
				instance.states.get(instance.current_state as usize).map(|state| state.image.clone())
			} else {
				None
			};
			slot_updates.push((context, image));
		}

		for position in 0..device_info.encoders {
			let context = crate::shared::Context {
				device: device.clone(),
				profile: id.clone(),
				controller: "Encoder".to_owned(),
				position,
			};
			let image = if let Ok(Some(instance)) = crate::store::profiles::get_slot_mut(&context, &mut locks).await {
				instance.states.get(instance.current_state as usize).map(|state| state.image.clone())
			} else {
				None
			};
			slot_updates.push((context, image));
		}

		for position in 0..infobar_segments {
			let preserve_existing_render = old_infobar_action_uuids.get(position as usize) == new_infobar_action_uuids.get(position as usize)
				&& new_infobar_action_uuids.get(position as usize).and_then(|v| v.as_ref()).is_some();
			if preserve_existing_render {
				continue;
			}

			let unchanged_component = old_infobar_components.get(position as usize) == new_infobar_components.get(position as usize);
			let needs_scroll_refresh = new_infobar_scrollable.get(position as usize).copied().unwrap_or(false);
			if unchanged_component && !needs_scroll_refresh {
				continue;
			}
			slot_updates.push((
				crate::shared::Context {
					device: device.clone(),
					profile: id.clone(),
					controller: "Infobar".to_owned(),
					position,
				},
				None,
			));
		}
	}

	drop(locks);

	for instance in &old_instances {
		let _ = crate::events::outbound::will_appear::will_disappear_tree(instance, false).await;
	}
	for instance in &new_instances {
		let _ = crate::events::outbound::will_appear::will_appear_tree(instance).await;
	}

	for (context, image) in slot_updates {
		let _ = crate::events::outbound::devices::update_image(context, image).await;
	}

	let settings = crate::store::get_settings()?.value;
	if selected_profile != id && settings.show_profile_switch_pill && infobar_segments > 0 {
		for position in 0..infobar_segments {
			let _ = crate::events::inbound::popover::show_infobar_popover(crate::events::inbound::PayloadEvent {
				payload: crate::events::inbound::popover::ShowInfobarPopoverPayload {
					context: crate::shared::ActionContext {
						device: device.clone(),
						profile: id.clone(),
						controller: "Infobar".to_string(),
						position,
						index: 0,
					},
					priority: 230,
					duration_ms: 1500,
					component: crate::infobar_popover::InfobarComponent::Pill { text: id.clone() },
				},
			})
			.await;
		}
	}

	Ok(())
}

#[command]
pub async fn delete_profile(device: String, profile: String) {
	let mut profile_stores = PROFILE_STORES.write().await;
	profile_stores.delete_profile(&device, &profile);
}

#[command]
pub async fn rename_profile(device: String, old_id: String, new_id: String) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	if !DEVICES.contains_key(&device) {
		return Err(Error::new(format!("device {device} not found")));
	}

	locks.profile_stores.rename_profile(&DEVICES.get(&device).unwrap(), &old_id, &new_id).await?;

	Ok(())
}

pub async fn rerender_images(app: &AppHandle) -> Result<(), anyhow::Error> {
	let window = app.get_webview_window("main").unwrap();
	window.emit("rerender_images", ())?;
	Ok(())
}
