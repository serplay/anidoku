<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { searchAnime, isDesktop, type AnimeSummary } from '$lib/api';
	import { rememberAnime, searchState } from '$lib/state.svelte';
	import SearchInput from '$lib/components/SearchInput.svelte';
	import Button from '$lib/components/Button.svelte';
	import AnimeCard from '$lib/components/AnimeCard.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';

	let query = $state(searchState.query);
	let dub = $state(searchState.dub);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let results = $state<AnimeSummary[]>(searchState.results);

	// A `?q=` param (e.g. from a Library entry with no stream source) pre-fills
	// and runs the search once.
	$effect(() => {
		const q = page.url.searchParams.get('q');
		if (q && q !== searchState.query) {
			query = q;
			void run();
		}
	});

	async function run() {
		const q = query.trim();
		if (!q) return;
		loading = true;
		error = null;
		try {
			results = await searchAnime(q, dub);
			searchState.query = q;
			searchState.results = results;
			searchState.dub = dub;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
			results = [];
		} finally {
			loading = false;
		}
	}

	function open(a: AnimeSummary) {
		rememberAnime(a);
		goto(`/anime/${encodeURIComponent(a.provider_id)}?dub=${dub ? 1 : 0}`);
	}
</script>

<section class="head">
	<h1>Find something to watch</h1>
	<div class="controls">
		<div class="grow"><SearchInput bind:value={query} onsubmit={run} /></div>
		<label class="dub">
			<input type="checkbox" bind:checked={dub} /> Dub
		</label>
		<Button onclick={run} disabled={loading}>{loading ? 'Searching…' : 'Search'}</Button>
	</div>
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
		{#each results as a (a.provider_id)}
			<AnimeCard anime={a} onselect={open} />
		{/each}
	</div>
{:else if searchState.query}
	<p class="empty">No results for “{searchState.query}”.</p>
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
</style>
