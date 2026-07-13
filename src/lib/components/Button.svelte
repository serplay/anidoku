<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		variant?: 'primary' | 'secondary' | 'ghost';
		type?: 'button' | 'submit';
		disabled?: boolean;
		onclick?: (e: MouseEvent) => void;
		children: Snippet;
	}

	let {
		variant = 'primary',
		type = 'button',
		disabled = false,
		onclick,
		children
	}: Props = $props();
</script>

<button class="btn {variant}" {type} {disabled} {onclick}>
	{@render children()}
</button>

<style>
	.btn {
		font: var(--text-button);
		border: none;
		border-radius: var(--radius-md);
		padding: 12px 24px;
		height: 40px;
		cursor: pointer;
		transition: background-color 0.12s ease;
		white-space: nowrap;
	}

	.btn:disabled {
		cursor: not-allowed;
	}

	.primary {
		background: var(--color-primary);
		color: var(--color-on-primary);
	}
	.primary:hover:not(:disabled) {
		background: var(--color-primary-active);
	}
	.primary:disabled {
		background: var(--color-primary-disabled);
		color: var(--color-muted);
	}

	.secondary {
		background: var(--color-surface-card);
		color: var(--color-on-dark);
	}
	.secondary:hover:not(:disabled) {
		background: var(--color-surface-elevated);
	}

	.ghost {
		background: transparent;
		color: var(--color-body);
	}
	.ghost:hover:not(:disabled) {
		color: var(--color-on-dark);
	}
</style>
