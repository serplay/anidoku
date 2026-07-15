// Tiny cross-route cache so the detail/player pages can show a title and cover
// without a redundant provider round-trip. Backed by an in-memory rune plus
// sessionStorage for reload survival.

import { setListEntry } from '$lib/api';
import type { AnimeSummary, AuthStatus, CatalogFilters, CatalogMedia } from '$lib/api';

const KEY = 'anidoku:anime-cache';

function load(): Record<string, AnimeSummary> {
	if (typeof sessionStorage === 'undefined') return {};
	try {
		return JSON.parse(sessionStorage.getItem(KEY) ?? '{}');
	} catch {
		return {};
	}
}

const cache = $state<Record<string, AnimeSummary>>(load());

export function rememberAnime(a: AnimeSummary) {
	cache[a.provider_id] = a;
	if (typeof sessionStorage !== 'undefined') {
		sessionStorage.setItem(KEY, JSON.stringify(cache));
	}
}

export function recallAnime(id: string): AnimeSummary | undefined {
	return cache[id];
}

// Last search query, so returning to the results page keeps context.
export const searchState = $state<{ query: string; results: AnimeSummary[]; dub: boolean }>({
	query: '',
	results: [],
	dub: false
});

// AniList catalog search state (reworked /search), cached in-memory so a
// return navigation restores the results without re-fetching. The active
// filters also live in the URL for back/forward, but the fetched result set
// only lives here.
export interface CatalogSearchState {
	filters: CatalogFilters;
	results: CatalogMedia[];
	hasNext: boolean;
	page: number;
	dub: boolean;
	ran: boolean;
}

export function emptyCatalogFilters(): CatalogFilters {
	return {
		query: '',
		page: 1,
		per_page: 30,
		genres: [],
		tags: [],
		season_year: null,
		status: [],
		format: [],
		include_adult: false
	};
}

export const catalogState = $state<CatalogSearchState>({
	filters: emptyCatalogFilters(),
	results: [],
	hasNext: false,
	page: 1,
	dub: false,
	ran: false
});

// Shared AniList auth status, kept in sync across the nav, settings and library.
export const authState = $state<AuthStatus>({
	viewer: null,
	logged_in: false,
	expired: false,
	has_client_id: false
});

export function setAuthStatus(s: AuthStatus) {
	authState.viewer = s.viewer;
	authState.logged_in = s.logged_in;
	authState.expired = s.expired;
	authState.has_client_id = s.has_client_id;
}

// Lightweight toast queue (DESIGN.md has no toast spec; we keep it minimal and
// on-brand: dark card, yellow accent for the "synced" confirmation).
export interface Toast {
	id: number;
	message: string;
	kind: 'info' | 'sync';
}

let toastSeq = 0;
export const toasts = $state<Toast[]>([]);

export function pushToast(message: string, kind: Toast['kind'] = 'info') {
	const id = ++toastSeq;
	toasts.push({ id, message, kind });
	setTimeout(() => {
		const i = toasts.findIndex((t) => t.id === id);
		if (i >= 0) toasts.splice(i, 1);
	}, 5000);
}

// Unread notification count for the nav bell badge, updated by the layout's
// notify:new / notify:read listeners and the inbox page.
export const notifyState = $state<{ unread: number }>({ unread: 0 });

// Whether an AniList media status means the show has not aired yet, so clicking
// it must not run provider resolution (there is nothing to stream).
export function isUnreleasedStatus(status: string | null | undefined): boolean {
	return status === 'NOT_YET_RELEASED';
}

// Handle a click on a not-yet-released show: never resolve a stream. Toast that
// it hasn't aired (with the year when known), and — when signed in — add it to
// the AniList Planning list so the click still does something useful.
export async function handleUnreleasedClick(
	anilistId: number,
	title: string,
	seasonYear: number | null
): Promise<void> {
	const airs = seasonYear ? ` — airs ${seasonYear}` : '';
	const name = title || 'This title';
	if (authState.logged_in) {
		try {
			await setListEntry(anilistId, 'PLANNING', 0, null);
			pushToast(`Not released yet${airs} · added ${name} to Planning`, 'sync');
			return;
		} catch {
			/* fall through to the plain info toast */
		}
	}
	pushToast(`Not released yet${airs}`);
}
