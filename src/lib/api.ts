// Typed wrapper over Tauri IPC. Falls back to clear errors when the app is
// opened in a plain browser (e.g. `npm run dev` without Tauri), so the UI can
// still build and render.

import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface AnimeSummary {
	provider_id: string;
	title: string;
	title_english: string | null;
	cover_url: string | null;
	available_episodes: number;
	anilist_id: number | null;
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

// ---------------------------------------------------------------------------
// AniList sync (M2)
// ---------------------------------------------------------------------------

export type MediaListStatus =
	| 'CURRENT'
	| 'PLANNING'
	| 'COMPLETED'
	| 'DROPPED'
	| 'PAUSED'
	| 'REPEATING';

export const STATUS_ORDER: MediaListStatus[] = [
	'CURRENT',
	'PLANNING',
	'COMPLETED',
	'PAUSED',
	'DROPPED',
	'REPEATING'
];

export const STATUS_LABEL: Record<MediaListStatus, string> = {
	CURRENT: 'Watching',
	PLANNING: 'Planning',
	COMPLETED: 'Completed',
	PAUSED: 'Paused',
	DROPPED: 'Dropped',
	REPEATING: 'Rewatching'
};

export interface Viewer {
	id: number;
	name: string;
	avatar_url: string | null;
}

export interface ListEntry {
	anilist_id: number;
	status: MediaListStatus;
	progress: number;
	score: number | null;
	local_updated_at: number;
	remote_updated_at: number | null;
	dirty: boolean;
}

export interface LibraryItem {
	anilist_id: number;
	status: MediaListStatus;
	progress: number;
	score: number | null;
	dirty: boolean;
	title_romaji: string | null;
	title_english: string | null;
	cover_url: string | null;
	episode_count: number | null;
	provider_id: string | null;
}

export interface MediaInfo {
	anilist_id: number;
	title_romaji: string | null;
	title_english: string | null;
	title_native: string | null;
	synonyms: string[];
	cover_url: string | null;
	episode_count: number | null;
	format: string | null;
}

export interface Settings {
	client_id: string | null;
	redirect_url: string;
}

export interface AuthStatus {
	viewer: Viewer | null;
	logged_in: boolean;
	expired: boolean;
	has_client_id: boolean;
}

export interface AnimeListState {
	anilist_id: number | null;
	entry: ListEntry | null;
	episode_count: number | null;
}

export function getSettings(): Promise<Settings> {
	return invoke('get_settings');
}

export function setClientId(clientId: string | null): Promise<void> {
	return invoke('set_client_id', { clientId });
}

export function anilistStatus(): Promise<AuthStatus> {
	return invoke('anilist_status');
}

export function anilistLogin(): Promise<Viewer> {
	return invoke('anilist_login');
}

export function anilistLogout(): Promise<void> {
	return invoke('anilist_logout');
}

export function anilistSyncNow(): Promise<void> {
	return invoke('anilist_sync_now');
}

export function getLibrary(): Promise<LibraryItem[]> {
	return invoke('get_library');
}

export function setListEntry(
	anilistId: number,
	status: MediaListStatus,
	progress: number,
	score: number | null
): Promise<ListEntry> {
	return invoke('set_list_entry', { anilistId, status, progress, score });
}

export function getAnimeListState(
	providerId: string,
	title: string,
	episodes: number | null,
	anilistHint: number | null
): Promise<AnimeListState> {
	return invoke('get_anime_list_state', {
		providerId,
		title,
		episodes,
		anilistHint
	});
}

export function searchAnilist(query: string): Promise<MediaInfo[]> {
	return invoke('search_anilist', { query });
}

export function setAnimeMapping(providerId: string, anilistId: number): Promise<void> {
	return invoke('set_anime_mapping', { providerId, anilistId });
}

// AniList cover images are served from s4.anilist.co without referer gating, so
// they can be used directly (no stream:// proxy needed).

export interface RemoteOverwrite {
	anilist_id: number;
	title: string | null;
}

// Subscribe to a Tauri event; no-ops (returns a noop unlisten) in a browser.
export function onEvent<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
	if (!inTauri()) return Promise.resolve(() => {});
	return listen<T>(event, (e) => handler(e.payload));
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
