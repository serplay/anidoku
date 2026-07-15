<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		getHomeCached,
		refreshHome,
		getContinueWatching,
		resolveProviderForAnilist,
		untilCaption,
		displayTitle,
		homeMediaMeta,
		isDesktop,
		type HomeSections,
		type HomeMedia,
		type ContinueWatchingItem,
		type AnimeSummary
	} from '$lib/api';
	import { rememberAnime, pushToast } from '$lib/state.svelte';
	import HomeRow from '$lib/components/HomeRow.svelte';
	import AnimeCard from '$lib/components/AnimeCard.svelte';

	let sections = $state<HomeSections | null>(null);
	let loading = $state(true);
	let refreshing = $state(false);
	let continueWatching = $state<ContinueWatchingItem[]>([]);
	// AniList ids currently resolving to a provider show (click feedback).
	let resolving = $state<Record<number, boolean>>({});

	$effect(() => {
		void init();
	});

	async function init() {
		if (!isDesktop()) {
			loading = false;
			return;
		}
		// Continue Watching is fully local: render immediately, works offline.
		try {
			continueWatching = await getContinueWatching();
		} catch {
			/* empty library */
		}
		// Home rows: render from cache instantly, refresh in background when
		// stale (6h TTL) or missing.
		try {
			const cached = await getHomeCached();
			if (cached) {
				sections = cached.sections;
				loading = false;
				if (!cached.fresh) void refresh();
			} else {
				await refresh();
			}
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			loading = false;
		}
	}

	async function refresh() {
		refreshing = true;
		try {
			sections = (await refreshHome()).sections;
		} catch {
			// Offline / AniList down: cached (or empty) rows stay up.
		} finally {
			refreshing = false;
		}
	}

	// HomeMedia → the AnimeSummary shape AnimeCard renders. provider_id is
	// unknown until the click-time resolve.
	function toSummary(m: HomeMedia): AnimeSummary {
		return {
			provider_id: `anilist:${m.anilist_id}`,
			title: m.title_romaji ?? m.title_english ?? `AniList #${m.anilist_id}`,
			title_english: m.title_english,
			cover_url: m.cover_url,
			available_episodes: m.episode_count ?? 0,
			anilist_id: m.anilist_id
		};
	}

	function cwSummary(i: ContinueWatchingItem): AnimeSummary {
		return {
			provider_id: i.provider_id ?? `anilist:${i.anilist_id}`,
			title: i.title_romaji ?? displayTitle(i),
			title_english: i.title_english,
			cover_url: i.cover_url,
			available_episodes: i.episode_count ?? 0,
			anilist_id: i.anilist_id
		};
	}

	function caption(m: HomeMedia): string | null {
		if (m.next_episode && m.airing_at) {
			return `Ep ${m.next_episode} in ${untilCaption(m.airing_at)}`;
		}
		return null;
	}

	function cwCaption(i: ContinueWatchingItem): string {
		if (i.next_episode) return `Next: Ep ${i.next_episode}`;
		return 'Caught up';
	}

	// Continue Watching click: deep-link straight to the next unwatched episode
	// when a provider mapping exists; otherwise resolve like a home card.
	async function openContinue(i: ContinueWatchingItem) {
		if (i.provider_id) {
			rememberAnime(cwSummary(i));
			if (i.next_episode) {
				goto(
					`/watch/${encodeURIComponent(i.provider_id)}/${encodeURIComponent(i.next_episode)}?dub=0`
				);
			} else {
				goto(`/anime/${encodeURIComponent(i.provider_id)}?dub=0`);
			}
			return;
		}
		await openAniList(i.anilist_id, displayTitle(i), i.episode_count);
	}

	// Home card click: AniList id → provider id (existing mapping, else
	// provider search matched by carried aniListId) → detail page. Falls back
	// to /search?q=title when unresolvable.
	async function openMedia(m: HomeMedia) {
		await openAniList(m.anilist_id, m.title_english ?? m.title_romaji ?? '', m.episode_count);
	}

	async function openAniList(anilistId: number, title: string, episodes: number | null) {
		if (resolving[anilistId]) return;
		resolving = { ...resolving, [anilistId]: true };
		try {
			const summary = await resolveProviderForAnilist(anilistId, title, episodes);
			if (summary) {
				rememberAnime(summary);
				goto(`/anime/${encodeURIComponent(summary.provider_id)}?dub=0`);
			} else {
				pushToast('No stream source matched — showing search results');
				goto(`/search?q=${encodeURIComponent(title)}`);
			}
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
			goto(`/search?q=${encodeURIComponent(title)}`);
		} finally {
			const { [anilistId]: _done, ...rest } = resolving;
			resolving = rest;
		}
	}
</script>

<div class="head">
	<h1>Home</h1>
	{#if refreshing}<span class="refreshing">Updating…</span>{/if}
</div>

{#if !isDesktop()}
	<p class="hint">
		The home page needs the desktop app: <code>npm run tauri dev</code>.
	</p>
{:else}
	{#if continueWatching.length > 0}
		<HomeRow title="Continue Watching">
			{#each continueWatching as i (i.anilist_id)}
				<AnimeCard
					anime={cwSummary(i)}
					caption={cwCaption(i)}
					proxyCover={false}
					onselect={() => openContinue(i)}
				/>
			{/each}
		</HomeRow>
	{/if}

	<HomeRow
		title="Trending Now"
		loading={loading && !sections}
		empty={sections && sections.trending.length === 0 ? 'Nothing to show — check back later.' : null}
	>
		{#each sections?.trending ?? [] as m (m.anilist_id)}
			<AnimeCard
				anime={toSummary(m)}
				caption={caption(m)}
				meta={homeMediaMeta(m)}
				proxyCover={false}
				onselect={() => openMedia(m)}
			/>
		{/each}
	</HomeRow>

	<HomeRow
		title="Popular This Season"
		loading={loading && !sections}
		empty={sections && sections.season.length === 0 ? 'Nothing to show — check back later.' : null}
	>
		{#each sections?.season ?? [] as m (m.anilist_id)}
			<AnimeCard
				anime={toSummary(m)}
				caption={caption(m)}
				meta={homeMediaMeta(m)}
				proxyCover={false}
				onselect={() => openMedia(m)}
			/>
		{/each}
	</HomeRow>

	<HomeRow
		title="Upcoming Next Season"
		loading={loading && !sections}
		empty={sections && sections.next_season.length === 0
			? 'Nothing announced yet.'
			: null}
	>
		{#each sections?.next_season ?? [] as m (m.anilist_id)}
			<AnimeCard
				anime={toSummary(m)}
				caption={caption(m)}
				meta={homeMediaMeta(m)}
				proxyCover={false}
				onselect={() => openMedia(m)}
			/>
		{/each}
	</HomeRow>
{/if}

<style>
	.head {
		display: flex;
		align-items: baseline;
		gap: var(--space-md);
		margin-bottom: var(--space-lg);
	}
	h1 {
		font: var(--text-display-sm);
		color: var(--color-on-dark);
		margin: 0;
	}
	.refreshing {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.hint {
		font: var(--text-body-sm);
		color: var(--color-muted);
	}
	.hint code {
		color: var(--color-primary);
	}
</style>
