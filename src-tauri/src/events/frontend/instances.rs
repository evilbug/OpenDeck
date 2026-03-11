use super::Error;

use crate::shared::{Action, ActionContext, ActionInstance, Context, config_dir};
use crate::store::profiles::{LocksMut, acquire_locks_mut, get_instance_mut, get_slot_mut, save_profile};

use tauri::{AppHandle, Emitter, Manager, command};
use tokio::fs::remove_dir_all;

#[command]
pub async fn create_instance(app: AppHandle, action: Action, context: Context) -> Result<Option<ActionInstance>, Error> {
	if !action.controllers.contains(&context.controller) {
		return Ok(None);
	}

	let mut locks = acquire_locks_mut().await;
	let slot = get_slot_mut(&context, &mut locks).await?;

	if let Some(parent) = slot {
		let (instance, parent_context, should_update_parent) = {
			if parent.children.is_none() && matches!(parent.action.uuid.as_str(), "opendeck.multiaction" | "opendeck.toggleaction" | crate::infobar_stack::ACTION_UUID) {
				parent.children = Some(vec![]);
			}

			let Some(children) = &mut parent.children else { return Ok(None) };
			let index = match children.last() {
				None => 1,
				Some(instance) => instance.context.index + 1,
			};

			let instance = ActionInstance {
				action: action.clone(),
				context: ActionContext::from_context(context.clone(), index),
				states: action.states.clone(),
				current_state: 0,
				settings: serde_json::Value::Object(serde_json::Map::new()),
				children: None,
			};
			children.push(instance.clone());
			renumber_children(&context, children);

			let should_update_parent = parent.action.uuid == "opendeck.toggleaction" && parent.states.len() < children.len();
			if should_update_parent {
				parent.states.push(crate::shared::ActionState {
					image: "opendeck/toggle-action.png".to_owned(),
					..Default::default()
				});
			}

			(instance, parent.context.clone(), should_update_parent)
		};

		if should_update_parent {
			let _ = update_state(&app, parent_context, &mut locks).await;
		}
		let _ = sync_infobar_stack_and_emit(&app, context.clone(), &mut locks).await;

		save_profile(&context.device, &mut locks).await?;
		let _ = crate::events::outbound::will_appear::will_appear(&instance).await;

		Ok(Some(instance))
	} else {
		let instance = ActionInstance {
			action: action.clone(),
			context: ActionContext::from_context(context.clone(), 0),
			states: action.states.clone(),
			current_state: 0,
			settings: serde_json::Value::Object(serde_json::Map::new()),
			children: if matches!(action.uuid.as_str(), "opendeck.multiaction" | "opendeck.toggleaction" | crate::infobar_stack::ACTION_UUID) {
				Some(vec![])
			} else {
				None
			},
		};

		let slot = {
			*slot = Some(instance.clone());
			slot.clone()
		};
		let _ = sync_infobar_stack_and_emit(&app, context.clone(), &mut locks).await;

		save_profile(&context.device, &mut locks).await?;
		let _ = crate::events::outbound::will_appear::will_appear(&instance).await;

		Ok(slot)
	}
}

fn instance_images_dir(context: &ActionContext) -> std::path::PathBuf {
	config_dir()
		.join("images")
		.join(&context.device)
		.join(&context.profile)
		.join(format!("{}.{}.{}", context.controller, context.position, context.index))
}

async fn sync_infobar_stack_and_emit(app: &AppHandle, context: Context, locks: &mut LocksMut<'_>) -> Result<(), anyhow::Error> {
	if let Some(parent_context) = crate::infobar_stack::sync_parent_display(&context, locks).await? {
		update_state(app, parent_context, locks).await?;
	}
	Ok(())
}

fn renumber_children(parent_context: &Context, children: &mut [ActionInstance]) {
	for (index, child) in children.iter_mut().enumerate() {
		child.context = ActionContext::from_context(parent_context.clone(), index as u16 + 1);
	}
}

