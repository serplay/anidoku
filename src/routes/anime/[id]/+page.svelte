<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		getEpisodes,
		listWatchStates,
		streamUrl,
		getAnimeListState,
		setListEntry,
		searchAnilist,
		setAnimeMapping,
		isDesktop,
		type WatchState,
		type ListEntry,
		type MediaListStatus,
		type MediaInfo
	} from '$lib/api';
	import { recallAnime, authState, pushToast } from '$lib/state.svelte';
	import EpisodeRow from '$lib/components/EpisodeRow.svelte';
	import VirtualList from '$lib/components/VirtualList.svelte';
	import ListControls from '$lib/components/ListControls.svelte';

	const id = $derived(decodeURIComponent(page.params.id ?? ''));
	const dub = $derived(page.url.searchParams.get('dub') === '1');
	const anime = $derived(recallAnime(id));

	let episodes = $state<string[]>([]);
	let progress = $state<Record<string, number>>({});
	let loading = $state(true);
	let error = $state<string | null>(null);

	// AniList list state for this show.
	let anilistId = $state<number | null>(null);
	let entry = $state<ListEntry | null>(null);
	let episodeCount = $state<number | null>(null);
	let matching = $state(false);

	// "Wrong match?" search UI.
	let showRematch = $state(false);
	let rematchQuery = $state('');
	let rematchResults = $state<MediaInfo[]>([]);
	let rematchBusy = $state(false);

	const cover = $derived(
		anime?.cover_url ? streamUrl(anime.cover_url, 'https://youtu-chan.com') : null
	);

	$effect(() => {
		void load(id, dub);
	});

	// Resolve the AniList mapping/list-state whenever the show changes.
	$effect(() => {
		void resolveListState(id);
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

	async function resolveListState(showId: string) {
		if (!isDesktop()) return;
		matching = true;
		try {
			const title = anime?.title ?? showId;
			const eps = anime?.available_episodes || null;
			const hint = anime?.anilist_id ?? null;
			const s = await getAnimeListState(showId, title, eps, hint);
			anilistId = s.anilist_id;
			entry = s.entry;
			episodeCount = s.episode_count;
		} catch {
			/* matching is best-effort; leave unmapped */
		} finally {
			matching = false;
		}
	}

	async function changeEntry(status: MediaListStatus, prog: number) {
		if (anilistId === null) return;
		try {
			entry = await setListEntry(anilistId, status, prog, entry?.score ?? null);
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	async function addToList() {
		if (anilistId === null) return;
		await changeEntry('CURRENT', entry?.progress ?? 0);
	}

	async function runRematch() {
		rematchBusy = true;
		try {
			rematchResults = await searchAnilist(rematchQuery || anime?.title || id);
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			rematchBusy = false;
		}
	}

	async function pickMatch(m: MediaInfo) {
		try {
			await setAnimeMapping(id, m.anilist_id);
			showRematch = false;
			rematchResults = [];
			await resolveListState(id);
			pushToast('Match updated.', 'sync');
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	function openRematch() {
		showRematch = true;
		rematchQuery = anime?.title ?? '';
		if (rematchResults.length === 0) void runRematch();
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

		{#if isDesktop()}
			<div class="anilist">
				{#if anilistId !== null}
					{#if entry}
						<ListControls
							status={entry.status}
							progress={entry.progress}
							{episodeCount}
							onchange={changeEntry}
						/>
					{:else}
						<button class="add" onclick={addToList}>+ Add to list</button>
					{/if}
					<button class="rematch" onclick={openRematch}>Wrong match?</button>
				{:else if matching}
					<span class="mstatus">Matching to AniList…</span>
				{:else}
					<span class="mstatus">Not matched to AniList.</span>
					<button class="rematch" onclick={openRematch}>Search AniList</button>
				{/if}
				{#if !authState.logged_in && (entry || anilistId !== null)}
					<span class="local-note">local only — <a href="/settings">sign in</a> to sync</span>
				{/if}
			</div>

			{#if showRematch}
				<div class="rematch-panel">
					<div class="rematch-search">
						<input
							type="text"
							bind:value={rematchQuery}
							placeholder="Search AniList…"
							onkeydown={(e) => e.key === 'Enter' && runRematch()}
						/>
						<button onclick={runRematch} disabled={rematchBusy}>
							{rematchBusy ? '…' : 'Search'}
						</button>
						<button class="close" onclick={() => (showRematch = false)}>✕</button>
					</div>
					{#if rematchResults.length > 0}
						<ul class="matches">
							{#each rematchResults as m (m.anilist_id)}
								<li>
									<button onclick={() => pickMatch(m)}>
										{#if m.cover_url}<img src={m.cover_url} alt="" />{/if}
										<span class="mt">
											<span class="mtitle">{m.title_english ?? m.title_romaji}</span>
											<span class="msub"
												>{m.title_romaji}{#if m.episode_count} · {m.episode_count} ep{/if}</span
											>
										</span>
									</button>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			{/if}
		{/if}
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
	.anilist {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--space-sm);
		margin-top: var(--space-md);
	}
	.add {
		background: var(--color-primary);
		color: var(--color-on-primary);
		border: none;
		border-radius: var(--radius-md);
		padding: 8px 16px;
		height: 34px;
		font: var(--text-button);
		cursor: pointer;
	}
	.add:hover {
		background: var(--color-primary-active);
	}
	.rematch {
		background: none;
		border: none;
		color: var(--color-muted-strong);
		font: var(--text-body-sm);
		text-decoration: underline;
		cursor: pointer;
	}
	.rematch:hover {
		color: var(--color-on-dark);
	}
	.mstatus {
		font: var(--text-body-sm);
		color: var(--color-muted);
	}
	.local-note {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.rematch-panel {
		margin-top: var(--space-md);
		background: var(--color-surface-card);
		border-radius: var(--radius-lg);
		padding: var(--space-md);
		max-width: 520px;
	}
	.rematch-search {
		display: flex;
		gap: var(--space-xs);
	}
	.rematch-search input {
		flex: 1;
		background: var(--color-canvas);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		color: var(--color-on-dark);
		padding: 8px 12px;
		height: 36px;
		font: var(--text-body-md);
	}
	.rematch-search button {
		background: var(--color-surface-elevated);
		color: var(--color-on-dark);
		border: none;
		border-radius: var(--radius-md);
		padding: 0 14px;
		height: 36px;
		cursor: pointer;
		font: var(--text-button);
	}
	.rematch-search .close {
		padding: 0 12px;
	}
	.matches {
		list-style: none;
		margin: var(--space-sm) 0 0;
		padding: 0;
		max-height: 320px;
		overflow-y: auto;
	}
	.matches li button {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		width: 100%;
		background: none;
		border: none;
		border-radius: var(--radius-md);
		padding: var(--space-xs);
		cursor: pointer;
		text-align: left;
	}
	.matches li button:hover {
		background: var(--color-surface-elevated);
	}
	.matches img {
		width: 34px;
		height: 48px;
		object-fit: cover;
		border-radius: var(--radius-sm);
	}
	.mt {
		display: flex;
		flex-direction: column;
	}
	.mtitle {
		font: var(--text-body-md);
		color: var(--color-on-dark);
	}
	.msub {
		font: var(--text-caption);
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
