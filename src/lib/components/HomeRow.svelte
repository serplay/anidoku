<script lang="ts">
	import type { Snippet } from 'svelte';
	import Skeleton from '$lib/components/Skeleton.svelte';

	interface Props {
		title: string;
		/** Show shimmer placeholders instead of content. */
		loading?: boolean;
		/** Skeleton card count while loading. */
		placeholders?: number;
		empty?: string | null;
		children?: Snippet;
	}

	let { title, loading = false, placeholders = 8, empty = null, children }: Props = $props();
</script>

<section class="row">
	<h2>{title}</h2>
	{#if loading}
		<div class="scroller">
			{#each Array(placeholders), i (i)}
				<div class="sk">
					<Skeleton aspect="2 / 3" radius="var(--radius-lg)" />
					<Skeleton width="85%" height="14px" />
				</div>
			{/each}
		</div>
	{:else if empty}
		<p class="empty">{empty}</p>
	{:else}
		<div class="scroller">
			{@render children?.()}
		</div>
	{/if}
</section>

<style>
	.row {
		margin-bottom: var(--space-xl);
	}
	h2 {
		font: var(--text-title-lg);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-md);
	}
	.scroller {
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: 150px;
		gap: var(--space-md);
		overflow-x: auto;
		padding-bottom: var(--space-xs);
		scrollbar-width: thin;
		scroll-snap-type: x proximity;
	}
	.scroller > :global(*) {
		scroll-snap-align: start;
	}
	.sk {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.empty {
		font: var(--text-body-md);
		color: var(--color-muted);
	}
</style>