async fn refresh_children_lifecycle(old_children: &[ActionInstance], new_children: &[ActionInstance]) {
	for child in old_children {
		let _ = crate::events::outbound::will_appear::will_disappear(child, false).await;
	}
	for child in new_children {
		let _ = crate::events::outbound::will_appear::will_appear(child).await;
	}
}

#[command]
pub async fn move_instance(source: Context, destination: Context, retain: bool) -> Result<Option<ActionInstance>, Error> {
	if source.controller != destination.controller {
		return Ok(None);
	}

	{
		let locks = crate::store::profiles::acquire_locks().await;
		let dst = crate::store::profiles::get_slot(&destination, &locks).await?;
		if dst.is_some() {
			return Ok(None);
		}
	}

	let mut locks = acquire_locks_mut().await;
	let src = get_slot_mut(&source, &mut locks).await?;

	let Some(mut new) = src.clone() else {
		return Ok(None);
	};
	new.context = ActionContext::from_context(destination.clone(), 0);
	if let Some(children) = &mut new.children {
		for (index, instance) in children.iter_mut().enumerate() {
			instance.context = ActionContext::from_context(destination.clone(), index as u16 + 1);
			for (i, state) in instance.states.iter_mut().enumerate() {
				if !instance.action.states[i].image.is_empty() {
					state.image = instance.action.states[i].image.clone();
				} else {
					state.image = instance.action.icon.clone();
				}
			}
		}
	}

	let old_dir = instance_images_dir(&src.as_ref().unwrap().context);
	let new_dir = instance_images_dir(&new.context);
	let _ = tokio::fs::create_dir_all(&new_dir).await;
	if let Ok(files) = old_dir.read_dir() {
		for file in files.flatten() {
			let _ = tokio::fs::copy(file.path(), new_dir.join(file.file_name())).await;
		}
	}
	for state in new.states.iter_mut() {
		let path = std::path::Path::new(&state.image);
		if path.starts_with(&old_dir) {
			state.image = new_dir.join(path.strip_prefix(&old_dir).unwrap()).to_string_lossy().into_owned();
		}
	}

	let dst = get_slot_mut(&destination, &mut locks).await?;
	*dst = Some(new.clone());
	let _ = sync_infobar_stack_and_emit(crate::APP_HANDLE.get().unwrap(), destination.clone(), &mut locks).await;

	if !retain {
		let src = get_slot_mut(&source, &mut locks).await?;
		if let Some(old) = src {
			let _ = crate::events::outbound::will_appear::will_disappear(old, true).await;
			let _ = remove_dir_all(instance_images_dir(&old.context)).await;
		}
		*src = None;
	}

	let _ = crate::events::outbound::will_appear::will_appear(&new).await;

	save_profile(&destination.device, &mut locks).await?;

	Ok(Some(new))
}

#[command]
pub async fn reorder_child_instance(context: Context, from: usize, to: usize) -> Result<Option<ActionInstance>, Error> {
	let mut locks = acquire_locks_mut().await;
	let (parent, old_children, new_children) = {
		let slot = get_slot_mut(&context, &mut locks).await?;
		let Some(parent) = slot else {
			return Ok(None);
		};
		let (old_children, new_children) = {
			let Some(children) = &mut parent.children else {
				return Ok(None);
			};
			if from >= children.len() || to >= children.len() {
				return Ok(None);
			}
			let old_children = children.clone();

			let child = children.remove(from);
			children.insert(to, child);
			renumber_children(&context, children);
			(old_children, children.clone())
		};
		(parent.clone(), old_children, new_children)
	};
	refresh_children_lifecycle(&old_children, &new_children).await;
	let _ = sync_infobar_stack_and_emit(crate::APP_HANDLE.get().unwrap(), context.clone(), &mut locks).await;

	save_profile(&context.device, &mut locks).await?;
	Ok(Some(parent))
}

