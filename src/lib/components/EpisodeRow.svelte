<script lang="ts">
	interface Props {
		number: string;
		progress?: number; // 0..1 watched fraction
		active?: boolean;
		onselect?: () => void;
	}

	let { number, progress = 0, active = false, onselect }: Props = $props();
	const pct = $derived(Math.round(Math.min(1, Math.max(0, progress)) * 100));
</script>

<button class="row" class:active onclick={onselect}>
	<span class="num">Episode {number}</span>
	<span class="right">
		{#if pct >= 90}
			<span class="watched">Watched</span>
		{:else if pct > 0}
			<span class="resume">{pct}%</span>
		{/if}
		<svg viewBox="0 0 24 24" class="play" aria-hidden="true">
			<path d="M8 5v14l11-7z" fill="currentColor" />
		</svg>
	</span>
	{#if pct > 0 && pct < 90}
		<span class="bar" style="width:{pct}%"></span>
	{/if}
</button>

<style>
	.row {
		position: relative;
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: space-between;
		background: transparent;
		border: none;
		border-bottom: 1px solid var(--color-hairline);
		padding: 12px 16px;
		cursor: pointer;
		color: var(--color-body);
	}
	.row:hover {
		background: var(--color-surface-elevated);
	}
	.row.active {
		background: var(--color-surface-card);
	}
	.num {
		font: var(--text-num-sm);
	}
	.right {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
	}
	.watched {
		font: var(--text-caption);
		color: var(--color-up);
	}
	.resume {
		font: var(--text-caption);
		color: var(--color-primary);
	}
	.play {
		width: 18px;
		height: 18px;
		color: var(--color-muted);
	}
	.row:hover .play {
		color: var(--color-primary);
	}
	.bar {
		position: absolute;
		left: 0;
		bottom: 0;
		height: 2px;
		background: var(--color-primary);
	}
</style>
