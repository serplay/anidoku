<script lang="ts">
	import '@fontsource/inter/400.css';
	import '@fontsource/inter/500.css';
	import '@fontsource/inter/600.css';
	import '@fontsource/inter/700.css';
	import '@fontsource/ibm-plex-sans/500.css';
	import '$lib/styles/tokens.css';
	import favicon from '$lib/assets/favicon.svg';
	import { page } from '$app/state';
	import {
		anilistStatus,
		onEvent,
		isDesktop,
		type RemoteOverwrite
	} from '$lib/api';
	import { authState, setAuthStatus, toasts, pushToast } from '$lib/state.svelte';

	let { children } = $props();

	async function refreshAuth() {
		if (!isDesktop()) return;
		try {
			setAuthStatus(await anilistStatus());
		} catch {
			/* browser / not ready */
		}
	}

	$effect(() => {
		refreshAuth();
		const unlisteners = [
			onEvent<RemoteOverwrite>('sync:remote-overwrote', (p) => {
				pushToast(`Synced from AniList: ${p.title ?? 'an entry'}`, 'sync');
			}),
			onEvent('sync:updated', () => refreshAuth()),
			onEvent('sync:auth-expired', () => {
				refreshAuth();
				pushToast('AniList session expired — sign in again in Settings.', 'info');
			})
		];
		return () => {
			for (const u of unlisteners) u.then((fn) => fn());
		};
	});

	const nav = [
		{ href: '/', label: 'Search' },
		{ href: '/library', label: 'Library' },
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
</style>
