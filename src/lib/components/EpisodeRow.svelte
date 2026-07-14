<script lang="ts">
	import type { DownloadState } from '$lib/api';

	interface Props {
		number: string;
		progress?: number; // 0..1 watched fraction
		active?: boolean;
		onselect?: () => void;
		/** Download affordance: state + 0..1 fraction, or null to hide. */
		download?: { state: DownloadState; fraction: number } | null;
		ondownload?: () => void;
	}

	let {
		number,
		progress = 0,
		active = false,
		onselect,
		download = null,
		ondownload
	}: Props = $props();
	const pct = $derived(Math.round(Math.min(1, Math.max(0, progress)) * 100));
	const dlPct = $derived(
		download ? Math.round(Math.min(1, Math.max(0, download.fraction)) * 100) : 0
	);

	const dlTitle = $derived.by(() => {
		if (!download) return 'Download episode';
		switch (download.state) {
			case 'queued':
				return 'Queued — click to pause';
			case 'downloading':
				return `Downloading ${dlPct}% — click to pause`;
			case 'paused':
				return 'Paused — click to resume';
			case 'done':
				return 'Downloaded — available offline';
			case 'failed':
				return 'Failed — click to retry';
		}
	});
</script>

<div class="row" class:active>
	<button class="main" onclick={onselect}>
		<span class="num">Episode {number}</span>
		<span class="right">
			{#if download?.state === 'done'}
				<span class="offline">Offline</span>
			{/if}
			{#if pct >= 90}
				<span class="watched">Watched</span>
			{:else if pct > 0}
				<span class="resume">{pct}%</span>
			{/if}
			<svg viewBox="0 0 24 24" class="play" aria-hidden="true">
				<path d="M8 5v14l11-7z" fill="currentColor" />
			</svg>
		</span>
	</button>
	{#if ondownload}
		<button
			class="dl state-{download?.state ?? 'none'}"
			title={dlTitle}
			aria-label={dlTitle}
			onclick={(e) => {
				e.stopPropagation();
				ondownload();
			}}
			disabled={download?.state === 'done'}
		>
			{#if !download || download.state === 'failed'}
				<!-- download arrow (retry shares it, tinted) -->
				<svg viewBox="0 0 24 24" aria-hidden="true">
					<path
						d="M12 3v10m0 0l-4-4m4 4l4-4M5 19h14"
						stroke="currentColor"
						stroke-width="2"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
				</svg>
			{:else if download.state === 'queued'}
				<!-- clock -->
				<svg viewBox="0 0 24 24" aria-hidden="true">
					<circle cx="12" cy="12" r="8" stroke="currentColor" stroke-width="2" fill="none" />
					<path d="M12 8v4l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" />
				</svg>
			{:else if download.state === 'downloading'}
				<span class="pct">{dlPct}%</span>
			{:else if download.state === 'paused'}
				<!-- resume/play -->
				<svg viewBox="0 0 24 24" aria-hidden="true">
					<path d="M8 5v14l11-7z" fill="currentColor" />
				</svg>
			{:else if download.state === 'done'}
				<!-- check -->
				<svg viewBox="0 0 24 24" aria-hidden="true">
					<path
						d="M5 13l4 4L19 7"
						stroke="currentColor"
						stroke-width="2.5"
						fill="none"
						stroke-linecap="round"
						stroke-linejoin="round"
					/>
				</svg>
			{/if}
		</button>
	{/if}
	{#if pct > 0 && pct < 90}
		<span class="bar" style="width:{pct}%"></span>
	{/if}
</div>

<style>
	.row {
		position: relative;
		width: 100%;
		display: flex;
		align-items: stretch;
		border-bottom: 1px solid var(--color-hairline);
	}
	.row:hover {
		background: var(--color-surface-elevated);
	}
	.row.active {
		background: var(--color-surface-card);
	}
	.main {
		flex: 1;
		display: flex;
		align-items: center;
		justify-content: space-between;
		background: transparent;
		border: none;
		padding: 12px 16px;
		cursor: pointer;
		color: var(--color-body);
		min-width: 0;
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
	.offline {
		font: var(--text-caption);
		color: var(--color-up);
		border: 1px solid var(--color-up);
		border-radius: var(--radius-sm);
		padding: 0 5px;
		line-height: 16px;
	}
	.play {
		width: 18px;
		height: 18px;
		color: var(--color-muted);
	}
	.main:hover .play {
		color: var(--color-primary);
	}
	.dl {
		width: 44px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: transparent;
		border: none;
		border-left: 1px solid var(--color-hairline);
		color: var(--color-muted);
		cursor: pointer;
		padding: 0;
	}
	.dl svg {
		width: 17px;
		height: 17px;
	}
	.dl:hover:not(:disabled) {
		color: var(--color-on-dark);
		background: var(--color-surface-card);
	}
	.dl.state-downloading .pct,
	.dl.state-queued,
	.dl.state-paused {
		color: var(--color-primary);
	}
	.pct {
		font: var(--text-caption);
	}
	.dl.state-done {
		color: var(--color-up);
		cursor: default;
	}
	.dl.state-failed {
		color: var(--color-down);
	}
	.bar {
		position: absolute;
		left: 0;
		bottom: 0;
		height: 2px;
		background: var(--color-primary);
		pointer-events: none;
	}
</style>
