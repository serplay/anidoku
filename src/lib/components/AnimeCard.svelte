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
		<span class="title" title={anime.title}>{anime.title_english ?? anime.title}</span>
	</div>
</button>

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		background: transparent;
		border: none;
		padding: 0;
		text-align: left;
		cursor: pointer;
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
