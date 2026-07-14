<script lang="ts">
	import { STATUS_ORDER, STATUS_LABEL, type MediaListStatus } from '$lib/api';

	interface Props {
		status: MediaListStatus;
		progress: number;
		episodeCount?: number | null;
		compact?: boolean;
		disabled?: boolean;
		onchange: (status: MediaListStatus, progress: number) => void;
	}

	let {
		status,
		progress,
		episodeCount = null,
		compact = false,
		disabled = false,
		onchange
	}: Props = $props();

	const max = $derived(episodeCount && episodeCount > 0 ? episodeCount : null);

	function setStatus(e: Event) {
		const s = (e.target as HTMLSelectElement).value as MediaListStatus;
		onchange(s, progress);
	}

	function bump(delta: number) {
		let next = Math.max(0, progress + delta);
		if (max !== null) next = Math.min(next, max);
		if (next !== progress) onchange(status, next);
	}
</script>

<div class="controls" class:compact>
	<select value={status} onchange={setStatus} {disabled} aria-label="List status">
		{#each STATUS_ORDER as s (s)}
			<option value={s}>{STATUS_LABEL[s]}</option>
		{/each}
	</select>

	<div class="stepper" role="group" aria-label="Progress">
		<button type="button" onclick={() => bump(-1)} disabled={disabled || progress <= 0}>−</button>
		<span class="count">{progress}{#if max !== null}<span class="of">/{max}</span>{/if}</span>
		<button
			type="button"
			onclick={() => bump(1)}
			disabled={disabled || (max !== null && progress >= max)}>+</button
		>
	</div>
</div>

<style>
	.controls {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
	}
	select {
		background: var(--color-surface-elevated);
		color: var(--color-on-dark);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: 6px 10px;
		height: 34px;
		font: var(--text-body-sm);
		cursor: pointer;
	}
	select:focus-visible {
		border-color: var(--color-primary);
	}
	.stepper {
		display: flex;
		align-items: center;
		gap: 2px;
		background: var(--color-surface-elevated);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		height: 34px;
		padding: 0 4px;
	}
	.stepper button {
		width: 26px;
		height: 26px;
		border: none;
		background: transparent;
		color: var(--color-body);
		font-size: 18px;
		line-height: 1;
		cursor: pointer;
		border-radius: var(--radius-sm);
	}
	.stepper button:hover:not(:disabled) {
		background: var(--color-surface-card);
		color: var(--color-primary);
	}
	.stepper button:disabled {
		color: var(--color-muted);
		cursor: not-allowed;
	}
	.count {
		font: var(--text-num-sm);
		color: var(--color-on-dark);
		min-width: 42px;
		text-align: center;
	}
	.of {
		color: var(--color-muted);
	}
	.compact select {
		height: 30px;
		padding: 4px 8px;
	}
	.compact .stepper {
		height: 30px;
	}
</style>
