<script lang="ts">
	interface Props {
		width?: string;
		height?: string;
		radius?: string;
		/** e.g. "2 / 3" for cover blocks; overrides height. */
		aspect?: string;
	}

	let { width = '100%', height = '16px', radius = 'var(--radius-md)', aspect }: Props = $props();
</script>

<div
	class="sk"
	style:width
	style:height={aspect ? 'auto' : height}
	style:aspect-ratio={aspect}
	style:border-radius={radius}
></div>

<style>
	.sk {
		background: linear-gradient(
			90deg,
			var(--color-surface-card) 25%,
			var(--color-surface-elevated) 50%,
			var(--color-surface-card) 75%
		);
		background-size: 200% 100%;
		animation: shimmer 1.4s ease infinite;
	}
	@keyframes shimmer {
		from {
			background-position: 200% 0;
		}
		to {
			background-position: -200% 0;
		}
	}
</style>
