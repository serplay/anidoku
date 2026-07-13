<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		getEpisodes,
		listWatchStates,
		streamUrl,
		type WatchState
	} from '$lib/api';
	import { recallAnime } from '$lib/state.svelte';
	import EpisodeRow from '$lib/components/EpisodeRow.svelte';
	import VirtualList from '$lib/components/VirtualList.svelte';

	const id = $derived(decodeURIComponent(page.params.id ?? ''));
	const dub = $derived(page.url.searchParams.get('dub') === '1');
	const anime = $derived(recallAnime(id));

	let episodes = $state<string[]>([]);
	let progress = $state<Record<string, number>>({});
	let loading = $state(true);
	let error = $state<string | null>(null);

	const cover = $derived(
		anime?.cover_url ? streamUrl(anime.cover_url, 'https://youtu-chan.com') : null
	);

	$effect(() => {
		void load(id, dub);
	});

	async function load(showId: string, isDub: boolean) {
		loading = true;
		error = null;
		try {
			const [eps, states] = await Promise.all([
				getEpisodes(showId, isDub),
				listWatchStates(showId).catch(() => [] as WatchState[])
			]);
			episodes = eps;
			const p: Record<string, number> = {};
			for (const s of states) {
				if (s.duration_secs && s.duration_secs > 0) {
					p[s.episode_number] = s.position_secs / s.duration_secs;
				}
			}
			progress = p;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	function play(ep: string) {
		goto(`/watch/${encodeURIComponent(id)}/${encodeURIComponent(ep)}?dub=${dub ? 1 : 0}`);
	}
</script>

<a class="back" href="/">← Back to search</a>

<section class="hero">
	{#if cover}
		<img class="cover" src={cover} alt={anime?.title} />
	{/if}
	<div class="info">
		<h1>{anime?.title_english ?? anime?.title ?? id}</h1>
		{#if anime?.title_english && anime.title_english !== anime.title}
			<p class="alt">{anime.title}</p>
		{/if}
		<p class="meta">
			{episodes.length || anime?.available_episodes || 0} episodes · {dub ? 'Dub' : 'Sub'}
		</p>
	</div>
</section>

<h2>Episodes</h2>

{#if loading}
	<p class="status">Loading episodes…</p>
{:else if error}
	<p class="error">{error}</p>
{:else if episodes.length === 0}
	<p class="status">No episodes found.</p>
{:else if episodes.length > 60}
	<VirtualList items={episodes} itemHeight={45} height={560}>
		{#snippet row(ep)}
			<EpisodeRow number={ep} progress={progress[ep] ?? 0} onselect={() => play(ep)} />
		{/snippet}
	</VirtualList>
{:else}
	<div class="list">
		{#each episodes as ep (ep)}
			<EpisodeRow number={ep} progress={progress[ep] ?? 0} onselect={() => play(ep)} />
		{/each}
	</div>
{/if}

<style>
	.back {
		display: inline-block;
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		margin-bottom: var(--space-lg);
	}
	.hero {
		display: flex;
		gap: var(--space-lg);
		margin-bottom: var(--space-xl);
	}
	.cover {
		width: 180px;
		aspect-ratio: 2 / 3;
		object-fit: cover;
		border-radius: var(--radius-xl);
		background: var(--color-surface-card);
		flex-shrink: 0;
	}
	.info h1 {
		font: var(--text-display-sm);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-xs);
	}
	.alt {
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		margin: 0 0 var(--space-sm);
	}
	.meta {
		font: var(--text-num-sm);
		color: var(--color-muted);
	}
	h2 {
		font: var(--text-title-lg);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-md);
	}
	.list {
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-xl);
		overflow: hidden;
	}
	.status {
		color: var(--color-muted);
	}
	.error {
		color: var(--color-down);
	}
</style>
