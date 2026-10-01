<script lang="ts">
	import Button from '$lib/components/Button.svelte';

	interface Props {
		/** Raw backend error, kept for bug reports. */
		detail: string;
		/** Display name of the source that broke, when it is known. */
		source?: string | null;
		checking?: boolean;
		oncheck: () => void;
		onretry: () => void;
	}

	let { detail, source = null, checking = false, oncheck, onretry }: Props = $props();
</script>

<div class="outage" data-testid="provider-outage" role="alert">
	<svg viewBox="0 0 24 24" aria-hidden="true">
		<path
			d="M12 3l9 16H3z M12 10v4 M12 17.5v.5"
			stroke="currentColor"
			stroke-width="1.8"
			fill="none"
			stroke-linecap="round"
			stroke-linejoin="round"
		/>
	</svg>
	<p class="title">{source ? `${source} changed its access scheme` : 'Streaming source changed'}</p>
	<p class="sub">
		{source ? `${source} updated how it hands out streams` : "The video provider updated its access scheme"},
		so this build can't fetch from it right now. A fix is published automatically once detected —
		check for it now, or try again later. Other sources you have enabled are used automatically
		when they have this episode.
	</p>
	<div class="actions">
		<Button onclick={oncheck} disabled={checking}>
			{checking ? 'Checking…' : 'Check for fix'}
		</Button>
		<Button variant="secondary" onclick={onretry} disabled={checking}>Retry</Button>
	</div>
	<details>
		<summary>Technical details</summary>
		<code>{detail}</code>
	</details>
</div>

<style>
	.outage {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		text-align: center;
		gap: var(--space-sm);
		padding: var(--space-xxl) var(--space-lg);
		background: var(--color-surface-card);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-xl);
		aspect-ratio: 16 / 9;
	}
	svg {
		width: 48px;
		height: 48px;
		color: var(--color-accent);
	}
	.title {
		font: var(--text-h3);
		margin: 0;
	}
	.sub {
		color: var(--color-text-secondary);
		max-width: 46ch;
		margin: 0;
	}
	.actions {
		display: flex;
		gap: var(--space-sm);
		margin-top: var(--space-sm);
	}
	details {
		margin-top: var(--space-sm);
		color: var(--color-text-secondary);
		font-size: 0.85em;
		max-width: 100%;
	}
	code {
		display: block;
		margin-top: var(--space-xs);
		word-break: break-word;
		text-align: left;
	}
</style>
