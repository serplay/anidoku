// Typed wrapper over Tauri IPC. Falls back to clear errors when the app is
// opened in a plain browser (e.g. `npm run dev` without Tauri), so the UI can
// still build and render.

import { invoke as tauriInvoke } from '@tauri-apps/api/core';

export interface AnimeSummary {
	provider_id: string;
	title: string;
	title_english: string | null;
	cover_url: string | null;
	available_episodes: number;
}

export type StreamKind = 'hls' | 'mp4';

export interface SubtitleTrack {
	label: string;
	lang: string;
	url: string;
}

export interface VideoSource {
	provider_name: string;
	quality: string;
	url: string;
	kind: StreamKind;
	referer: string | null;
	subtitles: SubtitleTrack[];
}

export interface WatchState {
	anime_id: string;
	episode_number: string;
	position_secs: number;
	duration_secs: number | null;
	updated_at: number;
}

function inTauri(): boolean {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
	if (!inTauri()) {
		throw new Error(
			`Command "${cmd}" is only available inside the AniDoku desktop app (Tauri). ` +
				`Run \`npm run tauri dev\`.`
		);
	}
	return tauriInvoke<T>(cmd, args);
}

export const isDesktop = inTauri;

export function searchAnime(query: string, dub = false): Promise<AnimeSummary[]> {
	return invoke('search_anime', { query, dub });
}

export function getEpisodes(showId: string, dub = false): Promise<string[]> {
	return invoke('get_episodes', { showId, dub });
}

export function getSources(showId: string, episode: string, dub = false): Promise<VideoSource[]> {
	return invoke('get_sources', { showId, episode, dub });
}

export function getWatchState(animeId: string, episode: string): Promise<WatchState | null> {
	return invoke('get_watch_state', { animeId, episode });
}

export function setWatchState(
	animeId: string,
	episode: string,
	positionSecs: number,
	durationSecs: number | null
): Promise<void> {
	return invoke('set_watch_state', { animeId, episode, positionSecs, durationSecs });
}

export function listWatchStates(animeId: string): Promise<WatchState[]> {
	return invoke('list_watch_states', { animeId });
}

export function convertSubtitles(content: string, formatHint: string): Promise<string> {
	return invoke('convert_subtitles', { content, formatHint });
}

// Base URL of the loopback media server (e.g. http://127.0.0.1:52123).
export function mediaBase(): Promise<string> {
	return invoke('media_base');
}

// Build a `stream://` URL for referer-gated cover images. The custom scheme is
// fine for images (fetched normally by the webview); it is NOT used for media,
// which needs Range/206 support — see `mediaUrl`.
export function streamUrl(upstream: string, referer: string | null): string {
	let s = `stream://localhost/?url=${encodeURIComponent(upstream)}`;
	if (referer) s += `&referer=${encodeURIComponent(referer)}`;
	return s;
}

// Build a loopback media-server URL for playback (MP4/HLS/subtitles). Routes
// through a real HTTP server that injects Referer, rewrites HLS playlists, and
// passes byte ranges through so macOS AVFoundation gets its 206 responses.
export function mediaUrl(
	base: string,
	upstream: string,
	referer: string | null,
	contentType?: string
): string {
	let s = `${base}/media?url=${encodeURIComponent(upstream)}`;
	if (referer) s += `&referer=${encodeURIComponent(referer)}`;
	// Hint for CDNs that serve MP4 as application/octet-stream (WebKit <video>
	// needs a video/* type). Ignored by the server when upstream is specific.
	if (contentType) s += `&ct=${encodeURIComponent(contentType)}`;
	return s;
}
