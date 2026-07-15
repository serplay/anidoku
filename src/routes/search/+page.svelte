<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		searchCatalog,
		getMediaTags,
		resolveProviderForAnilist,
		catalogMediaMeta,
		displayTitle,
		isDesktop,
		GENRES,
		STATUS_OPTIONS,
		FORMAT_OPTIONS,
		type CatalogMedia,
		type CatalogFilters,
		type MediaTag,
		type AnimeSummary
	} from '$lib/api';
	import {
		rememberAnime,
		pushToast,
		catalogState,
		emptyCatalogFilters,
		isUnreleasedStatus,
		handleUnreleasedClick
	} from '$lib/state.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import Button from '$lib/components/Button.svelte';
	import AnimeCard from '$lib/components/AnimeCard.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';

	let filters = $state<CatalogFilters>({ ...catalogState.filters });
	let dub = $state(catalogState.dub);
	let results = $state<CatalogMedia[]>(catalogState.results);
	let hasNext = $state(catalogState.hasNext);
	let curPage = $state(catalogState.page);
	let ran = $state(catalogState.ran);
	let loading = $state(false);
	let loadingMore = $state(false);
	let error = $state<string | null>(null);
	let filtersOpen = $state(false);

	// Tag list (cached in the DB with a long TTL; one fetch per app session here).
	let tags = $state<MediaTag[]>([]);
	let tagQuery = $state('');
	const adultTagNames = $derived(new Set(tags.filter((t) => t.is_adult).map((t) => t.name)));
	const tagMatches = $derived.by(() => {
		const q = tagQuery.trim().toLowerCase();
		if (!q) return [];
		return tags
			.filter((t) => t.name.toLowerCase().includes(q) && !filters.tags.includes(t.name))
			.slice(0, 20);
	});

	// AniList ids currently resolving to a provider show (click feedback).
	let resolving = $state<Record<number, boolean>>({});

	// URL is the source of truth for filters (back/forward + Library ?q= deep-link).
	// This effect runs whenever the query string changes: parse it, and either run
	// a search (any criteria present) or restore the last cached result set.
	let lastSearch = $state<string | null>(null);
	$effect(() => {
		const search = page.url.search; // track
		if (search === lastSearch) return;
		lastSearch = search;
		const f = parseUrl();
		filters = f;
		dub = page.url.searchParams.get('dub') === '1';
		if (hasCriteria(f)) {
			void run(1);
		} else if (catalogState.ran) {
			// Returning to /search with no params: restore the previous session.
			filters = { ...catalogState.filters };
			dub = catalogState.dub;
			results = catalogState.results;
			hasNext = catalogState.hasNext;
			curPage = catalogState.page;
			ran = true;
		}
	});

	$effect(() => {
		if (isDesktop() && tags.length === 0) {
			getMediaTags()
				.then((t) => (tags = t))
				.catch(() => {});
		}
	});

	function parseUrl(): CatalogFilters {
		const p = page.url.searchParams;
		const list = (k: string) => (p.get(k) ? p.get(k)!.split(',').filter(Boolean) : []);
		const f = emptyCatalogFilters();
		f.query = p.get('q') ?? '';
		f.genres = list('genres');
		f.tags = list('tags');
		const y = p.get('year');
		f.season_year = y && /^\d+$/.test(y) ? Number(y) : null;
		f.status = list('status');
		f.format = list('format');
		return f;
	}

	function activeCount(f: CatalogFilters): number {
		return (
			f.genres.length +
			f.tags.length +
			f.status.length +
			f.format.length +
			(f.season_year ? 1 : 0)
		);
	}

	function hasCriteria(f: CatalogFilters): boolean {
		return f.query.trim().length > 0 || activeCount(f) > 0;
	}

	function includeAdult(f: CatalogFilters): boolean {
		return f.genres.includes('Hentai') || f.tags.some((t) => adultTagNames.has(t));
	}

	// Commit the current filters to the URL; the effect above runs the search.
	function submit() {
		const params = new URLSearchParams();
		if (filters.query.trim()) params.set('q', filters.query.trim());
		if (filters.genres.length) params.set('genres', filters.genres.join(','));
		if (filters.tags.length) params.set('tags', filters.tags.join(','));
		if (filters.season_year) params.set('year', String(filters.season_year));
		if (filters.status.length) params.set('status', filters.status.join(','));
		if (filters.format.length) params.set('format', filters.format.join(','));
		if (dub) params.set('dub', '1');
		const qs = params.toString();
		goto(`/search${qs ? `?${qs}` : ''}`, { keepFocus: true, noScroll: true });
	}

	async function run(pageNum: number) {
		if (!isDesktop()) return;
		loading = pageNum === 1;
		loadingMore = pageNum > 1;
		error = null;
		const req: CatalogFilters = {
			...filters,
			page: pageNum,
			per_page: 30,
			include_adult: includeAdult(filters)
		};
		try {
			const res = await searchCatalog(req);
			results = pageNum === 1 ? res.media : [...results, ...res.media];
			hasNext = res.has_next_page;
			curPage = res.current_page;
			ran = true;
			// Persist so a return navigation restores instantly.
			catalogState.filters = { ...filters };
			catalogState.results = results;
			catalogState.hasNext = hasNext;
			catalogState.page = curPage;
			catalogState.dub = dub;
			catalogState.ran = true;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			if (pageNum === 1) results = [];
		} finally {
			loading = false;
			loadingMore = false;
		}
	}

	function loadMore() {
		void run(curPage + 1);
	}

	// --- filter mutators ---
	function toggleGenre(g: string) {
		filters.genres = filters.genres.includes(g)
			? filters.genres.filter((x) => x !== g)
			: [...filters.genres, g];
	}
	function addTag(name: string) {
		if (!filters.tags.includes(name)) filters.tags = [...filters.tags, name];
		tagQuery = '';
	}
	function removeTag(name: string) {
		filters.tags = filters.tags.filter((x) => x !== name);
	}
	function setStatus(v: string) {
		filters.status = v ? [v] : [];
	}
	function setFormat(v: string) {
		filters.format = v ? [v] : [];
	}
	function clearAll() {
		const q = filters.query;
		filters = { ...emptyCatalogFilters(), query: q };
		tagQuery = '';
	}

	// Card click: AniList id → provider id via the existing resolver; on success
	// deep-link to the detail page (dub preserved as a URL param), otherwise toast.
	async function open(m: CatalogMedia) {
		if (resolving[m.anilist_id]) return;
		const title = m.title_english ?? m.title_romaji ?? '';
		// Not-yet-aired shows have no stream to resolve — don't hit the provider.
		if (isUnreleasedStatus(m.status)) {
			await handleUnreleasedClick(m.anilist_id, title, m.season_year);
			return;
		}
		resolving = { ...resolving, [m.anilist_id]: true };
		try {
			const summary = await resolveProviderForAnilist(m.anilist_id, title, m.episode_count);
			if (summary) {
				rememberAnime(summary);
				goto(`/anime/${encodeURIComponent(summary.provider_id)}?dub=${dub ? 1 : 0}`);
			} else {
				pushToast(`No stream source found for “${title}”.`);
			}
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			const { [m.anilist_id]: _done, ...rest } = resolving;
			resolving = rest;
		}
	}

	function cardSummary(m: CatalogMedia): AnimeSummary {
		return {
			provider_id: `anilist:${m.anilist_id}`,
			title: m.title_romaji ?? m.title_english ?? `AniList #${m.anilist_id}`,
			title_english: m.title_english,
			cover_url: m.cover_url,
			available_episodes: m.episode_count ?? 0,
			anilist_id: m.anilist_id
		};
	}

	const nActive = $derived(activeCount(filters));
	// Year dropdown: current year + 1 down to 1940.
	const YEARS = Array.from({ length: new Date().getFullYear() + 2 - 1940 }, (_, i) =>
		new Date().getFullYear() + 1 - i
	);
