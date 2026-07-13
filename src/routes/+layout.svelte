<script lang="ts">
	import '@fontsource/inter/400.css';
	import '@fontsource/inter/500.css';
	import '@fontsource/inter/600.css';
	import '@fontsource/inter/700.css';
	import '@fontsource/ibm-plex-sans/500.css';
	import '$lib/styles/tokens.css';
	import favicon from '$lib/assets/favicon.svg';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>AniDoku</title>
</svelte:head>

<header class="topnav">
	<a class="brand" href="/">Ani<span>Doku</span></a>
</header>

<main>
	{@render children()}
</main>

<style>
	.topnav {
		height: 64px;
		display: flex;
		align-items: center;
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
	main {
		max-width: 1280px;
		margin: 0 auto;
		padding: var(--space-lg);
	}
</style>
