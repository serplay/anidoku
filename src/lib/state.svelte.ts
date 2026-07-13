// Tiny cross-route cache so the detail/player pages can show a title and cover
// without a redundant provider round-trip. Backed by an in-memory rune plus
// sessionStorage for reload survival.

import type { AnimeSummary } from '$lib/api';

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