</script>

<section class="head">
	<h1>Search</h1>
	<div class="controls">
		<div class="grow"><SearchInput bind:value={filters.query} onsubmit={submit} /></div>
		<label class="dub">
			<input type="checkbox" bind:checked={dub} /> Dub
		</label>
		<Button onclick={submit} disabled={loading}>{loading ? 'Searching…' : 'Search'}</Button>
	</div>

	<div class="filterbar">
		<button class="filtertoggle" onclick={() => (filtersOpen = !filtersOpen)}>
			<span>Filters</span>
			{#if nActive > 0}<span class="count">{nActive}</span>{/if}
			<span class="chevron" class:open={filtersOpen}>▾</span>
		</button>
		{#if nActive > 0}
			<button class="clear" onclick={clearAll}>Clear all</button>
		{/if}
	</div>

	{#if filtersOpen}
		<div class="panel">
			<div class="group">
				<span class="glabel">Genre</span>
				<div class="chiprow">
					{#each GENRES as g (g)}
						<button
							class="fchip"
							class:on={filters.genres.includes(g)}
							onclick={() => toggleGenre(g)}
						>
							{g}
						</button>
					{/each}
				</div>
			</div>

			<div class="group">
				<span class="glabel">Tags</span>
				{#if filters.tags.length > 0}
					<div class="chiprow selected">
						{#each filters.tags as t (t)}
							<button class="fchip on removable" onclick={() => removeTag(t)}>
								{t} <span class="x">×</span>
							</button>
						{/each}
					</div>
				{/if}
				<div class="tagpicker">
					<input
						class="taginput"
						placeholder="Filter tags…"
						bind:value={tagQuery}
						spellcheck="false"
						autocomplete="off"
					/>
					{#if tagMatches.length > 0}
						<div class="tagmenu">
							{#each tagMatches as t (t.name)}
								<button class="tagopt" onclick={() => addTag(t.name)}>
									<span>{t.name}</span>
									{#if t.category}<span class="tagcat">{t.category}</span>{/if}
								</button>
							{/each}
						</div>
					{/if}
				</div>
			</div>

			<div class="group inline">
				<div class="sub">
					<span class="glabel">Year</span>
					<select
						class="select"
						value={filters.season_year ?? ''}
						onchange={(e) => {
							const v = (e.currentTarget as HTMLSelectElement).value;
							filters.season_year = v ? Number(v) : null;
						}}
					>
						<option value="">Any</option>
						{#each YEARS as y (y)}
							<option value={y}>{y}</option>
						{/each}
					</select>
				</div>

				<div class="sub">
					<span class="glabel">Status</span>
					<select
						class="select"
						value={filters.status[0] ?? ''}
						onchange={(e) => setStatus((e.currentTarget as HTMLSelectElement).value)}
					>
						<option value="">Any</option>
						{#each STATUS_OPTIONS as s (s.value)}
							<option value={s.value}>{s.label}</option>
						{/each}
					</select>
				</div>

				<div class="sub">
					<span class="glabel">Format</span>
					<select
						class="select"
						value={filters.format[0] ?? ''}
						onchange={(e) => setFormat((e.currentTarget as HTMLSelectElement).value)}
					>
						<option value="">Any</option>
						{#each FORMAT_OPTIONS as f (f.value)}
							<option value={f.value}>{f.label}</option>
						{/each}
					</select>
				</div>
			</div>

			<div class="panelactions">
				<Button onclick={submit} disabled={loading}>Apply filters</Button>
			</div>
		</div>
	{/if}

	{#if !isDesktop()}
		<p class="hint">
			Running in a browser — search needs the desktop app: <code>npm run tauri dev</code>.
		</p>
	{/if}
</section>

{#if error}
	<p class="error">{error}</p>
{/if}

{#if loading}
	<div class="grid">
		{#each Array(12), i (i)}
			<div class="cardsk">
				<Skeleton aspect="2 / 3" radius="var(--radius-lg)" />
				<Skeleton width="85%" height="14px" />
				<Skeleton width="55%" height="14px" />
			</div>
		{/each}
	</div>
{:else if results.length > 0}
	<div class="grid">
		{#each results as m (m.anilist_id)}
			<AnimeCard
				anime={cardSummary(m)}
				meta={catalogMediaMeta(m)}
				proxyCover={false}
				showEpisodes={false}
				onselect={() => open(m)}
			/>
		{/each}
	</div>
	{#if hasNext}
		<div class="more">
			<Button variant="secondary" onclick={loadMore} disabled={loadingMore}>
				{loadingMore ? 'Loading…' : 'Load more'}
			</Button>
		</div>
	{/if}
{:else if ran}
	<p class="empty">No results. Try a different title or fewer filters.</p>
{/if}

<style>
	.head {
		margin-bottom: var(--space-xl);
	}
	h1 {
		font: var(--text-display-sm);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-lg);
	}
	.controls {
		display: flex;
		align-items: center;
		gap: var(--space-md);
	}
	.grow {
		flex: 1;
	}
	.dub {
		display: flex;
		align-items: center;
		gap: var(--space-xxs);
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		cursor: pointer;
		user-select: none;
	}
	.dub input {
		accent-color: var(--color-primary);
	}
	.filterbar {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		margin-top: var(--space-md);
	}
	.filtertoggle {
		display: inline-flex;
		align-items: center;
		gap: var(--space-xs);
		background: var(--color-surface-card);
		border: 1px solid transparent;
		color: var(--color-body);
		font: var(--text-button);
		border-radius: var(--radius-md);
		padding: 8px 14px;
		height: 36px;
		cursor: pointer;
	}
	.filtertoggle:hover {
		background: var(--color-surface-elevated);
	}
	.count {
		background: var(--color-primary);
		color: var(--color-on-primary);
		font: var(--text-caption);
		border-radius: var(--radius-pill);
		min-width: 18px;
		height: 18px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		padding: 0 5px;
	}
	.chevron {
		font-size: 10px;
		color: var(--color-muted);
		transition: transform 0.15s ease;
	}
	.chevron.open {
		transform: rotate(180deg);
	}
	.clear {
		background: transparent;
		border: none;
		color: var(--color-muted-strong);
		font: var(--text-body-sm);
		cursor: pointer;
	}
	.clear:hover {
		color: var(--color-body);
	}
	.panel {
		margin-top: var(--space-md);
		max-width: 100%;
		background: var(--color-surface-card);
		border-radius: var(--radius-xl);
		padding: var(--space-lg);
		display: flex;
		flex-direction: column;
		gap: var(--space-lg);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		min-width: 0;
	}
	.group.inline {
		flex-direction: row;
		flex-wrap: wrap;
		gap: var(--space-lg);
	}
	.sub {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		flex: 1 1 140px;
		min-width: 0;
	}
	.glabel {
		font: var(--text-caption);
		color: var(--color-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.chiprow {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-xs);
	}
	.fchip {
		background: var(--color-surface-elevated);
		border: 1px solid transparent;
		color: var(--color-body);
		font: var(--text-caption);
		padding: 5px 10px;
		border-radius: var(--radius-pill);
		cursor: pointer;
	}
	.fchip:hover {
		border-color: var(--color-hairline);
	}
	.fchip.on {
		background: var(--color-primary);
		color: var(--color-on-primary);
		border-color: var(--color-primary);
	}
	.fchip.removable .x {
		opacity: 0.7;
		margin-left: 2px;
	}
	.tagpicker {
		position: relative;
		max-width: 320px;
	}
	.taginput {
		width: 100%;
		background: var(--color-surface-elevated);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: 8px 12px;
		color: var(--color-on-dark);
		font: var(--text-body-md);
		outline: none;
	}
	.tagmenu {
		position: absolute;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		z-index: 30;
		background: var(--color-surface-elevated);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		max-height: 240px;
		overflow-y: auto;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
	}
	.tagopt {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-sm);
		width: 100%;
		background: transparent;
		border: none;
		text-align: left;
		color: var(--color-body);
		font: var(--text-body-sm);
		padding: 8px 12px;
		cursor: pointer;
	}
	.tagopt:hover {
		background: var(--color-surface-card);
	}
	.tagcat {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.select {
		max-width: 100%;
		background: var(--color-surface-elevated);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: 8px 12px;
		height: 38px;
		color: var(--color-on-dark);
		font: var(--text-body-md);
		cursor: pointer;
	}
	.panelactions {
		display: flex;
		justify-content: flex-end;
	}
	.hint {
		font: var(--text-body-sm);
		color: var(--color-muted);
		margin-top: var(--space-sm);
	}
	.hint code {
		color: var(--color-primary);
	}
	.error {
		color: var(--color-down);
		font: var(--text-body-md);
	}
	.empty {
		color: var(--color-muted);
	}
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: var(--space-lg);
	}
	.cardsk {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.more {
		display: flex;
		justify-content: center;
		margin-top: var(--space-xl);
	}
</style>
