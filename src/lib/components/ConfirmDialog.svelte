<script lang="ts">
	interface Props {
		title: string;
		/** Optional second line, e.g. what exactly gets removed. */
		body?: string | null;
		confirmLabel?: string;
		onconfirm: () => void;
		oncancel: () => void;
	}

	let { title, body = null, confirmLabel = 'Delete', onconfirm, oncancel }: Props = $props();

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') oncancel();
	}

	// Focus starts on Cancel so a stray Enter never confirms a destructive action.
	function focusOnMount(el: HTMLButtonElement) {
		el.focus();
	}
</script>

<svelte:window {onkeydown} />

<div
	class="scrim"
	role="presentation"
	onclick={(e) => e.target === e.currentTarget && oncancel()}
>
	<div class="dialog" role="alertdialog" aria-modal="true" aria-label={title} tabindex="-1">
		<p class="title">{title}</p>
		{#if body}
			<p class="body">{body}</p>
		{/if}
		<div class="actions">
			<button class="cancel" use:focusOnMount onclick={oncancel}>Cancel</button>
			<button class="confirm" onclick={onconfirm}>{confirmLabel}</button>
		</div>
	</div>
</div>

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 100;
		background: rgba(11, 14, 17, 0.6);
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.dialog {
		background: var(--color-surface-card);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-xl);
		padding: var(--space-lg);
		width: min(380px, calc(100vw - 2 * var(--space-lg)));
	}
	.title {
		font: var(--text-title-sm);
		color: var(--color-on-dark);
		margin: 0;
	}
	.body {
		font: var(--text-body-md);
		color: var(--color-muted-strong);
		margin: var(--space-xs) 0 0;
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-xs);
		margin-top: var(--space-lg);
	}
	.actions button {
		border: none;
		border-radius: var(--radius-md);
		padding: 8px 16px;
		height: 34px;
		font: var(--text-button);
		cursor: pointer;
	}
	.cancel {
		background: var(--color-surface-elevated);
		color: var(--color-on-dark);
	}
	.confirm {
		background: transparent;
		border: 1px solid var(--color-down);
		color: var(--color-down);
	}
	.confirm:hover {
		background: var(--color-down);
		color: var(--color-on-dark);
	}
</style>
