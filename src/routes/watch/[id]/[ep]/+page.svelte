<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import Hls from 'hls.js';
	import {
		getSources,
		getWatchState,
		setWatchState,
		convertSubtitles,
		streamUrl,
		type VideoSource,
		type SubtitleTrack
	} from '$lib/api';
	import { recallAnime } from '$lib/state.svelte';
	import Button from '$lib/components/Button.svelte';

	const id = $derived(decodeURIComponent(page.params.id ?? ''));
	const ep = $derived(decodeURIComponent(page.params.ep ?? ''));
	const dub = $derived(page.url.searchParams.get('dub') === '1');
	const anime = $derived(recallAnime(id));

	let video = $state<HTMLVideoElement>();
	let sources = $state<VideoSource[]>([]);
	let selected = $state<VideoSource | null>(null);
	let subtitles = $state<SubtitleTrack[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let hls: Hls | null = null;
	let lastSaved = 0;
	let resumeTo = 0;

	// Distinct qualities for the selector.
	const qualities = $derived(sources.map((s) => s.quality));

	$effect(() => {
		void load(id, ep, dub);
		return () => teardown();
	});

	async function load(showId: string, episode: string, isDub: boolean) {
		loading = true;
		error = null;
		selected = null;
		try {
			const [srcs, ws] = await Promise.all([
				getSources(showId, episode, isDub),
				getWatchState(showId, episode).catch(() => null)
			]);
			sources = srcs;
			resumeTo = ws?.position_secs ?? 0;
			if (srcs.length === 0) {
				error = 'No playable sources found for this episode.';
			} else {
				selectSource(srcs[0]);
			}
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	}

	function selectSource(s: VideoSource) {
		selected = s;
		subtitles = s.subtitles;
		// Wait for the <video> to exist, then attach.
		queueMicrotask(() => attach(s));
	}

	function teardown() {
		if (hls) {
			hls.destroy();
			hls = null;
		}
	}

	function attach(s: VideoSource) {
		if (!video) return;
		teardown();
		const url = streamUrl(s.url, s.referer);

		if (s.kind === 'hls') {
			// WKWebView (macOS/iOS) plays HLS natively; elsewhere use hls.js.
			if (video.canPlayType('application/vnd.apple.mpegurl')) {
				video.src = url;
			} else if (Hls.isSupported()) {
				hls = new Hls({ enableWorker: true });
				hls.loadSource(url);
				hls.attachMedia(video);
				hls.on(Hls.Events.ERROR, (_e, data) => {
					if (data.fatal) error = `Playback error: ${data.details}`;
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

<a class="back" href={`/anime/${encodeURIComponent(id)}?dub=${dub ? 1 : 0}`}>← Back to episodes</a>

<h1>{anime?.title_english ?? anime?.title ?? id} · Episode {ep}</h1>

{#if loading}
	<p class="status">Resolving sources…</p>
{:else if error && !selected}
	<p class="error">{error}</p>
{:else}
	<div class="player">
		<!-- svelte-ignore a11y_media_has_caption -->
		<video
			bind:this={video}
			controls
			autoplay
			playsinline
			onloadedmetadata={onloaded}
			ontimeupdate={ontimeupdate}
			onpause={onpause}
		>
			{#each subtitles as sub (sub.url)}
				<track
					kind="subtitles"
					label={sub.label}
					srclang={sub.lang}
					src={streamUrl(sub.url, selected?.referer ?? null)}
				/>
			{/each}
		</video>
	</div>

	{#if error}
		<p class="error">{error}</p>
	{/if}

	<div class="controls">
		{#if qualities.length > 1}
			<div class="group">
				<span class="label">Quality</span>
				<div class="chips">
					{#each sources as s (s.url)}
						<button
							class="chip"
							class:active={selected?.url === s.url}
							onclick={() => selectSource(s)}
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
	.status {
		color: var(--color-muted);
	}
	.error {
		color: var(--color-down);
		margin-top: var(--space-md);
	}
</style>
