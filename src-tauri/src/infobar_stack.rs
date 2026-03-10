use std::sync::LazyLock;

use dashmap::DashMap;

use crate::shared::{ActionContext, ActionInstance, ActionState, Context};
use crate::store::profiles::{LocksMut, get_slot_mut};

pub const ACTION_UUID: &str = "opendeck.infobarstack";
const BLANK_IMAGE_DATA_URL: &str = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==";

static VISIBILITY: LazyLock<DashMap<ActionContext, bool>> = LazyLock::new(DashMap::new);

pub fn is_infobar_stack(instance: &ActionInstance) -> bool {
	instance.action.uuid == ACTION_UUID
}

pub fn is_visible(context: &ActionContext) -> bool {
	VISIBILITY.get(context).map(|entry| *entry).unwrap_or(true)
}

pub fn set_visibility(context: ActionContext, visible: bool) {
	VISIBILITY.insert(context, visible);
}

pub fn clear_visibility(context: &ActionContext) {
	VISIBILITY.remove(context);
}

pub fn active_child(instance: &ActionInstance) -> Option<&ActionInstance> {
	instance.children.as_ref()?.iter().find(|child| is_visible(&child.context))
}

fn effective_state(instance: &ActionInstance) -> ActionState {
	active_child(instance)
		.and_then(|child| child.states.get(child.current_state as usize).cloned())
		.unwrap_or_else(|| ActionState {
			image: BLANK_IMAGE_DATA_URL.to_owned(),
			show: false,
			text: String::new(),
			..Default::default()
		})
}

pub async fn sync_parent_display(context: &Context, locks: &mut LocksMut<'_>) -> Result<Option<ActionContext>, anyhow::Error> {
	let Some(parent) = get_slot_mut(context, locks).await? else {
		return Ok(None);
	};
	if !is_infobar_stack(parent) {
		return Ok(None);
	}

	parent.states = vec![effective_state(parent)];
	parent.current_state = 0;
	Ok(Some(parent.context.clone()))
}
