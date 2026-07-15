<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		listDownloads,
		downloadStorage,
		pauseDownload,
		resumeDownload,
		cancelDownload,
		deleteAnimeDownloads,
		deleteCompletedDownloads,
		onEvent,
		isDesktop,
		formatBytes,
		downloadFraction,
		type DownloadRow,
		type DownloadStorage,
		type DownloadProgressEvent,
		type DownloadStateEvent
	} from '$lib/api';
	import { pushToast } from '$lib/state.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';

	let rows = $state<DownloadRow[]>([]);
	let storage = $state<DownloadStorage>({ per_anime: [], total_bytes: 0 });
	let speeds = $state<Record<number, number>>({});
	let loading = $state(true);
	let error = $state<string | null>(null);

	const ACTIVE_STATES = ['queued', 'downloading', 'paused', 'failed'] as const;
	const active = $derived(
		rows.filter((r) => (ACTIVE_STATES as readonly string[]).includes(r.state))
	);
	const completedByAnime = $derived.by(() => {
		const m = new Map<string, DownloadRow[]>();
		for (const r of rows) {
			if (r.state !== 'done') continue;
			const list = m.get(r.anime_id) ?? [];
			list.push(r);
			m.set(r.anime_id, list);
		}
		for (const list of m.values()) {
			list.sort((a, b) => parseFloat(a.episode_number) - parseFloat(b.episode_number));
		}
		return m;
	});
	const storageOf = $derived.by(() => {
		const m = new Map<string, number>();
		for (const s of storage.per_anime) m.set(s.anime_id, s.bytes);
		return m;
	});

	async function load() {
		if (!isDesktop()) {
			loading = false;
			return;
		}
		try {
			[rows, storage] = await Promise.all([listDownloads(), downloadStorage()]);
			error = null;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
		const unlisteners = [
			onEvent<DownloadStateEvent>('download:state', (p) => {
				if (p.removed) {
					rows = rows.filter((r) => r.id !== p.id);
					void refreshStorage();
					return;
				}
				const i = rows.findIndex((r) => r.id === p.id);
				if (i >= 0) {
					rows[i] = { ...rows[i], state: p.state, error: p.error };
					rows = [...rows];
				} else {
					void load(); // a row we have not seen yet (fresh enqueue)
				}
				if (p.state === 'done') void refreshStorage();
			}),
			onEvent<DownloadProgressEvent>('download:progress', (p) => {
				const i = rows.findIndex((r) => r.id === p.id);
				if (i >= 0) {
					rows[i] = {
						...rows[i],
						bytes_done: p.bytes_done,
						bytes_total: p.bytes_total ?? rows[i].bytes_total,
						segments_done: p.segments_done,
						segments_total: p.segments_total ?? rows[i].segments_total
					};
					rows = [...rows];
				}
				speeds = { ...speeds, [p.id]: p.speed_bps };
			})
		];
		return () => {
			for (const u of unlisteners) u.then((fn) => fn());
		};
	});

	async function refreshStorage() {
		try {
			storage = await downloadStorage();
		} catch {
			/* best-effort */
		}
	}

	async function act(fn: () => Promise<unknown>) {
		try {
			await fn();
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	// Deleting downloaded files is irreversible, so every delete goes through
	// a confirm dialog first.
	let confirm = $state<{ title: string; body: string; action: () => Promise<void> } | null>(null);

	async function runConfirmed() {
		const c = confirm;
		confirm = null;
		if (c) await c.action();
	}

	function askDeleteAnime(animeId: string, title: string | null, count: number) {
		confirm = {
			title: `Delete ${title ?? animeId}?`,
			body: `Removes ${count} downloaded episode${count === 1 ? '' : 's'} from disk.`,
			action: () =>
				act(async () => {
					const n = await deleteAnimeDownloads(animeId);
					pushToast(`Deleted ${n} episode${n === 1 ? '' : 's'} of ${title ?? animeId}`);
					await load();
				})
		};
	}

	function askDeleteAllCompleted() {
		const n = [...completedByAnime.values()].reduce((s, l) => s + l.length, 0);
		confirm = {
			title: 'Delete all completed downloads?',
			body: `Removes ${n} downloaded episode${n === 1 ? '' : 's'} from disk.`,
			action: () =>
				act(async () => {
					const d = await deleteCompletedDownloads();
					pushToast(`Deleted ${d} completed download${d === 1 ? '' : 's'}`);
					await load();
				})
		};
	}

	function askDeleteEpisode(r: DownloadRow) {
		confirm = {
			title: `Delete episode ${r.episode_number}?`,
			body: `Removes the downloaded file (${formatBytes(r.bytes_done)}) from disk.`,
			action: () =>
				act(async () => {
					await cancelDownload(r.id);
				})
		};
	}

	function titleOf(r: DownloadRow): string {
		return r.title ?? r.anime_id;
	}

	function pct(r: DownloadRow): number {
		return Math.round(downloadFraction(r) * 100);
	}

	function progressDetail(r: DownloadRow): string {
		if (r.kind === 'hls' && r.segments_total) {
			return `${r.segments_done}/${r.segments_total} segments · ${formatBytes(r.bytes_done)}`;
		}
		if (r.bytes_total) {
			return `${formatBytes(r.bytes_done)} / ${formatBytes(r.bytes_total)}`;
		}
		return formatBytes(r.bytes_done);
	}

	const STATE_LABEL: Record<string, string> = {
		queued: 'Queued',
		downloading: 'Downloading',
		paused: 'Paused',
		failed: 'Failed'
	};
</script>

<div class="head">
	<h1>Downloads</h1>
	{#if completedByAnime.size > 0}
		<button class="danger" onclick={askDeleteAllCompleted}>Delete all completed</button>
	{/if}
</div>

{#if !isDesktop()}
	<p class="hint">Downloads need the desktop app: <code>npm run tauri dev</code>.</p>
{:else if loading}
	<div class="list">
		{#each Array(4), i (i)}
			<Skeleton height="64px" />
		{/each}
	</div>
{:else if error}
	<p class="error">{error}</p>
{:else}
	<h2>Active &amp; queued <span class="count">{active.length}</span></h2>
	{#if active.length === 0}
		<p class="empty">Nothing downloading. Queue episodes from a show page.</p>
	{:else}
		<div class="list">
			{#each active as r (r.id)}
				<div class="job">
					<div class="jmain">
						<button class="jtitle" onclick={() => goto(`/anime/${encodeURIComponent(r.anime_id)}`)}>
							{titleOf(r)} · Ep {r.episode_number}
						</button>
						<div class="jmeta">
							<span class="state s-{r.state}">{STATE_LABEL[r.state] ?? r.state}</span>
							{#if r.state === 'downloading'}
								<span class="num">{pct(r)}%</span>
								<span class="num">{progressDetail(r)}</span>
								{#if speeds[r.id] > 0}
									<span class="num">{formatBytes(speeds[r.id])}/s</span>
								{/if}
							{:else if r.state === 'paused'}
								<span class="num">{pct(r)}% · {progressDetail(r)}</span>
							{:else if r.state === 'failed' && r.error}
								<span class="err" title={r.error}>{r.error}</span>
							{/if}
							{#if r.quality}<span class="num q">{r.quality}</span>{/if}
						</div>
						<div class="track">
							<div class="fill" class:paused={r.state === 'paused'} style="width:{pct(r)}%"></div>
						</div>
					</div>
					<div class="jactions">
						{#if r.state === 'downloading' || r.state === 'queued'}
							<button onclick={() => act(() => pauseDownload(r.id))}>Pause</button>
						{:else if r.state === 'paused'}
							<button class="primary" onclick={() => act(() => resumeDownload(r.id))}>Resume</button>
						{:else if r.state === 'failed'}
							<button class="primary" onclick={() => act(() => resumeDownload(r.id))}>Retry</button>
						{/if}
						<button class="danger" onclick={() => act(() => cancelDownload(r.id))}>Cancel</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}

	<h2>Completed <span class="count">{[...completedByAnime.values()].reduce((n, l) => n + l.length, 0)}</span></h2>
	{#if completedByAnime.size === 0}
		<p class="empty">No completed downloads yet.</p>
	{:else}
		{#each [...completedByAnime.entries()] as [animeId, eps] (animeId)}
			<div class="group">
				<div class="ghead">
					<button class="gtitle" onclick={() => goto(`/anime/${encodeURIComponent(animeId)}`)}>
						{eps[0].title ?? animeId}
					</button>
					<span class="num gsize">
						{eps.length} episode{eps.length === 1 ? '' : 's'} · {formatBytes(
							storageOf.get(animeId) ?? 0
						)}
					</span>
					<button class="danger small" onclick={() => askDeleteAnime(animeId, eps[0].title, eps.length)}>
						Delete show
					</button>
				</div>
				<div class="geps">
					{#each eps as r (r.id)}
						<div class="gep">
							<button
								class="gplay"
								title="Play offline"
								onclick={() =>
									goto(
										`/watch/${encodeURIComponent(r.anime_id)}/${encodeURIComponent(r.episode_number)}?dub=${r.dub ? 1 : 0}`
									)}
							>
								<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" fill="currentColor" /></svg>
								Ep {r.episode_number}
							</button>
							<span class="num">{r.quality ?? ''}</span>
							<span class="num">{formatBytes(r.bytes_done)}</span>
							<button class="gdel" title="Delete episode" onclick={() => askDeleteEpisode(r)}>
								✕
							</button>
						</div>
					{/each}
				</div>
			</div>
		{/each}
	{/if}

	<div class="footer">
		<span>Total storage used</span>
		<span class="num total">{formatBytes(storage.total_bytes)}</span>
	</div>
{/if}

{#if confirm}
	<ConfirmDialog
		title={confirm.title}
		body={confirm.body}
		onconfirm={runConfirmed}
		oncancel={() => (confirm = null)}
	/>
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
	h2 {
		font: var(--text-title-md);
		color: var(--color-on-dark);
		margin: var(--space-lg) 0 var(--space-sm);
		display: flex;
		align-items: center;
		gap: var(--space-xs);
	}
	.count {
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
	.job {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		background: var(--color-surface-card);
		border-radius: var(--radius-lg);
		padding: var(--space-sm) var(--space-md);
	}
	.jmain {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.jtitle {
		background: none;
		border: none;
		padding: 0;
		text-align: left;
		color: var(--color-on-dark);
		font: var(--text-title-sm);
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.jtitle:hover {
		color: var(--color-primary);
	}
	.jmeta {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		flex-wrap: wrap;
	}
	.state {
		font: var(--text-caption);
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}
	.s-downloading {
		color: var(--color-primary);
	}
	.s-queued,
	.s-paused {
		color: var(--color-muted-strong);
	}
	.s-failed {
		color: var(--color-down);
	}
	.num {
		font: var(--text-num-sm);
		color: var(--color-muted-strong);
	}
	.num.q {
		color: var(--color-muted);
	}
	.err {
		font: var(--text-caption);
		color: var(--color-down);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		max-width: 420px;
	}
	.track {
		height: 4px;
		background: var(--color-surface-elevated);
		border-radius: var(--radius-pill);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		background: var(--color-primary);
		transition: width 0.3s ease;
	}
	.fill.paused {
		background: var(--color-muted);
	}
	.jactions {
		display: flex;
		gap: var(--space-xs);
		flex-shrink: 0;
	}
	.jactions button,
	.danger,
	.primary {
		background: var(--color-surface-elevated);
		color: var(--color-on-dark);
		border: none;
		border-radius: var(--radius-md);
		padding: 6px 14px;
		height: 30px;
		font: var(--text-button);
		cursor: pointer;
	}
	.jactions .primary {
		background: var(--color-primary);
		color: var(--color-on-primary);
	}
	.jactions .primary:hover {
		background: var(--color-primary-active);
	}
	.danger {
		background: transparent;
		border: 1px solid var(--color-hairline);
		color: var(--color-muted-strong);
	}
	.danger:hover {
		color: var(--color-down);
		border-color: var(--color-down);
	}
	.danger.small {
		height: 26px;
		padding: 3px 10px;
		font: var(--text-caption);
	}
	.group {
		background: var(--color-surface-card);
		border-radius: var(--radius-lg);
		padding: var(--space-sm) var(--space-md);
		margin-bottom: var(--space-xs);
	}
	.ghead {
		display: flex;
		align-items: center;
		gap: var(--space-md);
	}
	.gtitle {
		background: none;
		border: none;
		padding: 0;
		color: var(--color-on-dark);
		font: var(--text-title-sm);
		cursor: pointer;
		text-align: left;
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.gtitle:hover {
		color: var(--color-primary);
	}
	.gsize {
		flex-shrink: 0;
	}
	.geps {
		margin-top: var(--space-xs);
		display: flex;
		flex-direction: column;
	}
	.gep {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: 6px 0;
		border-top: 1px solid var(--color-hairline);
	}
	.gplay {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		background: none;
		border: none;
		padding: 0;
		color: var(--color-body);
		font: var(--text-num-sm);
		cursor: pointer;
		flex: 1;
		text-align: left;
	}
	.gplay svg {
		width: 14px;
		height: 14px;
		color: var(--color-muted);
	}
	.gplay:hover,
	.gplay:hover svg {
		color: var(--color-primary);
	}
	.gdel {
		background: none;
		border: none;
		color: var(--color-muted);
		cursor: pointer;
		font: var(--text-body-sm);
		padding: 2px 6px;
	}
	.gdel:hover {
		color: var(--color-down);
	}
	.footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		border-top: 1px solid var(--color-hairline);
		margin-top: var(--space-lg);
		padding-top: var(--space-md);
		color: var(--color-muted-strong);
		font: var(--text-body-md);
	}
	.total {
		font: var(--text-num-md);
		color: var(--color-on-dark);
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
	@media (max-width: 767px) {
		h1 {
			font: var(--text-title-lg);
		}
		/* Actions drop below the progress block instead of squeezing the title. */
		.job {
			flex-wrap: wrap;
		}
		.jmain {
			flex: 1 1 100%;
		}
		.jactions {
			margin-left: auto;
		}
		.ghead {
			flex-wrap: wrap;
		}
		.err {
			max-width: 100%;
		}
	}
</style>
