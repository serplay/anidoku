import type { Page } from '@playwright/test';

/**
 * Map of Tauri command name -> canned response. Anything not listed resolves to
 * a benign default (see `defaults` below) or `null`, so a page never crashes on
 * an un-mocked command — tests only specify the commands they care about.
 */
export type CommandResponses = Record<string, unknown>;

/** Make `invoke(cmd)` reject with this message (Tauri rejects with a string). */
export function reject(message: string): { __reject: string } {
	return { __reject: message };
}

/** Per-call responses: the Nth `invoke(cmd)` gets `sequence[N]` (the last
 * entry repeats). Entries may be `reject(...)`. */
export function sequence(...responses: unknown[]): { __sequence: unknown[] } {
	return { __sequence: responses };
}

/**
 * Install a fake `window.__TAURI_INTERNALS__` before the app's scripts run, so
 * `isDesktop()` reports true and every `invoke()` / event subscription resolves
 * against canned data instead of a real Rust backend.
 *
 * This runs as an init script (before any page JS), so it is in place for the
 * layout's on-mount command calls and event subscriptions.
 */
export async function mockTauri(page: Page, commands: CommandResponses = {}): Promise<void> {
	await page.addInitScript((cmds: CommandResponses) => {
		let nextId = 0;
		const callbacks = new Map<number, (payload: unknown) => void>();

		// Sensible defaults for commands most pages touch on mount. Overridable
		// per-test via `commands`.
		const defaults: CommandResponses = {
			media_base: 'http://mock.localhost',
			anilist_status: { logged_in: false },
			unread_notifications: 0,
			get_settings: {
				client_id: null,
				redirect_url: 'http://127.0.0.1:8737/callback',
				sources: [
					{
						source: 'allanime',
						display_name: 'AllAnime',
						build_id: '174',
						config_source: 'baked',
						config_url: '',
						enabled: true
					}
				]
			},
			set_source_enabled: null,
			set_source_order: null,
			set_preferred_source: null,
			refresh_provider_config: { changed: false, build_id: '166', config_source: 'baked' },
			get_watch_state: null,
			list_watch_states: [],
			get_offline_info: null,
			get_anime_list_state: null,
			get_episodes: [],
			get_sources: [],
			search_anime: [],
			get_home_cached: null,
			get_continue_watching: [],
			get_library: [],
			get_notifications: [],
			list_downloads: [],
			// Search / catalog page. NB: a non-empty tags list is required — the
			// page re-fetches while `tags.length === 0`, so an empty array would
			// spin (the real backend always returns a populated list).
			get_media_tags: [{ name: 'Action', category: 'Theme' }],
			get_upcoming: [],
			search_catalog: { media: [], has_next_page: false, total: 0 }
		};

		// Record IPC calls so tests can assert what the frontend requested.
		const calls: Array<{ cmd: string; args: unknown }> = [];
		(window as unknown as { __IPC_CALLS__: typeof calls }).__IPC_CALLS__ = calls;
		const seen = new Map<string, number>();

		// Unwrap the `reject(...)` / `sequence(...)` markers. Throws for a
		// rejection so `invoke()` turns it into a rejected promise.
		function materialise(cmd: string, value: unknown): unknown {
			if (value && typeof value === 'object' && '__sequence' in value) {
				const seq = (value as { __sequence: unknown[] }).__sequence;
				const n = seen.get(cmd) ?? 0;
				seen.set(cmd, n + 1);
				return materialise(cmd, seq[Math.min(n, seq.length - 1)]);
			}
			if (value && typeof value === 'object' && '__reject' in value) {
				throw (value as { __reject: string }).__reject;
			}
			return value;
		}

		function resolveInvoke(cmd: string, args: unknown): unknown {
			// Event plugin: pretend to subscribe/unsubscribe. `listen` expects a
			// numeric handler id; `unlisten` a void.
			if (cmd === 'plugin:event|listen') return ++nextId;
			if (cmd === 'plugin:event|unlisten') return undefined;
			// Any other plugin (window, notification, opener, …): benign no-op.
			if (cmd.startsWith('plugin:')) return null;

			calls.push({ cmd, args });
			if (Object.prototype.hasOwnProperty.call(cmds, cmd)) return materialise(cmd, cmds[cmd]);
			if (Object.prototype.hasOwnProperty.call(defaults, cmd)) return defaults[cmd];
			return null;
		}

		(
			window as unknown as { __TAURI_INTERNALS__: Record<string, unknown> }
		).__TAURI_INTERNALS__ = {
			transformCallback(cb: (payload: unknown) => void) {
				const id = ++nextId;
				callbacks.set(id, cb);
				return id;
			},
			unregisterCallback(id: number) {
				callbacks.delete(id);
			},
			invoke(cmd: string, args: unknown) {
				try {
					return Promise.resolve(resolveInvoke(cmd, args));
				} catch (e) {
					return Promise.reject(e);
				}
			}
		};
	}, commands);
}

/** A ready-made `AnimeSummary` for search / detail fixtures. */
export function animeFixture(over: Partial<Record<string, unknown>> = {}) {
	return {
		provider_id: 'show-one-piece',
		title: 'One Piece',
		title_english: 'One Piece',
		cover_url: null,
		available_episodes: 1000,
		anilist_id: 21,
		...over
	};
}

/** A ready-made `VideoSource`. `url` is intentionally unreachable so the media
 * element errors — used to drive the player's auto-advance path. */
export function sourceFixture(over: Partial<Record<string, unknown>> = {}) {
	return {
		source: 'allanime',
		provider_name: 'Default',
		quality: '1080',
		url: 'http://127.0.0.1:9/nonexistent.mp4',
		kind: 'mp4',
		referer: null,
		subtitles: [],
		...over
	};
}
