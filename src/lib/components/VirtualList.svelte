<script lang="ts" generics="T">
	import type { Snippet } from 'svelte';

	interface Props {
		items: T[];
		itemHeight: number;
		height?: number;
		overscan?: number;
		row: Snippet<[T, number]>;
	}

	let { items, itemHeight, height = 520, overscan = 6, row }: Props = $props();

	let scrollTop = $state(0);

	const total = $derived(items.length * itemHeight);
	const start = $derived(Math.max(0, Math.floor(scrollTop / itemHeight) - overscan));
	const visibleCount = $derived(Math.ceil(height / itemHeight) + overscan * 2);
	const end = $derived(Math.min(items.length, start + visibleCount));
	const slice = $derived(items.slice(start, end));
	const offset = $derived(start * itemHeight);

	function onscroll(e: Event) {
		scrollTop = (e.currentTarget as HTMLElement).scrollTop;
	}
</script>

<div class="vp" style="height:{height}px" {onscroll}>
	<div class="spacer" style="height:{total}px">
		<div class="window" style="transform:translateY({offset}px)">
			{#each slice as item, i (start + i)}
				<div style="height:{itemHeight}px">
					{@render row(item, start + i)}
				</div>
			{/each}
		</div>
	</div>
</div>

<style>
	.vp {
		overflow-y: auto;
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-xl);
		background: var(--color-surface-card);
	}
	.spacer {
		position: relative;
		width: 100%;
	}
	.window {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
	}
</style>
