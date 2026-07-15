<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		getNotifications,
		getUpcoming,
		markNotificationsRead,
		clearNotifications,
		airingRefreshNow,
		resolveProviderForAnilist,
		unreadNotifications,
		onEvent,
		isDesktop,
		airsAtLabel,
		untilCaption,
		displayTitle,
		type Notification
	} from '$lib/api';
	import { notifyState, pushToast, rememberAnime } from '$lib/state.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import coverPlaceholder from '$lib/assets/cover-placeholder.svg';

	let upcoming = $state<Notification[]>([]);
	let fired = $state<Notification[]>([]);
	let loading = $state(true);
	let busy = $state(false);
	let resolvingId = $state<number | null>(null);

	$effect(() => {
		void load();
		// Live updates while the page is open.
		const unlisteners = [
			onEvent('notify:new', () => void load()),
			onEvent('airing:updated', () => void load())
		];
		return () => {
			for (const u of unlisteners) u.then((fn) => fn());
		};
	});

	async function load() {
		if (!isDesktop()) {
			loading = false;
			return;
		}
		try {
			[upcoming, fired] = await Promise.all([getUpcoming(), getNotifications()]);
			notifyState.unread = await unreadNotifications();
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			loading = false;
		}
	}

	async function markAllRead() {
		try {
			await markNotificationsRead();
			notifyState.unread = 0;
			fired = fired.map((n) => ({ ...n, read: true }));
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	async function clearAll() {
		try {
			await clearNotifications();
			fired = [];
			notifyState.unread = 0;
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	async function refresh() {
		busy = true;
		try {
			await airingRefreshNow();
			await load();
		} finally {
			busy = false;
		}
	}

	// Click → detail page via resolve; unresolvable → /search fallback.
	async function open(n: Notification) {
		const title = displayTitle(n);
		if (n.provider_id) {
			rememberAnime({
				provider_id: n.provider_id,
				title: n.title_romaji ?? title,
				title_english: n.title_english,
				cover_url: n.cover_url,
				available_episodes: 0,
				anilist_id: n.anilist_id
			});
			goto(`/anime/${encodeURIComponent(n.provider_id)}?dub=0`);
			return;
		}
		if (resolvingId !== null) return;
		resolvingId = n.anilist_id;
		try {
			const summary = await resolveProviderForAnilist(n.anilist_id, title, null);
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
			resolvingId = null;
		}
	}
</script>

<div class="head">
	<h1>Inbox</h1>
	<div class="actions">
		<button class="ghost" onclick={refresh} disabled={busy}>
			{busy ? 'Refreshing…' : 'Refresh'}
		</button>
		{#if fired.some((n) => !n.read)}
			<button class="ghost" onclick={markAllRead}>Mark all read</button>
		{/if}
		{#if fired.length > 0}
			<button class="ghost danger" onclick={clearAll}>Clear</button>
		{/if}
	</div>
</div>

{#if !isDesktop()}
	<p class="hint">The inbox needs the desktop app: <code>npm run tauri dev</code>.</p>
{:else if loading}
	<div class="list">
		{#each Array(4), i (i)}
			<div class="rowsk">
				<Skeleton width="42px" height="58px" radius="var(--radius-md)" />
				<div class="rowsk-body">
					<Skeleton width="45%" height="15px" />
					<Skeleton width="30%" height="12px" />
				</div>
			</div>
		{/each}
	</div>
{:else}
	<section>
		<h2>Upcoming</h2>
		{#if upcoming.length === 0}
			<p class="empty">
				No tracked airings. Shows on your Watching list that are currently airing appear here.
			</p>
		{:else}
			<div class="list">
				{#each upcoming as n (n.anilist_id)}
					<button class="row" onclick={() => open(n)} disabled={resolvingId === n.anilist_id}>
						<img
							src={n.cover_url ?? coverPlaceholder}
							alt=""
							loading="lazy"
							onerror={(e) => ((e.currentTarget as HTMLImageElement).src = coverPlaceholder)}
						/>
						<span class="body">
							<span class="title">{displayTitle(n)}</span>
							<span class="sub">
								Ep {n.episode} airs {n.airing_at ? airsAtLabel(n.airing_at) : 'soon'}
							</span>
						</span>
						{#if n.airing_at}
							<span class="countdown">in {untilCaption(n.airing_at)}</span>
						{/if}
					</button>
				{/each}
			</div>
		{/if}
	</section>

	<section>
		<h2>New episodes</h2>
		{#if fired.length === 0}
			<p class="empty">Nothing yet — released episodes of tracked shows land here.</p>
		{:else}
			<div class="list">
				{#each fired as n (n.id)}
					<button
						class="row"
						class:unread={!n.read}
						onclick={() => open(n)}
						disabled={resolvingId === n.anilist_id}
					>
						<img
							src={n.cover_url ?? coverPlaceholder}
							alt=""
							loading="lazy"
							onerror={(e) => ((e.currentTarget as HTMLImageElement).src = coverPlaceholder)}
						/>
						<span class="body">
							<span class="title">{displayTitle(n)}</span>
							<span class="sub">Episode {n.episode} is out</span>
						</span>
						{#if !n.read}<span class="dot" aria-label="unread"></span>{/if}
					</button>
				{/each}
			</div>
		{/if}
	</section>
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
	.actions {
		display: flex;
		gap: var(--space-xs);
	}
	.ghost {
		background: var(--color-surface-card);
		color: var(--color-on-dark);
		border: none;
		border-radius: var(--radius-md);
		padding: 8px 14px;
		height: 34px;
		font: var(--text-button);
		cursor: pointer;
	}
	.ghost:hover:not(:disabled) {
		background: var(--color-surface-elevated);
	}
	.ghost.danger:hover {
		color: var(--color-down);
	}
	section {
		margin-bottom: var(--space-xl);
	}
	h2 {
		font: var(--text-title-lg);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-md);
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		background: var(--color-surface-card);
		border: none;
		border-radius: var(--radius-lg);
		padding: var(--space-sm);
		cursor: pointer;
		text-align: left;
	}
	.row:hover {
		background: var(--color-surface-elevated);
	}
	.row.unread {
		border-left: 3px solid var(--color-primary);
	}
	.row img {
		width: 42px;
		height: 58px;
		object-fit: cover;
		border-radius: var(--radius-sm);
		flex-shrink: 0;
		background: var(--color-canvas);
	}
	.body {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.title {
		font: var(--text-title-sm);
		color: var(--color-on-dark);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.sub {
		font: var(--text-body-sm);
		color: var(--color-muted-strong);
	}
	.countdown {
		font: var(--text-num-sm);
		color: var(--color-primary);
		flex-shrink: 0;
	}
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--color-primary);
		flex-shrink: 0;
	}
	.rowsk {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: var(--space-xs) 0;
	}
	.rowsk-body {
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.empty,
	.hint {
		font: var(--text-body-md);
		color: var(--color-muted);
	}
	.hint code {
		color: var(--color-primary);
	}
	@media (max-width: 767px) {
		h1 {
			font: var(--text-title-lg);
		}
		.head {
			flex-wrap: wrap;
			gap: var(--space-sm);
		}
	}
</style>
