<script lang="ts">
	import type { ActionInstance } from "$lib/ActionInstance";
	import type { Profile } from "$lib/Profile";

	import ArrowDown from "phosphor-svelte/lib/ArrowDown";
	import ArrowUp from "phosphor-svelte/lib/ArrowUp";
	import Trash from "phosphor-svelte/lib/Trash";
	import Key from "./Key.svelte";

	import { inspectedInstance, inspectedParentAction } from "$lib/propertyInspector";

	import { invoke } from "@tauri-apps/api/core";

	export let profile: Profile;

	function slotArray() {
		if (!$inspectedParentAction) return [];
		if ($inspectedParentAction.controller == "Encoder") return profile.sliders;
		if ($inspectedParentAction.controller == "Infobar") return profile.infobar;
		return profile.keys;
	}

	function parentInstance() {
		return $inspectedParentAction ? slotArray()[$inspectedParentAction.position] : null;
	}

	let children: ActionInstance[];
	$: children = parentInstance()?.children ?? [];
	let parentUuid: string;
	$: parentUuid = parentInstance()?.action.uuid ?? "";
	$: title = parentUuid == "opendeck.toggleaction" ? "Toggle Action" : parentUuid == "opendeck.infobarstack" ? "Infobar Stack" : "Multi Action";

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		if (event.dataTransfer?.types.includes("action")) event.dataTransfer.dropEffect = "copy";
	}

	async function handleDrop({ dataTransfer }: DragEvent) {
		if (dataTransfer?.getData("action")) {
			let action = JSON.parse(dataTransfer?.getData("action"));
			if (
				(parentUuid == "opendeck.multiaction" && !action.supported_in_multi_actions) ||
				(
					parentUuid == "opendeck.toggleaction" &&
					(action.uuid == "opendeck.multiaction" || action.uuid == "opendeck.toggleaction")
				) ||
				(parentUuid == "opendeck.infobarstack" && action.uuid == "opendeck.infobarstack")
			) {
				return;
			}
			let response: ActionInstance | null = await invoke("create_instance", { context: $inspectedParentAction, action });
			if (response && parentInstance()) {
				parentInstance()!.children = [...children, response];
				profile = profile;
			}
		}
	}

	async function removeInstance(index: number) {
		await invoke("remove_instance", { context: children[index].context });
		children.splice(index, 1);
		if (parentInstance()) {
			parentInstance()!.children = children;
			profile = profile;
		}
	}

	async function moveInstance(index: number, direction: -1 | 1) {
		if (!$inspectedParentAction) return;
		const destination = index + direction;
		if (destination < 0 || destination >= children.length) return;
		const response: ActionInstance | null = await invoke("reorder_child_instance", {
			context: $inspectedParentAction,
			from: index,
			to: destination,
		});
		if (response) {
			const slot = parentInstance();
			if (slot) {
				slot.children = response.children;
				profile = profile;
			}
			inspectedInstance.set(null);
		}
	}
</script>

<svelte:window
	on:keydown={(event) => {
		if (event.key == "Escape") $inspectedParentAction = null;
	}}
/>

<div class="px-6 pt-6 pb-4 text-neutral-300">
	<button class="float-right text-xl" on:click={() => $inspectedParentAction = null}>✕</button>
	<h1 class="font-semibold text-2xl">{title}</h1>
	{#if parentUuid == "opendeck.infobarstack"}
		<p class="mt-2 text-sm text-neutral-400">Top items have higher priority. The first visible child widget is rendered on the infobar.</p>
	{/if}
</div>

<!-- svelte-ignore a11y-no-static-element-interactions -->
<div
	class="flex flex-col h-128 overflow-auto"
	on:click={() => inspectedInstance.set(null)}
	on:keyup={() => inspectedInstance.set(null)}
>
	{#each children as instance, index}
		<div class="flex flex-row items-center mx-4 my-2 bg-neutral-700 hover:bg-neutral-600 transition-colors border border-neutral-600 rounded-lg">
			<Key inslot={instance} context={null} active={false} scale={3 / 4} />
			<p class="ml-4 text-xl text-neutral-300">{instance.action.name}</p>
			<div class="flex flex-col ml-auto mr-4 gap-1">
				<button on:click={() => moveInstance(index, -1)} disabled={index == 0}>
					<ArrowUp size="20" class="text-neutral-400 disabled:text-neutral-600" />
				</button>
				<button on:click={() => moveInstance(index, 1)} disabled={index == children.length - 1}>
					<ArrowDown size="20" class="text-neutral-400 disabled:text-neutral-600" />
				</button>
			</div>
			<button
				class="mr-10"
				on:click={() => removeInstance(index)}
			>
				<Trash size="32" class="text-neutral-400" />
			</button>
		</div>
	{/each}
	<div
		class="flex flex-row items-center mx-4 mt-2 mb-4 p-3 bg-neutral-700 hover:bg-neutral-600 transition-colors border border-dashed border-neutral-600 rounded-lg"
		on:dragover={handleDragOver}
		on:drop={handleDrop}
	>
		<img src="/cube.png" class="m-2 w-24 rounded-xl" alt="Add new action" />
		<p class="ml-4 text-xl text-neutral-400">Drop actions here</p>
	</div>
</div>
