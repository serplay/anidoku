<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import Hls from 'hls.js';
	import {
		getSources,
		getEpisodes,
		getWatchState,
		setWatchState,
		convertSubtitles,
		getAnimeListState,
		isDesktop,
		isProviderRotated,
		refreshProviderConfig,
		mediaBase,
		mediaUrl,
		getOfflineInfo,
		offlineUrl,
		type VideoSource,
		type SubtitleTrack,
		type OfflineInfo
	} from '$lib/api';
	import { recallAnime, pushToast } from '$lib/state.svelte';
	import Button from '$lib/components/Button.svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';
	import ProviderOutage from '$lib/components/ProviderOutage.svelte';
	import { getCurrentWindow } from '@tauri-apps/api/window';

	const id = $derived(decodeURIComponent(page.params.id ?? ''));
	const ep = $derived(decodeURIComponent(page.params.ep ?? ''));
	const dub = $derived(page.url.searchParams.get('dub') === '1');
	const anime = $derived(recallAnime(id));

	let video = $state<HTMLVideoElement>();
	let sources = $state<VideoSource[]>([]);
	let selected = $state<VideoSource | null>(null);
	let subtitles = $state<{ label: string; lang: string; src: string }[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	// Distinct from `error`: the provider answered fine, this episode just has no
	// playable hosts (some shows are searchable but source-less). Rendered as a
	// calm empty-state, not a failure.
	let noSources = $state(false);
	// The provider rotated its scheme (backend tagged the error). Rendered as an
	// actionable "check for fix" state rather than a raw error string.
	let providerOutage = $state<string | null>(null);
	let checkingFix = $state(false);
	// Completed download for this episode, when playing offline.
	let offline = $state<OfflineInfo | null>(null);
	// User opted out of the offline copy for this episode ("Stream instead").
	let forceStream = $state(false);

	let hls: Hls | null = null;
	let lastSaved = 0;
	let resumeTo = 0;
	// Source URLs already tried this load, so a failed source falls forward to
	// the next candidate instead of dead-ending (dead streamsb/streamlare
	// embeds, region-locked ok.ru, deleted mp4upload files, …).
	let attempted = new Set<string>();
	// Whether the current source ever reached playback — gates auto-advance so a
	// transient mid-playback blip on a working source doesn't swap it out.
	let startedPlaying = false;
	let base = $state('');
	let episodes = $state<string[]>([]);
	let episodesFor = '';
	let autoNext = $state(localStorage.getItem('autoNext') !== '0');
	$effect(() => localStorage.setItem('autoNext', autoNext ? '1' : '0'));
	const nextEp = $derived.by(() => {
		const i = episodes.indexOf(ep);
		return i >= 0 && i + 1 < episodes.length ? episodes[i + 1] : null;
	});
	const prevEp = $derived.by(() => {
		const i = episodes.indexOf(ep);
		return i > 0 ? episodes[i - 1] : null;
	});
	// The media-server URL currently attached, so playback errors can name it.
	let currentUrl = $state<string | null>(null);

	// Distinct qualities for the selector.
	const qualities = $derived(sources.map((s) => s.quality));

	$effect(() => {
		// Re-run only when the route changes. `load` reads other reactive state
		// synchronously (the summary cache, forceStream) which the layout can
		// populate right after mount — without `untrack` that re-triggered a
		// second, redundant sources fetch on every watch-page open.
		const [showId, episode, isDub] = [id, ep, dub];
		untrack(() => void load(showId, episode, isDub));
		return () => teardown();
	});

	async function load(showId: string, episode: string, isDub: boolean) {
		loading = true;
		error = null;
		noSources = false;
		providerOutage = null;
		selected = null;
		// Ensure the AniList mapping exists before auto-progress needs it —
		// the detail page resolves it too, but a deep link / restart may not
		// have passed through there with the show summary in memory.
		if (isDesktop() && anime) {
			void getAnimeListState(
				showId,
				anime.title,
				anime.available_episodes || null,
				anime.anilist_id ?? null
			).catch(() => {});
		}
		// Episode list for next-episode navigation; best-effort.
		if (episodesFor !== showId) {
			episodesFor = showId;
			episodes = [];
			void getEpisodes(showId, isDub)
				.then((eps) => (episodes = eps))
				.catch(() => {});
		}
		try {
			// Prefer a completed download: instant, works offline. Falls back to
			// streaming when absent (or when the user picked "Stream instead").
			offline = null;
			if (isDesktop() && !forceStream) {
				const [info, ws, mb] = await Promise.all([
					getOfflineInfo(showId, episode).catch(() => null),
					getWatchState(showId, episode).catch(() => null),
					base ? Promise.resolve(base) : mediaBase()
				]);
				base = mb;
				resumeTo = ws?.position_secs ?? 0;
				if (info) {
					offline = info;
					subtitles = info.subtitles.map((t) => ({
						label: t.label,
						lang: t.lang,
						src: offlineUrl(base, info.dir, t.file)
					}));
					queueMicrotask(() =>
						attachMedia(info.kind, offlineUrl(base, info.dir, info.video))
					);
					loading = false;
					return;
				}
			}
			const [srcs, ws, mb] = await Promise.all([
				getSources(showId, episode, isDub),
				getWatchState(showId, episode).catch(() => null),
				base ? Promise.resolve(base) : mediaBase()
			]);
			base = mb;
			sources = srcs;
			resumeTo = ws?.position_secs ?? 0;
			attempted = new Set();
			if (srcs.length === 0) {
				noSources = true;
			} else {
				selectSource(srcs[0]);
			}
		} catch (e) {
			const msg = e instanceof Error ? e.message : String(e);
			if (isProviderRotated(msg)) providerOutage = msg;
			else error = msg;
		} finally {
			loading = false;
		}
	}

	// "Check for fix": force-fetch the published provider config and, if it
	// changed, reload sources. Otherwise tell the user plainly.
	async function checkForFix() {
		checkingFix = true;
		try {
			const r = await refreshProviderConfig();
			if (r.changed) {
				pushToast(`Provider fix applied (build ${r.build_id}) — retrying…`, 'sync');
				await load(id, ep, dub);
			} else {
				pushToast('No fix published yet — try again in a little while.', 'info');
			}
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e), 'info');
		} finally {
			checkingFix = false;
		}
	}

	function streamInstead() {
		forceStream = true;
		loading = true;
		void load(id, ep, dub);
	}

	function selectSource(s: VideoSource, manual = false) {
		// A manual pick is a fresh intent: restart the fallback chain so every
		// source is eligible again (including ones that failed earlier).
		if (manual) attempted = new Set();
		attempted.add(s.url);
		selected = s;
		subtitles = s.subtitles.map((t: SubtitleTrack) => ({
			label: t.label,
			lang: t.lang,
			src: mediaUrl(base, t.url, s.referer)
		}));
		// Wait for the <video> to exist, then attach.
		queueMicrotask(() =>
			attachMedia(s.kind, mediaUrl(base, s.url, s.referer, s.kind === 'mp4' ? 'video/mp4' : undefined))
		);
	}

	// Move to the next source not yet tried this load. Returns false when the
	// list is exhausted, so the caller can surface the real error.
	function advanceSource(): boolean {
		const next = sources.find((s) => !attempted.has(s.url));
		if (!next) return false;
		error = null;
		selectSource(next);
		return true;
	}

	function teardown() {
		if (hls) {
			hls.destroy();
			hls = null;
		}
	}

	function attachMedia(kind: 'hls' | 'mp4', url: string) {
		if (!video) return;
		teardown();
		currentUrl = url;
		startedPlaying = false;

		if (kind === 'hls') {
			// WKWebView (macOS/iOS) plays HLS natively; elsewhere use hls.js.
			if (video.canPlayType('application/vnd.apple.mpegurl')) {
				video.src = url;
			} else if (Hls.isSupported()) {
				hls = new Hls({ enableWorker: true });
				hls.loadSource(url);
				hls.attachMedia(video);
				hls.on(Hls.Events.ERROR, (_e, data) => {
					if (!data.fatal) return;
					// Fall forward to the next source before giving up.
					if (!startedPlaying && advanceSource()) return;
					error = `HLS error: ${data.type} / ${data.details}` +
						(data.response ? ` (HTTP ${data.response.code})` : '') +
						` — source: ${url}`;
				});
			} else {
				error = 'HLS is not supported in this webview.';
				return;
			}
		} else {
			video.src = url;
		}
		video.load();
	}

	// Surface the real <video> media error instead of the dead slashed-play icon.
	const MEDIA_ERR: Record<number, string> = {
		1: 'MEDIA_ERR_ABORTED — fetch aborted',
		2: 'MEDIA_ERR_NETWORK — network error while fetching media',
		3: 'MEDIA_ERR_DECODE — decode error (corrupt or unsupported codec)',
		4: 'MEDIA_ERR_SRC_NOT_SUPPORTED — source failed to load or is unsupported'
	};
	function onVideoError() {
		const e = video?.error;
		if (!e) return;
		// A source that never started playing is a dead embed / bad link — try
		// the next one silently before surfacing the failure.
		if (!startedPlaying && advanceSource()) return;
		const desc = MEDIA_ERR[e.code] ?? `code ${e.code}`;
		error = `Playback failed [${desc}]${e.message ? `: ${e.message}` : ''} — source: ${currentUrl}`;
	}

	function onplaying() {
		startedPlaying = true;
	}

	function onloaded() {
		if (video && resumeTo > 1 && resumeTo < video.duration - 5) {
			video.currentTime = resumeTo;
		}
	}

	// Throttle resume-point writes to once every 5s of playback.
	function ontimeupdate() {
		if (!video) return;
		const now = video.currentTime;
		if (Math.abs(now - lastSaved) >= 5) {
			lastSaved = now;
			void setWatchState(id, ep, now, isFinite(video.duration) ? video.duration : null);
		}
	}

	function onpause() {
		if (video) void setWatchState(id, ep, video.currentTime, isFinite(video.duration) ? video.duration : null);
	}

	function goTo(episode: string) {
		goto(`/watch/${encodeURIComponent(id)}/${encodeURIComponent(episode)}?dub=${dub ? 1 : 0}`);
	}

	function goNext() {
		if (nextEp) goTo(nextEp);
	}

	function onended() {
		// Record the episode as fully watched, then advance.
		if (video && isFinite(video.duration)) {
			void setWatchState(id, ep, video.duration, video.duration);
		}
		if (autoNext) goNext();
	}

	// Fullscreen: WKWebView often disables element fullscreen (no native button),
	// so fall back to Tauri window fullscreen + a "theater" overlay that fills it.
	let playerEl = $state<HTMLDivElement>();
	let theater = $state(false);
	// Window fullscreen state before we entered theater, so exiting restores
	// windowed mode (or keeps fullscreen if the user already had it).
	let wasWindowFullscreen = false;

	async function toggleFullscreen() {
		if (document.fullscreenElement) {
			await document.exitFullscreen();
			return;
		}
		const win = getCurrentWindow();
		if (theater) {
			theater = false;
			try {
				await win.setFullscreen(wasWindowFullscreen);
			} catch {
				/* capability missing — overlay off is still correct */
			}
			return;
		}
		if (!playerEl) return;
		try {
			await playerEl.requestFullscreen();
		} catch {
			// Element fullscreen unavailable (WKWebView): take the whole
			// screen via window fullscreen + the theater overlay.
			try {
				wasWindowFullscreen = await win.isFullscreen();
				await win.setFullscreen(true);
			} catch {
				wasWindowFullscreen = false;
			}
			theater = true;
		}
	}

	function seekBy(delta: number) {
		if (!video) return;
		let t = video.currentTime + delta;
		if (isFinite(video.duration)) t = Math.min(t, video.duration);
		video.currentTime = Math.max(0, t);
	}

	function onKeydown(e: KeyboardEvent) {
		const t = e.target as HTMLElement | null;
		if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
		if (e.key === 'f' || e.key === 'F') {
			e.preventDefault();
			void toggleFullscreen();
		} else if (e.key === ' ') {
			e.preventDefault();
			if (video) video.paused ? void video.play() : video.pause();
		} else if (e.key === 'ArrowRight') {
			e.preventDefault();
			seekBy(5);
		} else if (e.key === 'ArrowLeft') {
			e.preventDefault();
			seekBy(-5);
		} else if (e.key === 'Escape' && theater) {
			e.preventDefault();
			void toggleFullscreen();
		}
	}

	// External subtitle file: convert (SRT/VTT) in Rust, attach as a blob track.
	let extInput = $state<HTMLInputElement>();
	async function onExternalSub(e: Event) {
		const file = (e.target as HTMLInputElement).files?.[0];
		if (!file || !video) return;
		try {
			const text = await file.text();
			const ext = file.name.split('.').pop() ?? 'srt';
			const vtt = await convertSubtitles(text, ext);
			const blobUrl = URL.createObjectURL(new Blob([vtt], { type: 'text/vtt' }));
			const track = document.createElement('track');
			track.kind = 'subtitles';
			track.label = file.name;
			track.srclang = 'und';
			track.src = blobUrl;
			track.default = true;
			video.appendChild(track);
			track.track.mode = 'showing';
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<a class="back" href={`/anime/${encodeURIComponent(id)}?dub=${dub ? 1 : 0}`}>← Back to episodes</a>

<h1>{anime?.title_english ?? anime?.title ?? id} · Episode {ep}</h1>

{#if loading}
	<Skeleton aspect="16 / 9" radius="var(--radius-xl)" />
	<div class="skrow">
		<Skeleton width="90px" height="34px" />
		<Skeleton width="90px" height="34px" />
		<Skeleton width="120px" height="34px" />
	</div>
{:else if noSources && !selected}
	<div class="empty-state">
		<svg viewBox="0 0 24 24" aria-hidden="true">
			<path
				d="M4 5h16v11H4z M8 20h8 M12 16v4 M3 3l18 18"
				stroke="currentColor"
				stroke-width="1.8"
				fill="none"
				stroke-linecap="round"
				stroke-linejoin="round"
			/>
		</svg>
		<p class="empty-title">No sources available</p>
		<p class="empty-sub">
			This episode can be found but has no playable video hosts right now. Try another episode,
			switch sub/dub, or check back later.
		</p>
	</div>
{:else if providerOutage && !selected}
	<ProviderOutage
		detail={providerOutage}
		checking={checkingFix}
		oncheck={() => void checkForFix()}
		onretry={() => void load(id, ep, dub)}
	/>
{:else if error && !selected}
	<p class="error">{error}</p>
{:else}
	<div class="player" class:theater bind:this={playerEl}>
		<!-- svelte-ignore a11y_media_has_caption -->
		<video
			bind:this={video}
			controls
			autoplay
			playsinline
			onloadedmetadata={onloaded}
			ontimeupdate={ontimeupdate}
			onpause={onpause}
			onplaying={onplaying}
			{onended}
			onerror={onVideoError}
		>
			{#each subtitles as sub (sub.src)}
				<track kind="subtitles" label={sub.label} srclang={sub.lang} src={sub.src} />
			{/each}
		</video>
	</div>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	<div class="controls">
		{#if offline}
			<div class="group offline-note">
				<span class="badge">
					<svg viewBox="0 0 24 24" aria-hidden="true">
						<path
							d="M5 13l4 4L19 7"
							stroke="currentColor"
							stroke-width="2.5"
							fill="none"
							stroke-linecap="round"
							stroke-linejoin="round"
						/>
					</svg>
					Playing offline copy{offline.quality ? ` · ${offline.quality}` : ''}
				</span>
				<button class="streamlink" onclick={streamInstead}>Stream instead</button>
			</div>
		{/if}
		{#if !offline && qualities.length > 1}
			<div class="group">
				<span class="label">Quality</span>
				<div class="chips">
					{#each sources as s (s.url)}
						<button
							class="chip"
							class:active={selected?.url === s.url}
							onclick={() => selectSource(s, true)}
						>
							{s.quality}
							<span class="prov">{s.provider_name}</span>
						</button>
					{/each}
				</div>
			</div>
		{/if}

		<div class="group">
			<span class="label">Subtitles</span>
			<input
				bind:this={extInput}
				type="file"
				accept=".srt,.vtt"
				style="display:none"
				onchange={onExternalSub}
			/>
			<Button variant="secondary" onclick={() => extInput?.click()}>Load subtitle file…</Button>
		</div>

		<div class="group">
			<span class="label">Player</span>
			<div class="row">
				<Button variant="secondary" onclick={() => void toggleFullscreen()}>⛶ Fullscreen</Button>
				{#if prevEp}
					<Button variant="secondary" onclick={() => goTo(prevEp)}>← Ep {prevEp}</Button>
				{/if}
				{#if nextEp}
					<Button onclick={goNext}>Next episode ({nextEp}) →</Button>
				{/if}
				<label class="autonext">
					<input type="checkbox" bind:checked={autoNext} /> Auto-next
				</label>
			</div>
			<span class="hint">Space play/pause · F fullscreen · ← / → skip 5s</span>
		</div>
	</div>
{/if}

<style>
	.back {
		display: inline-block;
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		margin-bottom: var(--space-md);
	}
	h1 {
		font: var(--text-title-lg);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-lg);
	}
	.player {
		background: #000;
		border-radius: var(--radius-xl);
		overflow: hidden;
		aspect-ratio: 16 / 9;
	}
	video {
		width: 100%;
		height: 100%;
		display: block;
		background: #000;
	}
	.controls {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-xl);
		margin-top: var(--space-lg);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.label {
		font: var(--text-caption);
		color: var(--color-muted);
		text-transform: uppercase;
		letter-spacing: 0.5px;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-xs);
	}
	.chip {
		display: flex;
		align-items: center;
		gap: 6px;
		background: var(--color-surface-card);
		color: var(--color-body);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		padding: 8px 12px;
		font: var(--text-num-sm);
		cursor: pointer;
	}
	.chip.active {
		border-color: var(--color-primary);
		color: var(--color-primary);
	}
	.chip .prov {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.player.theater {
		position: fixed;
		inset: 0;
		z-index: 100;
		aspect-ratio: auto;
		border-radius: 0;
	}
	.hint {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.row {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
	}
	.autonext {
		display: flex;
		align-items: center;
		gap: var(--space-xxs);
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		cursor: pointer;
		user-select: none;
	}
	.autonext input {
		accent-color: var(--color-primary);
	}
	.offline-note {
		flex-direction: row;
		align-items: center;
		gap: var(--space-sm);
	}
	.badge {
		display: inline-flex;
		align-items: center;
		gap: var(--space-xs);
		font: var(--text-caption);
		color: var(--color-up);
		border: 1px solid var(--color-up);
		border-radius: var(--radius-pill);
		padding: 4px 10px;
	}
	.badge svg {
		width: 13px;
		height: 13px;
	}
	.streamlink {
		background: none;
		border: none;
		color: var(--color-muted-strong);
		font: var(--text-body-sm);
		text-decoration: underline;
		cursor: pointer;
	}
	.streamlink:hover {
		color: var(--color-on-dark);
	}
	.skrow {
		display: flex;
		gap: var(--space-sm);
		margin-top: var(--space-lg);
	}
	@media (max-width: 767px) {
		h1 {
			font: var(--text-title-md);
		}
		.controls {
			gap: var(--space-md);
		}
		.row {
			flex-wrap: wrap;
		}
		/* Keyboard shortcuts don't exist on touch. */
		.hint {
			display: none;
		}
	}
	.error {
		color: var(--color-down);
		margin-top: var(--space-md);
	}
	.empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		text-align: center;
		gap: var(--space-sm);
		padding: var(--space-xxl) var(--space-lg);
		background: var(--color-surface-card);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-xl);
		aspect-ratio: 16 / 9;
	}
	.empty-state svg {
		width: 48px;
		height: 48px;
		color: var(--color-muted);
		opacity: 0.7;
	}
	.empty-title {
		font: var(--text-title-md);
	}
	.empty-sub {
		color: var(--color-muted);
		font: var(--text-body-md);
		max-width: 42ch;
	}
</style>
