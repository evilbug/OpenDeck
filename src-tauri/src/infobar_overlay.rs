use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dashmap::DashMap;

struct OverlayEntry {
	/// Unique ID assigned at push time so the expiry task can avoid removing a
	/// newer push at the same priority.
	id: u64,
	image: String,
	expires_at: Option<Instant>,
}

type OverlayMap = BTreeMap<Reverse<u8>, OverlayEntry>;

/// (device_id, position) → descending-priority BTreeMap of overlay entries.
static OVERLAYS: LazyLock<DashMap<(String, u8), OverlayMap>> = LazyLock::new(DashMap::new);

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Return the image from the highest-priority non-expired overlay for this
/// slot, or `None` if no active overlay exists.
pub fn get_active_overlay(device_id: &str, position: u8) -> Option<String> {
	let entries = OVERLAYS.get(&(device_id.to_owned(), position))?;
	let now = Instant::now();
	for entry in entries.values() {
		if entry.expires_at.is_none_or(|exp| exp > now) {
			return Some(entry.image.clone());
		}
	}
	None
}

/// Push an overlay image for a slot.  Returns an opaque ID that the caller
/// should pass to `expire_and_rerender` when the TTL fires.
///
/// `duration_ms == 0` means permanent (until explicitly cleared or the device
/// screen is reset).
pub fn push_overlay(device_id: &str, position: u8, image: String, priority: u8, duration_ms: u64) -> u64 {
	let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
	let expires_at = if duration_ms > 0 {
		Some(Instant::now() + std::time::Duration::from_millis(duration_ms))
	} else {
		None
	};
	OVERLAYS
		.entry((device_id.to_owned(), position))
		.or_default()
		.insert(Reverse(priority), OverlayEntry { id, image, expires_at });
	id
}

/// Remove a specific overlay entry, identified by its unique ID, and then
/// re-render the slot from the currently stored profile image.
pub async fn expire_and_rerender(device_id: String, position: u8, priority: u8, overlay_id: u64) {
	// Only remove this exact push; a newer push at the same priority replaces
	// this entry in the BTreeMap so the id will differ and we leave it alone.
	if let Some(mut entries) = OVERLAYS.get_mut(&(device_id.clone(), position)) {
		if entries.get(&Reverse(priority)).is_some_and(|e| e.id == overlay_id) {
			entries.remove(&Reverse(priority));
		} else {
			// A newer overlay replaced us; nothing to rerender.
			return;
		}
	}

	// Re-render the slot from the profile's stored image (or clear it).
	let (context, image) = {
		let mut locks = crate::store::profiles::acquire_locks_mut().await;
		let device = match crate::shared::DEVICES.get(&device_id) {
			Some(d) => d.clone(),
			None => return,
		};
		let selected = match locks.device_stores.get_selected_profile(&device_id) {
			Ok(p) => p,
			Err(_) => return,
		};
		let store = match locks.profile_stores.get_profile_store(&device, &selected) {
			Ok(s) => s,
			Err(_) => return,
		};
		// Only use the stored image if it is a real data URI; "actionDefaultImage"
		// or any other sentinel falls back to clearing the slot.
		let image = store
			.value
			.infobar
			.get(position as usize)
			.and_then(|slot| slot.as_ref())
			.and_then(|instance| instance.states.get(instance.current_state as usize))
			.map(|state| state.image.clone())
			.filter(|img| img.starts_with("data:"));
		let context = crate::shared::Context {
			device: device_id,
			profile: selected,
			controller: "Infobar".to_string(),
			position,
		};
		(context, image)
		// locks released here
	};

	let _ = crate::events::outbound::devices::update_image(context, image).await;
}

/// Remove all overlays for a device (called on screen clear / device
/// disconnect so stale overlays from a previous profile don't bleed through).
pub fn clear_device_overlays(device_id: &str) {
	OVERLAYS.retain(|(device, _), _| device.as_str() != device_id);
}

/// Remove all overlays at a specific position and trigger a rerender.
/// Called when a plugin explicitly dismisses its overlay early.
pub async fn clear_position_overlays(device_id: String, position: u8) {
	OVERLAYS.remove(&(device_id.clone(), position));

	// Re-render from the stored profile image (same logic as expire_and_rerender
	// but without per-entry ID checking since we clear everything at once).
	let (context, image) = {
		let mut locks = crate::store::profiles::acquire_locks_mut().await;
		let device = match crate::shared::DEVICES.get(&device_id) {
			Some(d) => d.clone(),
			None => return,
		};
		let selected = match locks.device_stores.get_selected_profile(&device_id) {
			Ok(p) => p,
			Err(_) => return,
		};
		let store = match locks.profile_stores.get_profile_store(&device, &selected) {
			Ok(s) => s,
			Err(_) => return,
		};
		let image = store
			.value
			.infobar
			.get(position as usize)
			.and_then(|slot| slot.as_ref())
			.and_then(|instance| instance.states.get(instance.current_state as usize))
			.map(|state| state.image.clone())
			.filter(|img| img.starts_with("data:"));
		let context = crate::shared::Context {
			device: device_id,
			profile: selected,
			controller: "Infobar".to_string(),
			position,
		};
		(context, image)
	};

	let _ = crate::events::outbound::devices::update_image(context, image).await;
}
