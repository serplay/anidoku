<script lang="ts">
	import type { AnimeSummary, CardMeta } from '$lib/api';
	import { streamUrl, formatLabel } from '$lib/api';
	import coverPlaceholder from '$lib/assets/cover-placeholder.svg';

	interface Props {
		anime: AnimeSummary;
		onselect?: (a: AnimeSummary) => void;
		/** Extra caption badge, e.g. "Ep 5 in 2d" on home airing cards. */
		caption?: string | null;
		/** Route the cover through the referer proxy (allanime thumbnails).
		 *  Home rows use direct AniList covers and pass false. */
		proxyCover?: boolean;
		/** Hide the "N ep" badge (home rows where the count is unknown/0). */
		showEpisodes?: boolean;
		/** Meta chips (format / year / episodes / score) under the title. Only
		 *  chips with data render; omit entirely for provider-only cards. */
		meta?: CardMeta | null;
	}

	let {
		anime,
		onselect,
		caption = null,
		proxyCover = true,
		showEpisodes = true,
		meta = null
	}: Props = $props();

	// The episodes chip: "aired/total" while releasing, else "N ep".
	const episodesChip = $derived.by(() => {
		if (!meta) return null;
		const { episodes, aired, releasing } = meta;
		if (releasing && aired && aired > 0) {
			return episodes ? `${aired}/${episodes}` : `${aired} ep`;
		}
		if (episodes && episodes > 0) return `${episodes} ep`;
		return null;
	});
	// A high score gets a subtle green tint (trading-up), per DESIGN.md.
	const scoreHigh = $derived(!!meta?.score && meta.score >= 75);

	// allanime thumbnails are referer-gated; route them through the proxy so
	// they load inside the webview. AniList covers (home rows) load directly.
	const cover = $derived(
		anime.cover_url
			? proxyCover
				? streamUrl(anime.cover_url, 'https://youtu-chan.com')
				: anime.cover_url
			: null
	);
	const display = $derived(anime.title_english ?? anime.title);
</script>

<button class="card" onclick={() => onselect?.(anime)}>
	<div class="cover">
		<img
			src={cover ?? coverPlaceholder}
			alt={anime.title}
			loading="lazy"
			onerror={(e) => ((e.currentTarget as HTMLImageElement).src = coverPlaceholder)}
		/>
		{#if caption}
			<span class="badge caption">{caption}</span>
		{:else if showEpisodes && anime.available_episodes > 0}
			<span class="badge">{anime.available_episodes} ep</span>
		{/if}
		<!-- Full-title tooltip: titles clamp to 2 lines, so long ones cut off.
		     Anchored inside the cover (position:relative; overflow:hidden) so it
		     can never escape the card and be clipped by a row's scroll container. -->
		<div class="tooltip" role="tooltip">
			<span class="t-main">{display}</span>
			{#if anime.title !== display}
				<span class="t-sub">{anime.title}</span>
			{/if}
		</div>
	</div>
	<div class="meta">
		<span class="title">{display}</span>
		{#if meta}
			<div class="chips">
				{#if formatLabel(meta.format)}
					<span class="chip">{formatLabel(meta.format)}</span>
				{/if}
				{#if meta.year}
					<span class="chip">{meta.year}</span>
				{/if}
				{#if episodesChip}
					<span class="chip">{episodesChip}</span>
				{/if}
				{#if meta.score}
					<span class="chip score" class:high={scoreHigh}>★ {meta.score}</span>
				{/if}
			</div>
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
		left: 6px;
		right: 6px;
		bottom: 6px;
		z-index: 20;
		display: flex;
		flex-direction: column;
		gap: 2px;
		background: rgba(11, 14, 17, 0.92);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: var(--space-xs) var(--space-sm);
		opacity: 0;
		pointer-events: none;
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
	.badge.caption {
		color: var(--color-body);
	}
	.meta {
		padding: 0 2px;
		display: flex;
		flex-direction: column;
		gap: var(--space-xxs);
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
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-xxs);
	}
	.chip {
		background: var(--color-surface-card);
		color: var(--color-muted-strong);
		font: var(--text-caption);
		padding: 1px 6px;
		border-radius: var(--radius-sm);
		white-space: nowrap;
	}
	/* High scores get a subtle green tint; no yellow (reserved for CTAs). */
	.chip.score.high {
		color: var(--color-up);
	}
</style>
