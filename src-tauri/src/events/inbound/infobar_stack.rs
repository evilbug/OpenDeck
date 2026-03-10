use super::ContextAndPayloadEvent;

use crate::events::frontend::instances::update_state;
use crate::store::profiles::acquire_locks_mut;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct SetInfobarItemVisibilityPayload {
	pub visible: bool,
}

pub async fn set_infobar_item_visibility(event: ContextAndPayloadEvent<SetInfobarItemVisibilityPayload>) -> Result<(), anyhow::Error> {
	crate::infobar_stack::set_visibility(event.context.clone(), event.payload.visible);

	let mut locks = acquire_locks_mut().await;
	let parent_context = crate::infobar_stack::sync_parent_display(&(&event.context).into(), &mut locks).await?;
	if let Some(parent_context) = parent_context {
		update_state(crate::APP_HANDLE.get().unwrap(), parent_context, &mut locks).await?;
	}
	Ok(())
}
