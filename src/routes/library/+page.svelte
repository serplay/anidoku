<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		getLibrary,
		setListEntry,
		anilistSyncNow,
		isDesktop,
		STATUS_ORDER,
		STATUS_LABEL,
		type LibraryItem,
		type MediaListStatus
	} from '$lib/api';
	import { authState, pushToast, rememberAnime } from '$lib/state.svelte';
	import ListControls from '$lib/components/ListControls.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import coverPlaceholder from '$lib/assets/cover-placeholder.svg';

	let items = $state<LibraryItem[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let tab = $state<MediaListStatus>('CURRENT');

	$effect(() => {
		void load();
	});

	async function load() {
		if (!isDesktop()) {
			loading = false;
			return;
		}
		loading = true;
		error = null;
		try {
			items = await getLibrary();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	const counts = $derived.by(() => {
		const c: Record<string, number> = {};
		for (const i of items) c[i.status] = (c[i.status] ?? 0) + 1;
		return c;
	});

	const shown = $derived(items.filter((i) => i.status === tab));

	function titleOf(i: LibraryItem): string {
		return i.title_english ?? i.title_romaji ?? `AniList #${i.anilist_id}`;
	}

	async function edit(i: LibraryItem, status: MediaListStatus, progress: number) {
		// Optimistic local update.
		i.status = status;
		i.progress = progress;
		i.dirty = true;
		items = [...items];
		try {
			await setListEntry(i.anilist_id, status, progress, i.score);
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	async function open(i: LibraryItem) {
		if (i.provider_id) {
			rememberAnime({
				provider_id: i.provider_id,
				title: i.title_romaji ?? titleOf(i),
				title_english: i.title_english,
				cover_url: i.cover_url,
				available_episodes: i.episode_count ?? 0,
				anilist_id: i.anilist_id
			});
			goto(`/anime/${encodeURIComponent(i.provider_id)}?dub=0`);
		} else {
			// No provider mapping yet — search by title to find a stream source.
			goto(`/?q=${encodeURIComponent(titleOf(i))}`);
		}
	}

	async function refresh() {
		try {
			await anilistSyncNow();
			await load();
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}
</script>

<div class="head">
	<h1>Library</h1>
	{#if authState.logged_in}
		<button class="refresh" onclick={refresh}>Sync now</button>
	{/if}
</div>

{#if !isDesktop()}
	<p class="hint">The library needs the desktop app: <code>npm run tauri dev</code>.</p>
{:else if !authState.logged_in && items.length === 0}
	<p class="empty">
		Not signed in to AniList. <a href="/settings">Sign in</a> to pull your list, or start watching —
		local progress is tracked and will sync once you connect.
	</p>
{/if}

<div class="tabs">
	{#each STATUS_ORDER as s (s)}
		<button class="tab" class:active={tab === s} onclick={() => (tab = s)}>
			{STATUS_LABEL[s]}
			<span class="badge">{counts[s] ?? 0}</span>
		</button>
	{/each}
</div>

{#if loading}
	<div class="list">
		{#each Array(6), i (i)}
			<div class="entrysk">
				<Skeleton width="48px" height="68px" radius="var(--radius-md)" />
				<div class="entrysk-body">
					<Skeleton width="45%" height="16px" />
					<Skeleton width="25%" height="13px" />
				</div>
			</div>
		{/each}
	</div>
{:else if error}
	<p class="error">{error}</p>
{:else if shown.length === 0}
	<p class="empty">Nothing in {STATUS_LABEL[tab]}.</p>
{:else}
	<div class="list">
		{#each shown as i (i.anilist_id)}
			<div class="entry">
				<button class="cover" onclick={() => open(i)} aria-label={titleOf(i)}>
					<img
						src={i.cover_url ?? coverPlaceholder}
						alt=""
						loading="lazy"
						onerror={(e) => ((e.currentTarget as HTMLImageElement).src = coverPlaceholder)}
					/>
				</button>
				<div class="body">
					<button class="title" onclick={() => open(i)}>{titleOf(i)}</button>
					<div class="sub">
						{i.progress}{#if i.episode_count}/{i.episode_count}{/if} episodes
						{#if !i.provider_id}<span class="tag">no source</span>{/if}
						{#if i.dirty}<span class="tag pending">pending sync</span>{/if}
					</div>
				</div>
				<ListControls
					status={i.status}
					progress={i.progress}
					episodeCount={i.episode_count}
					onchange={(s, p) => edit(i, s, p)}
				/>
			</div>
		{/each}
	</div>
{/if}

<style>
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-lg);
	}
	h1 {
		font: var(--text-display-sm);
		color: var(--color-on-dark);
		margin: 0;
	}
	.refresh {
		background: var(--color-surface-card);
		color: var(--color-on-dark);
		border: none;
		border-radius: var(--radius-md);
		padding: 8px 16px;
		height: 36px;
		font: var(--text-button);
		cursor: pointer;
	}
	.refresh:hover {
		background: var(--color-surface-elevated);
	}
	.tabs {
		display: flex;
		gap: var(--space-xs);
		flex-wrap: wrap;
		border-bottom: 1px solid var(--color-hairline);
		margin-bottom: var(--space-lg);
	}
	.tab {
		background: transparent;
		border: none;
		border-bottom: 2px solid transparent;
		color: var(--color-muted-strong);
		font: var(--text-nav);
		padding: 10px 12px;
		cursor: pointer;
		display: flex;
		align-items: center;
		gap: var(--space-xs);
	}
	.tab:hover {
		color: var(--color-on-dark);
	}
	.tab.active {
		color: var(--color-on-dark);
		border-bottom-color: var(--color-primary);
	}
	.badge {
		font: var(--text-caption);
		color: var(--color-muted);
		background: var(--color-surface-card);
		border-radius: var(--radius-pill);
		padding: 1px 7px;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.entry {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		background: var(--color-surface-card);
		border-radius: var(--radius-lg);
		padding: var(--space-sm);
	}
	.cover {
		border: none;
		background: none;
		padding: 0;
		cursor: pointer;
		width: 46px;
		height: 66px;
		flex-shrink: 0;
		border-radius: var(--radius-sm);
		overflow: hidden;
	}
	.cover img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.entrysk {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: var(--space-xs) 0;
	}
	.entrysk-body {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.body {
		flex: 1;
		min-width: 0;
	}
	.title {
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		color: var(--color-on-dark);
		font: var(--text-title-sm);
		cursor: pointer;
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 100%;
	}
	.title:hover {
		color: var(--color-primary);
	}
	.sub {
		font: var(--text-num-sm);
		color: var(--color-muted);
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		margin-top: 2px;
	}
	.tag {
		font: var(--text-caption);
		color: var(--color-muted-strong);
		background: var(--color-surface-elevated);
		padding: 1px 6px;
		border-radius: var(--radius-sm);
	}
	.tag.pending {
		color: var(--color-primary);
	}
	.hint,
	.empty {
		color: var(--color-muted);
		font: var(--text-body-md);
	}
	.hint code {
		color: var(--color-primary);
	}
	.error {
		color: var(--color-down);
	}
	a {
		color: var(--color-primary);
	}
</style>
