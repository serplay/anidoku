<script lang="ts">
	import type { AnimeSummary } from '$lib/api';
	import { streamUrl } from '$lib/api';

	interface Props {
		anime: AnimeSummary;
		onselect?: (a: AnimeSummary) => void;
	}

	let { anime, onselect }: Props = $props();

	// allanime thumbnails are also referer-gated; route them through the proxy
	// so they load inside the webview.
	const cover = $derived(
		anime.cover_url ? streamUrl(anime.cover_url, 'https://youtu-chan.com') : null
	);
	const display = $derived(anime.title_english ?? anime.title);
</script>

<button class="card" onclick={() => onselect?.(anime)}>
	<div class="cover">
		{#if cover}
			<img src={cover} alt={anime.title} loading="lazy" />
		{:else}
			<div class="placeholder">{anime.title.slice(0, 1)}</div>
		{/if}
		<span class="badge">{anime.available_episodes} ep</span>
	</div>
	<div class="meta">
		<span class="title">{display}</span>
	</div>
	<!-- Full-title tooltip: card titles clamp to 2 lines, so long ones cut off. -->
	<div class="tooltip" role="tooltip">
		<span class="t-main">{display}</span>
		{#if anime.title !== display}
			<span class="t-sub">{anime.title}</span>
		{/if}
	</div>
</button>

<style>
	.card {
		position: relative;
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		background: transparent;
		border: none;
		padding: 0;
		text-align: left;
		cursor: pointer;
	}
	.tooltip {
		position: absolute;
		top: 100%;
		left: 0;
		z-index: 20;
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 100%;
		width: max-content;
		max-width: 260px;
		background: var(--color-surface-elevated);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: var(--space-xs) var(--space-sm);
		opacity: 0;
		pointer-events: none;
		transform: translateY(2px);
		transition: opacity 0.15s ease 0.3s;
	}
	.card:hover .tooltip,
	.card:focus-visible .tooltip {
		opacity: 1;
	}
	.t-main {
		font: var(--text-title-sm);
		color: var(--color-on-dark);
	}
	.t-sub {
		font: var(--text-body-sm);
		color: var(--color-muted-strong);
	}
	.cover {
		position: relative;
		aspect-ratio: 2 / 3;
		border-radius: var(--radius-lg);
		overflow: hidden;
		background: var(--color-surface-card);
	}
	.cover img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
		transition: transform 0.18s ease;
	}
	.card:hover .cover img {
		transform: scale(1.04);
	}
	.placeholder {
		width: 100%;
		height: 100%;
		display: grid;
		place-items: center;
		font: var(--text-display-md);
		color: var(--color-muted);
	}
	.badge {
		position: absolute;
		bottom: var(--space-xs);
		right: var(--space-xs);
		background: rgba(11, 14, 17, 0.85);
		color: var(--color-primary);
		font: var(--text-caption);
		padding: 2px 8px;
		border-radius: var(--radius-sm);
	}
	.meta {
		padding: 0 2px;
	}
	.title {
		font: var(--text-title-sm);
		color: var(--color-body);
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
</style>
