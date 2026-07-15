<script module lang="ts">
	// Show the boot splash once per app launch. This module-level flag survives
	// client-side navigation (the layout stays mounted, but this also guards
	// against any remount), so the splash never re-appears after startup.
	let splashDone = false;
</script>

<script lang="ts">
	import '@fontsource/inter/400.css';
	import '@fontsource/inter/500.css';
	import '@fontsource/inter/600.css';
	import '@fontsource/inter/700.css';
	import '@fontsource/ibm-plex-sans/500.css';
	import '$lib/styles/tokens.css';
	import favicon from '$lib/assets/favicon.svg';
	import splashLogo from '$lib/assets/splash-logo.png';
	import { page } from '$app/state';
	import {
		anilistStatus,
		anilistSyncNow,
		unreadNotifications,
		onEvent,
		isDesktop,
		type RemoteOverwrite,
		type DownloadStateEvent,
		type NotifyNew
	} from '$lib/api';
	import { authState, setAuthStatus, toasts, pushToast, notifyState } from '$lib/state.svelte';

	let { children } = $props();

	// Boot splash: full-viewport overlay on first launch, then fade + unmount.
	// Home renders instantly from cache, so a fixed hold is simpler than
	// readiness plumbing. Respects prefers-reduced-motion (skip the logo
	// animation, shorten the hold + fade).
	let showSplash = $state(!splashDone);
	let splashFading = $state(false);

	$effect(() => {
		if (splashDone) return;
		const reduce =
			typeof window !== 'undefined' &&
			window.matchMedia('(prefers-reduced-motion: reduce)').matches;
		const hold = reduce ? 300 : 1200;
		const fade = reduce ? 150 : 300;
		const t1 = setTimeout(() => (splashFading = true), hold);
		const t2 = setTimeout(() => {
			showSplash = false;
			splashDone = true;
		}, hold + fade);
		return () => {
			clearTimeout(t1);
			clearTimeout(t2);
		};
	});

	async function refreshAuth() {
		if (!isDesktop()) return;
		try {
			setAuthStatus(await anilistStatus());
		} catch {
			/* browser / not ready */
		}
	}

	type PushOutcome = {
		anilist_id: number;
		progress: number;
		status: string;
		completed: boolean;
	};
	// Toast the unmapped warning once per show per session, not on every
	// throttled watch-state write past the threshold.
	const unmappedSeen = new Set<string>();

	async function refreshUnread() {
		if (!isDesktop()) return;
		try {
			notifyState.unread = await unreadNotifications();
		} catch {
			/* browser / not ready */
		}
	}

	$effect(() => {
		refreshAuth();
		refreshUnread();
		const unlisteners = [
			// Airing tracker: badge + toast when an episode releases.
			onEvent<NotifyNew>('notify:new', (p) => {
				notifyState.unread = p.unread;
				pushToast(`New episode: Ep ${p.episode} of ${p.title ?? 'a tracked show'}`, 'sync');
			}),
			onEvent('notify:read', () => refreshUnread()),
			onEvent<RemoteOverwrite>('sync:remote-overwrote', (p) => {
				pushToast(`Synced from AniList: ${p.title ?? 'an entry'}`, 'sync');
			}),
			onEvent('sync:updated', () => refreshAuth()),
			onEvent('sync:auth-expired', () => {
				refreshAuth();
				pushToast('AniList session expired — sign in again in Settings.', 'info');
			}),
			onEvent<PushOutcome>('sync:pushed', (p) => {
				pushToast(
					p.completed ? 'AniList: marked Completed 🎉' : `AniList: progress → Ep ${p.progress}`,
					'sync'
				);
			}),
			onEvent<PushOutcome>('sync:queued', (p) => {
				pushToast(`AniList: Ep ${p.progress} queued — syncs when connected`, 'info');
			}),
			onEvent<string>('sync:unmapped', (providerId) => {
				if (unmappedSeen.has(providerId)) return;
				unmappedSeen.add(providerId);
				pushToast('Show not linked to AniList — progress kept locally only', 'info');
			}),
			// Download completion/failure toasts, wherever the user is.
			onEvent<DownloadStateEvent>('download:state', (p) => {
				if (p.removed) return;
				if (p.state === 'done') {
					pushToast(`Download complete: Episode ${p.episode_number}`, 'sync');
				} else if (p.state === 'failed') {
					pushToast(
						`Download failed: Episode ${p.episode_number}${p.error ? ` — ${p.error}` : ''}`,
						'info'
					);
				}
			})
		];
		// Drain the queue the moment connectivity returns instead of waiting
		// for the periodic worker.
		const onOnline = () => {
			if (isDesktop()) void anilistSyncNow().catch(() => {});
		};
		window.addEventListener('online', onOnline);
		return () => {
			window.removeEventListener('online', onOnline);
			for (const u of unlisteners) u.then((fn) => fn());
		};
	});

	const nav = [
		{ href: '/', label: 'Home' },
		{ href: '/search', label: 'Search' },
		{ href: '/library', label: 'Library' },
		{ href: '/downloads', label: 'Downloads' },
		{ href: '/settings', label: 'Settings' }
	];

	function active(href: string): boolean {
		if (href === '/') return page.url.pathname === '/';
		return page.url.pathname.startsWith(href);
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>AniDoku</title>
</svelte:head>

<header class="topnav">
	<a class="brand" href="/">Ani<span>Doku</span></a>
	<nav>
		{#each nav as item (item.href)}
			<a href={item.href} class:active={active(item.href)}>{item.label}</a>
		{/each}
	</nav>
	<div class="spacer"></div>
	<a
		class="bell"
		href="/inbox"
		class:active={active('/inbox')}
		aria-label="Notification inbox{notifyState.unread > 0 ? ` (${notifyState.unread} unread)` : ''}"
		title="Airing notifications"
	>
		<svg viewBox="0 0 24 24" aria-hidden="true">
			<path
				d="M18 8a6 6 0 1 0-12 0c0 7-3 8-3 8h18s-3-1-3-8m-4.7 11a2 2 0 0 1-3.4 0"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				stroke-linecap="round"
				stroke-linejoin="round"
			/>
		</svg>
		{#if notifyState.unread > 0}
			<span class="unread">{notifyState.unread > 99 ? '99+' : notifyState.unread}</span>
		{/if}
	</a>
	{#if authState.viewer && authState.logged_in}
		<a class="viewer" href="/settings" title="AniList: {authState.viewer.name}">
			{#if authState.viewer.avatar_url}
				<img src={authState.viewer.avatar_url} alt="" />
			{/if}
			<span>{authState.viewer.name}</span>
		</a>
	{:else if authState.expired}
		<a class="signin expired" href="/settings">Session expired — sign in</a>
	{:else}
		<a class="signin" href="/settings">Sign in to AniList</a>
	{/if}
</header>

<main>
	{@render children()}
</main>

{#if showSplash}
	<div class="splash" class:fading={splashFading} aria-hidden="true">
		<img class="splash-logo" src={splashLogo} alt="AniDoku" />
	</div>
{/if}

{#if toasts.length > 0}
	<div class="toasts">
		{#each toasts as t (t.id)}
			<div class="toast" class:sync={t.kind === 'sync'}>{t.message}</div>
		{/each}
	</div>
{/if}

<style>
	.topnav {
		height: 64px;
		display: flex;
		align-items: center;
		gap: var(--space-lg);
		padding: 0 var(--space-lg);
		background: var(--color-canvas);
		border-bottom: 1px solid var(--color-hairline);
		position: sticky;
		top: 0;
		z-index: 10;
	}
	.brand {
		font: var(--text-title-lg);
		color: var(--color-on-dark);
		letter-spacing: -0.3px;
	}
	.brand span {
		color: var(--color-primary);
	}
	nav {
		display: flex;
		gap: var(--space-md);
	}
	nav a {
		font: var(--text-nav);
		color: var(--color-muted-strong);
		padding: 6px 10px;
		border-radius: var(--radius-md);
	}
	nav a:hover {
		color: var(--color-on-dark);
	}
	nav a.active {
		color: var(--color-on-dark);
		background: var(--color-surface-card);
	}
	.spacer {
		flex: 1;
	}
	.bell {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: center;
		width: 36px;
		height: 36px;
		border-radius: var(--radius-md);
		color: var(--color-muted-strong);
	}
	.bell:hover {
		color: var(--color-on-dark);
	}
	.bell.active {
		color: var(--color-on-dark);
		background: var(--color-surface-card);
	}
	.bell svg {
		width: 19px;
		height: 19px;
	}
	.unread {
		position: absolute;
		top: 2px;
		right: 0;
		min-width: 16px;
		height: 16px;
		padding: 0 4px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-primary);
		color: var(--color-on-primary);
		font: 600 10px/1 var(--font-num);
		border-radius: var(--radius-pill);
	}
	.viewer {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		color: var(--color-body);
		font: var(--text-nav);
		padding: 4px 10px 4px 4px;
		background: var(--color-surface-card);
		border-radius: var(--radius-pill);
	}
	.viewer img {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		object-fit: cover;
	}
	.signin {
		font: var(--text-nav);
		color: var(--color-primary);
	}
	.signin.expired {
		color: var(--color-down);
	}
	main {
		max-width: 1280px;
		margin: 0 auto;
		padding: var(--space-lg);
		/* Backstop: the page body must never scroll horizontally (DESIGN.md).
		   `clip` contains stray overflow without creating a scroll container the
		   way `hidden` would. Real offenders are fixed at their source. */
		overflow-x: clip;
	}
	.toasts {
		position: fixed;
		bottom: var(--space-lg);
		right: var(--space-lg);
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		z-index: 100;
	}
	.toast {
		background: var(--color-surface-elevated);
		color: var(--color-body);
		border: 1px solid var(--color-hairline);
		border-left: 3px solid var(--color-muted);
		border-radius: var(--radius-md);
		padding: var(--space-sm) var(--space-md);
		font: var(--text-body-sm);
		max-width: 320px;
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
	}
	.toast.sync {
		border-left-color: var(--color-primary);
	}
	/* Boot splash: dark canvas, centered logo. The single yellow accent lives in
	   the logo itself — a deliberate brand moment (DESIGN.md). */
	.splash {
		position: fixed;
		inset: 0;
		z-index: 1000;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--color-canvas);
		opacity: 1;
		transition: opacity 0.3s ease;
	}
	.splash.fading {
		opacity: 0;
	}
	.splash-logo {
		width: 160px;
		height: 160px;
		border-radius: var(--radius-xl);
		animation: splash-in 0.9s cubic-bezier(0.2, 0.7, 0.2, 1) both;
	}
	@keyframes splash-in {
		from {
			opacity: 0;
			transform: scale(0.82);
		}
		to {
			opacity: 1;
			transform: scale(1);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.splash-logo {
			animation: none;
		}
	}
</style>
