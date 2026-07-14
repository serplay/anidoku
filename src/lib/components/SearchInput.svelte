<script lang="ts">
	interface Props {
		value?: string;
		placeholder?: string;
		onsubmit?: () => void;
	}

	let { value = $bindable(''), placeholder = 'Search anime…', onsubmit }: Props = $props();

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') onsubmit?.();
	}
</script>

<div class="search">
	<svg viewBox="0 0 24 24" class="icon" aria-hidden="true">
		<path
			d="M15.5 14h-.79l-.28-.27a6.5 6.5 0 1 0-.7.7l.27.28v.79l5 5 1.5-1.5-5-5zm-6 0A4.5 4.5 0 1 1 14 9.5 4.5 4.5 0 0 1 9.5 14z"
			fill="currentColor"
		/>
	</svg>
	<input {placeholder} bind:value {onkeydown} spellcheck="false" autocomplete="off" />
</div>

<style>
	.search {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		background: var(--color-surface-card);
		border: 1px solid transparent;
		border-radius: var(--radius-lg);
		padding: 10px 16px;
		height: 40px;
	}
	/* No blue focus ring — a one-step surface lift marks focus instead. */
	.search:focus-within {
		background: var(--color-surface-elevated);
		border-color: var(--color-hairline);
	}
	.icon {
		width: 18px;
		height: 18px;
		color: var(--color-muted);
		flex-shrink: 0;
	}
	input {
		flex: 1;
		background: transparent;
		border: none;
		outline: none;
		color: var(--color-on-dark);
		font: var(--text-body-md);
	}
	input::placeholder {
		color: var(--color-muted);
	}
</style>