#[command]
pub async fn remove_instance(context: ActionContext) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	let slot = get_slot_mut(&(&context).into(), &mut locks).await?;
	let Some(instance) = slot else {
		return Ok(());
	};

	if instance.context == context {
		let _ = crate::events::outbound::will_appear::will_disappear_tree(instance, true).await;
		if let Some(children) = &instance.children {
			for child in children {
				let _ = remove_dir_all(instance_images_dir(&child.context)).await;
			}
		}
		let _ = remove_dir_all(instance_images_dir(&instance.context)).await;
		*slot = None;
	} else {
		let children = instance.children.as_mut().unwrap();
		let old_children = children.clone();
		let mut removed_index = None;
		for (index, instance) in children.iter().enumerate() {
			if instance.context == context {
				let _ = crate::events::outbound::will_appear::will_disappear(instance, true).await;
				let _ = remove_dir_all(instance_images_dir(&instance.context)).await;
				children.remove(index);
				removed_index = Some(index);
				renumber_children(&(&context).into(), children);
				break;
			}
		}
		if let Some(removed_index) = removed_index {
			let shifted_old = &old_children[(removed_index + 1).min(old_children.len())..];
			let shifted_new = &children[removed_index.min(children.len())..];
			for (old_child, new_child) in shifted_old.iter().zip(shifted_new.iter()) {
				if old_child.context != new_child.context {
					let _ = crate::events::outbound::will_appear::will_disappear(old_child, false).await;
					let _ = crate::events::outbound::will_appear::will_appear(new_child).await;
				}
			}
		}
		if instance.action.uuid == "opendeck.toggleaction" {
			if instance.current_state as usize >= children.len() {
				instance.current_state = if children.is_empty() { 0 } else { children.len() as u16 - 1 };
			}
			if !children.is_empty() {
				instance.states.pop();
				let _ = update_state(crate::APP_HANDLE.get().unwrap(), instance.context.clone(), &mut locks).await;
			}
		}
		let _ = sync_infobar_stack_and_emit(crate::APP_HANDLE.get().unwrap(), (&context).into(), &mut locks).await;
	}

	save_profile(&context.device, &mut locks).await?;

	Ok(())
}

#[derive(Clone, serde::Serialize)]
struct UpdateStateEvent {
	context: ActionContext,
	contents: Option<ActionInstance>,
}

pub async fn update_state(app: &AppHandle, context: ActionContext, locks: &mut LocksMut<'_>) -> Result<(), anyhow::Error> {
	let window = app.get_webview_window("main").unwrap();
	window.emit(
		"update_state",
		UpdateStateEvent {
			contents: get_instance_mut(&context, locks).await?.cloned(),
			context: context.clone(),
		},
	)?;

	if context.index != 0 {
		let slot_context: Context = (&context).into();
		if let Some(parent_context) = crate::infobar_stack::sync_parent_display(&slot_context, locks).await? {
			window.emit(
				"update_state",
				UpdateStateEvent {
					contents: get_instance_mut(&parent_context, locks).await?.cloned(),
					context: parent_context,
				},
			)?;
		}
	}
	Ok(())
}

#[command]
pub async fn set_state(instance: ActionInstance, state: u16) -> Result<(), Error> {
	let mut locks = acquire_locks_mut().await;
	let reference = get_instance_mut(&instance.context, &mut locks).await?.unwrap();
	*reference = instance.clone();
	update_state(crate::APP_HANDLE.get().unwrap(), instance.context.clone(), &mut locks).await?;
	save_profile(&instance.context.device, &mut locks).await?;
	crate::events::outbound::states::title_parameters_did_change(&instance, state).await?;
	Ok(())
}

#[command]
pub async fn update_image(context: Context, image: String) {
	if Some(&context.profile) != crate::store::profiles::DEVICE_STORES.write().await.get_selected_profile(&context.device).ok().as_ref() {
		return;
	}

	if let Err(error) = crate::events::outbound::devices::update_image(context, Some(image)).await {
		log::warn!("Failed to update device image: {}", error);
	}
}

#[derive(Clone, serde::Serialize)]
struct KeyMovedEvent {
	context: Context,
	pressed: bool,
}

pub async fn key_moved(app: &AppHandle, context: Context, pressed: bool) -> Result<(), anyhow::Error> {
	let window = app.get_webview_window("main").unwrap();
	window.emit("key_moved", KeyMovedEvent { context, pressed })?;
	Ok(())
}
