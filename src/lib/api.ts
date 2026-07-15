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

// ---------------------------------------------------------------------------
// AniList catalog search + filters (reworked /search)
// ---------------------------------------------------------------------------

export interface CatalogMedia {
	anilist_id: number;
	title_romaji: string | null;
	title_english: string | null;
	cover_url: string | null;
	format: string | null;
	episode_count: number | null;
	average_score: number | null;
	season_year: number | null;
	status: string | null;
	genres: string[];
	next_episode: number | null;
	is_adult: boolean;
}

export interface CatalogPage {
	media: CatalogMedia[];
	has_next_page: boolean;
	current_page: number;
}

export interface MediaTag {
	name: string;
	category: string | null;
	is_adult: boolean;
}

// Filters sent to the `search_catalog` command. Snake_case keys match the Rust
// `CatalogSearch` struct (serde). Empty arrays / null mean "no constraint".
export interface CatalogFilters {
	query: string;
	page: number;
	per_page: number;
	genres: string[];
	tags: string[];
	season_year: number | null;
	status: string[];
	format: string[];
	include_adult: boolean;
}

// AniList's fixed genre list (from their docs). "Hentai" opts into adult results.
export const GENRES = [
	'Action',
	'Adventure',
	'Comedy',
	'Drama',
	'Ecchi',
	'Fantasy',
	'Hentai',
	'Horror',
	'Mahou Shoujo',
	'Mecha',
	'Music',
	'Mystery',
	'Psychological',
	'Romance',
	'Sci-Fi',
	'Slice of Life',
	'Sports',
	'Supernatural',
	'Thriller'
];

// MediaStatus filter options (single-select in the UI).
export const STATUS_OPTIONS: { value: string; label: string }[] = [
	{ value: 'RELEASING', label: 'Releasing' },
	{ value: 'FINISHED', label: 'Finished' },
	{ value: 'CANCELLED', label: 'Cancelled' },
	{ value: 'HIATUS', label: 'Hiatus' }
];

// MediaFormat filter options.
export const FORMAT_OPTIONS: { value: string; label: string }[] = [
	{ value: 'TV', label: 'TV' },
	{ value: 'TV_SHORT', label: 'TV Short' },
	{ value: 'MOVIE', label: 'Movie' },
	{ value: 'SPECIAL', label: 'Special' },
	{ value: 'OVA', label: 'OVA' },
	{ value: 'ONA', label: 'ONA' },
	{ value: 'MUSIC', label: 'Music' }
];

export function searchCatalog(filters: CatalogFilters): Promise<CatalogPage> {
	return invoke('search_catalog', { filters });
}

export function getMediaTags(): Promise<MediaTag[]> {
	return invoke('get_media_tags');
}

// ---------------------------------------------------------------------------
// Card meta chips (format / year / episodes / score)
// ---------------------------------------------------------------------------

// The compact meta a card renders as small chips under its title. Every field
// is optional so provider-only cards (Continue Watching) simply render none.
export interface CardMeta {
	format?: string | null;
	year?: number | null;
	episodes?: number | null;
	/** Episodes aired so far while releasing (nextAiringEpisode.episode - 1). */
	aired?: number | null;
	releasing?: boolean;
	/** Weighted mean score, 0–100. */
	score?: number | null;
}

// Pretty label for an AniList MediaFormat enum value.
export function formatLabel(format: string | null | undefined): string | null {
	if (!format) return null;
	const map: Record<string, string> = {
		TV: 'TV',
		TV_SHORT: 'TV Short',
		MOVIE: 'Movie',
		SPECIAL: 'Special',
		OVA: 'OVA',
		ONA: 'ONA',
		MUSIC: 'Music'
	};
	return map[format] ?? format;
}

export function homeMediaMeta(m: HomeMedia): CardMeta {
	const releasing = m.status === 'RELEASING';
	return {
		format: m.format,
		year: m.season_year,
		episodes: m.episode_count,
		aired: releasing && m.next_episode ? m.next_episode - 1 : null,
		releasing,
		score: m.average_score
	};
}

