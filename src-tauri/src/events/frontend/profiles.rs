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

	let selected_profile = locks.device_stores.get_selected_profile(&device)?;

	if selected_profile != id {
		let old_profile = &locks.profile_stores.get_profile_store(&DEVICES.get(&device).unwrap(), &selected_profile)?.value;
		for instance in old_profile
			.keys
			.iter()
			.flatten()
			.chain(&mut old_profile.sliders.iter().flatten())
			.chain(&mut old_profile.infobar.iter().flatten())
		{
			let _ = crate::events::outbound::will_appear::will_disappear_tree(instance, false).await;
		}
		let _ = crate::events::outbound::devices::clear_screen(device.clone()).await;
	}

	// We must use the mutable version of get_profile_store in order to create the store if it does not exist.
	let store = locks.profile_stores.get_profile_store_mut(&DEVICES.get(&device).unwrap(), &id).await?;
	let new_profile = &store.value;
	for instance in new_profile
		.keys
		.iter()
		.flatten()
		.chain(&mut new_profile.sliders.iter().flatten())
		.chain(&mut new_profile.infobar.iter().flatten())
	{
		let _ = crate::events::outbound::will_appear::will_appear_tree(instance).await;
	}
	store.save()?;

	locks.device_stores.set_selected_profile(&device, id.clone())?;

	let settings = crate::store::get_settings()?.value;
	if selected_profile != id && settings.show_profile_switch_pill {
		let infobar_segments = DEVICES.get(&device).map(|d| d.infobar).unwrap_or(0);
		if infobar_segments > 0 {
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