export function catalogMediaMeta(m: CatalogMedia): CardMeta {
	const releasing = m.status === 'RELEASING';
	return {
		format: m.format,
		year: m.season_year,
		episodes: m.episode_count,
		aired: releasing && m.next_episode ? m.next_episode - 1 : null,
		releasing,
		score: m.average_score
	};
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

// ---------------------------------------------------------------------------
// Downloads (M3)
// ---------------------------------------------------------------------------

export type DownloadState = 'queued' | 'downloading' | 'paused' | 'done' | 'failed';

export interface DownloadRow {
	id: number;
	anime_id: string;
	episode_number: string;
	state: DownloadState;
	quality: string | null;
	dub: boolean;
	kind: StreamKind | null;
	bytes_total: number | null;
	bytes_done: number;
	segments_done: number;
	segments_total: number | null;
	dir_path: string | null;
	error: string | null;
	created_at: number;
	updated_at: number;
	title: string | null;
}

export interface AnimeStorage {
	anime_id: string;
	title: string | null;
	cover_url: string | null;
	episodes: number;
	bytes: number;
}

export interface DownloadStorage {
	per_anime: AnimeStorage[];
	total_bytes: number;
}

export interface OfflineSubtitle {
	label: string;
	lang: string;
	file: string;
}

export interface OfflineInfo {
	dir: string;
	kind: StreamKind;
	quality: string;
	video: string;
	subtitles: OfflineSubtitle[];
}

// Event payloads for download:progress / download:state.
export interface DownloadProgressEvent {
	id: number;
	anime_id: string;
	episode_number: string;
	bytes_done: number;
	bytes_total: number | null;
	segments_done: number;
	segments_total: number | null;
	speed_bps: number;
}

export interface DownloadStateEvent {
	id: number;
	anime_id: string;
	episode_number: string;
	state: DownloadState;
	error: string | null;
	removed: boolean;
}

export function enqueueDownloads(
	animeId: string,
	episodes: string[],
	quality: string | null,
	dub: boolean
): Promise<number> {
	return invoke('enqueue_downloads', { animeId, episodes, quality, dub });
}

export function listDownloads(): Promise<DownloadRow[]> {
	return invoke('list_downloads');
}

export function downloadsForAnime(animeId: string): Promise<DownloadRow[]> {
	return invoke('downloads_for_anime', { animeId });
}

export function pauseDownload(id: number): Promise<void> {
	return invoke('pause_download', { id });
}

export function resumeDownload(id: number): Promise<void> {
	return invoke('resume_download', { id });
}

// Cancels an active/queued download or deletes a completed/failed one
// (row + files).
export function cancelDownload(id: number): Promise<void> {
	return invoke('cancel_download', { id });
}

export function deleteAnimeDownloads(animeId: string): Promise<number> {
	return invoke('delete_anime_downloads', { animeId });
}

export function deleteCompletedDownloads(): Promise<number> {
	return invoke('delete_completed_downloads');
}

export function downloadStorage(): Promise<DownloadStorage> {
	return invoke('download_storage');
}

export function getOfflineInfo(animeId: string, episode: string): Promise<OfflineInfo | null> {
	return invoke('get_offline_info', { animeId, episode });
}

// URL of a downloaded file served by the media server's /dl route. `dir` is
// the episode dir relative to the downloads root (already fs-sanitized).
export function offlineUrl(base: string, dir: string, file: string): string {
	return `${base}/dl/${dir}/${file}`;
}

// Human-readable byte size ("1.4 GB").
export function formatBytes(n: number): string {
	if (!isFinite(n) || n <= 0) return '0 B';
	const units = ['B', 'KB', 'MB', 'GB', 'TB'];
	let i = 0;
	let v = n;
	while (v >= 1024 && i < units.length - 1) {
		v /= 1024;
		i++;
	}
	return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

// Fraction complete (0..1) for a download row / progress event.
export function downloadFraction(d: {
	bytes_done: number;
	bytes_total: number | null;
	segments_done: number;
	segments_total: number | null;
}): number {
	if (d.bytes_total && d.bytes_total > 0) return Math.min(1, d.bytes_done / d.bytes_total);
	if (d.segments_total && d.segments_total > 0)
		return Math.min(1, d.segments_done / d.segments_total);
	return 0;
}

// ---------------------------------------------------------------------------
// Home page + airing tracker & notification inbox (M3.5)
// ---------------------------------------------------------------------------

export interface HomeMedia {
	anilist_id: number;
	title_romaji: string | null;
	title_english: string | null;
	cover_url: string | null;
	episode_count: number | null;
	format: string | null;
	status: string | null;
	next_episode: number | null;
	airing_at: number | null;
	season_year: number | null;
	average_score: number | null;
}

export interface HomeSections {
	trending: HomeMedia[];
	season: HomeMedia[];
	next_season: HomeMedia[];
}

export interface HomePayload {
	sections: HomeSections;
	fetched_at: number;
	fresh: boolean;
}

export interface ContinueWatchingItem {
	anilist_id: number;
	title_romaji: string | null;
	title_english: string | null;
	cover_url: string | null;
	episode_count: number | null;
	progress: number;
	next_episode: string | null;
	provider_id: string | null;
	last_watched_at: number;
}

export interface Notification {
	id: number;
	anilist_id: number;
	episode: number;
	airing_at: number | null;
	kind: string;
	created_at: number;
	read: boolean;
	title_romaji: string | null;
	title_english: string | null;
	cover_url: string | null;
	provider_id: string | null;
}

// Payload of the `notify:new` event.
export interface NotifyNew {
	anilist_id: number;
	episode: number;
	title: string | null;
	unread: number;
}

export function getHomeCached(): Promise<HomePayload | null> {
	return invoke('get_home_cached');
}

export function refreshHome(): Promise<HomePayload> {
	return invoke('refresh_home');
}

export function getContinueWatching(): Promise<ContinueWatchingItem[]> {
	return invoke('get_continue_watching');
}

// Resolve an AniList id to a provider show (existing mapping, else provider
// search matched by carried aniListId / title). `null` → fall back to /search.
export function resolveProviderForAnilist(
	anilistId: number,
	title: string,
	episodes: number | null
): Promise<AnimeSummary | null> {
	return invoke('resolve_provider_for_anilist', { anilistId, title, episodes });
}

export function getNotifications(): Promise<Notification[]> {
	return invoke('get_notifications');
}

export function getUpcoming(): Promise<Notification[]> {
	return invoke('get_upcoming');
}

export function unreadNotifications(): Promise<number> {
	return invoke('unread_notifications');
}

export function markNotificationsRead(): Promise<void> {
	return invoke('mark_notifications_read');
}

export function clearNotifications(): Promise<void> {
	return invoke('clear_notifications');
}

export function airingRefreshNow(): Promise<void> {
	return invoke('airing_refresh_now');
}

export function getNotifyPlanning(): Promise<boolean> {
	return invoke('get_notify_planning');
}

export function setNotifyPlanning(enabled: boolean): Promise<void> {
	return invoke('set_notify_planning', { enabled });
}

// "Ep N in Xd" caption for airing shows: compact relative time until `at`.
export function untilCaption(at: number, nowSecs = Math.floor(Date.now() / 1000)): string {
	const d = at - nowSecs;
	if (d <= 0) return 'now';
	if (d < 3600) return `${Math.max(1, Math.round(d / 60))}m`;
	if (d < 86400) return `${Math.round(d / 3600)}h`;
	return `${Math.round(d / 86400)}d`;
}

// "Ep N airs <weekday, local date/time>" for the inbox Upcoming list.
export function airsAtLabel(at: number): string {
	return new Date(at * 1000).toLocaleString(undefined, {
		weekday: 'short',
		month: 'short',
		day: 'numeric',
		hour: 'numeric',
		minute: '2-digit'
	});
}

export function displayTitle(x: {
	title_english: string | null;
	title_romaji: string | null;
	anilist_id: number;
}): string {
	return x.title_english ?? x.title_romaji ?? `AniList #${x.anilist_id}`;
}
