<script lang="ts">
	import {
		getSettings,
		setClientId,
		anilistLogin,
		anilistLogout,
		anilistStatus,
		anilistSyncNow,
		getNotifyPlanning,
		setNotifyPlanning,
		refreshProviderConfig,
		setSourceEnabled,
		setSourceOrder,
		isDesktop,
		type Settings
	} from '$lib/api';
	import { authState, setAuthStatus, pushToast } from '$lib/state.svelte';
	import Button from '$lib/components/Button.svelte';

	let settings = $state<Settings | null>(null);
	let clientId = $state('');
	let saving = $state(false);
	let loggingIn = $state(false);
	let error = $state<string | null>(null);
	let notifyPlanning = $state(false);
	let notifySaving = $state(false);
	let providerChecking = $state(false);

	async function checkProvider() {
		providerChecking = true;
		try {
			const r = await refreshProviderConfig();
			settings = await getSettings();
			pushToast(
				r.changed
					? `Source config updated (build ${r.build_id}).`
					: 'Every source is already on its latest published config.'
			);
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			providerChecking = false;
		}
	}

	async function toggleSource(source: string, enabled: boolean) {
		// Disabling the last enabled source would leave nothing to play from.
		const enabledCount = (settings?.sources ?? []).filter((s) => s.enabled).length;
		if (!enabled && enabledCount <= 1) {
			pushToast('At least one source has to stay on — nothing could play otherwise.');
			return;
		}
		try {
			await setSourceEnabled(source, enabled);
			settings = await getSettings();
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	/// Move a source up or down the failover chain. Order is sent as the full
	/// list so the backend never has to reconstruct intent from a delta.
	async function move(source: string, delta: -1 | 1) {
		const ids = (settings?.sources ?? []).map((s) => s.source);
		const i = ids.indexOf(source);
		const j = i + delta;
		if (i < 0 || j < 0 || j >= ids.length) return;
		[ids[i], ids[j]] = [ids[j], ids[i]];
		try {
			await setSourceOrder(ids);
			settings = await getSettings();
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	$effect(() => {
		void init();
	});

	async function init() {
		if (!isDesktop()) return;
		try {
			settings = await getSettings();
			clientId = settings.client_id ?? '';
			setAuthStatus(await anilistStatus());
			notifyPlanning = await getNotifyPlanning();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	async function saveClientId() {
		saving = true;
		error = null;
		try {
			await setClientId(clientId.trim() || null);
			settings = await getSettings();
			setAuthStatus(await anilistStatus());
			pushToast('Client ID saved.');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			saving = false;
		}
	}

	async function login() {
		loggingIn = true;
		error = null;
		try {
			const viewer = await anilistLogin();
			setAuthStatus(await anilistStatus());
			pushToast(`Signed in as ${viewer.name}.`, 'sync');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loggingIn = false;
		}
	}

	async function logout() {
		await anilistLogout();
		setAuthStatus(await anilistStatus());
		pushToast('Signed out of AniList.');
	}

	async function syncNow() {
		try {
			await anilistSyncNow();
			pushToast('Synced with AniList.', 'sync');
		} catch (e) {
			pushToast(e instanceof Error ? e.message : String(e));
		}
	}

	function copyRedirect() {
		if (settings) navigator.clipboard?.writeText(settings.redirect_url);
	}

	async function togglePlanning() {
		notifySaving = true;
		try {
			await setNotifyPlanning(notifyPlanning);
			pushToast(
				notifyPlanning
					? 'Planning shows will notify when episodes air.'
					: 'Planning shows no longer tracked.',
				'sync'
			);
		} catch (e) {
			notifyPlanning = !notifyPlanning; // revert on failure
			pushToast(e instanceof Error ? e.message : String(e));
		} finally {
			notifySaving = false;
		}
	}
</script>

<h1>Settings</h1>

{#if !isDesktop()}
	<p class="hint">Settings need the desktop app: <code>npm run tauri dev</code>.</p>
{/if}

<section class="card">
	<h2>AniList account</h2>

	{#if authState.logged_in && authState.viewer}
		<div class="signed-in">
			{#if authState.viewer.avatar_url}
				<img src={authState.viewer.avatar_url} alt="" />
			{/if}
			<div class="who">
				<span class="name">{authState.viewer.name}</span>
				<span class="sub">Signed in · tokens last ~1 year (no refresh)</span>
			</div>
			<div class="actions">
				<Button variant="secondary" onclick={syncNow}>Sync now</Button>
				<Button variant="ghost" onclick={logout}>Log out</Button>
			</div>
		</div>
	{:else}
		{#if authState.expired}
			<p class="expired">Your AniList session expired. Sign in again to resume sync.</p>
		{/if}

		<ol class="steps">
			<li>
				Create a client at
				<a href="https://anilist.co/settings/developer" target="_blank" rel="noreferrer"
					>anilist.co/settings/developer</a
				>.
			</li>
			<li>
				Set the client's <strong>Redirect URL</strong> to exactly:
				<code class="redirect">{settings?.redirect_url ?? 'http://127.0.0.1:8737/callback'}</code>
				<button class="copy" onclick={copyRedirect}>copy</button>
			</li>
			<li>Paste the numeric <strong>Client ID</strong> below and save.</li>
		</ol>

		<label class="field">
			<span>AniList Client ID</span>
			<input
				type="text"
				bind:value={clientId}
				placeholder="e.g. 12345"
				inputmode="numeric"
			/>
		</label>
		<div class="row">
			<Button onclick={saveClientId} disabled={saving}>
				{saving ? 'Saving…' : 'Save Client ID'}
			</Button>
			<Button
				variant="secondary"
				onclick={login}
				disabled={loggingIn || !authState.has_client_id}
			>
				{loggingIn ? 'Waiting for browser…' : 'Sign in to AniList'}
			</Button>
		</div>
		{#if !authState.has_client_id}
			<p class="hint">Save a Client ID first, then sign in.</p>
		{/if}
	{/if}

	{#if error}
		<p class="error">{error}</p>
	{/if}
</section>

<section class="card provider" data-testid="provider-card">
	<h2>Streaming sources</h2>
	<p class="hint">
		Episodes are looked up across every source you leave on, top to bottom — if one is down or
		missing an episode, the next is used automatically. Sources rotate their access scheme every
		few weeks; a fix is published automatically and picked up on the next play attempt, but you
		can fetch it right away.
	</p>
	<ul class="sources" data-testid="source-list">
		{#each settings?.sources ?? [] as s, i (s.source)}
			<li class="source-row" class:off={!s.enabled} data-testid="source-{s.source}">
				<label class="toggle">
					<input
						type="checkbox"
						checked={s.enabled}
						onchange={(e) => toggleSource(s.source, e.currentTarget.checked)}
					/>
					<span class="name">{s.display_name}</span>
				</label>
				<span class="meta">
					{#if s.config_source === 'static'}
						no rotating config
					{:else}
						build <strong data-testid="provider-build">{s.build_id || '—'}</strong>
						· config <strong data-testid="provider-source">{s.config_source}</strong>
					{/if}
				</span>
				<span class="reorder">
					<button
						type="button"
						aria-label="Move {s.display_name} up"
						disabled={i === 0}
						onclick={() => move(s.source, -1)}>↑</button
					>
					<button
						type="button"
						aria-label="Move {s.display_name} down"
						disabled={i === (settings?.sources.length ?? 0) - 1}
						onclick={() => move(s.source, 1)}>↓</button
					>
				</span>
			</li>
		{/each}
	</ul>
	<div class="provider-row">
		<Button variant="secondary" onclick={checkProvider} disabled={providerChecking}>
			{providerChecking ? 'Checking…' : 'Check for provider update'}
		</Button>
	</div>
</section>

<section class="card notifications">
	<h2>Notifications</h2>
	<p class="hint">
		Shows on your Watching list that are currently airing notify you when a new episode is out
		(bell icon in the top bar + a system notification).
	</p>
	<label class="toggle">
		<input
			type="checkbox"
			bind:checked={notifyPlanning}
			onchange={togglePlanning}
			disabled={notifySaving || !isDesktop()}
		/>
		Also notify for shows on my Planning list
	</label>
</section>

<p class="note">
	When signed out, AniDoku still tracks your progress locally. Watching to 85% of an episode marks
	it watched and (once mapped) queues an AniList update that syncs when you're back online.
</p>

<style>
	h1 {
		font: var(--text-display-sm);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-lg);
	}
	.card {
		background: var(--color-surface-card);
		border-radius: var(--radius-xl);
		padding: var(--space-lg);
		max-width: 640px;
	}
	h2 {
		font: var(--text-title-md);
		color: var(--color-on-dark);
		margin: 0 0 var(--space-md);
	}
	.steps {
		margin: 0 0 var(--space-lg);
		padding-left: var(--space-lg);
		color: var(--color-body);
		font: var(--text-body-md);
		line-height: 1.9;
	}
	.redirect {
		display: inline-block;
		background: var(--color-canvas);
		color: var(--color-primary);
		padding: 2px 8px;
		border-radius: var(--radius-sm);
		font: var(--text-num-sm);
	}
	.copy {
		background: none;
		border: none;
		color: var(--color-muted-strong);
		cursor: pointer;
		font: var(--text-caption);
		text-decoration: underline;
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		margin-bottom: var(--space-md);
	}
	.field span {
		font: var(--text-caption);
		color: var(--color-muted-strong);
	}
	.field input {
		background: var(--color-canvas);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md);
		color: var(--color-on-dark);
		padding: 10px 16px;
		height: 40px;
		font: var(--text-body-md);
	}
	.field input:focus-visible {
		border-color: var(--color-primary);
	}
	.row {
		display: flex;
		gap: var(--space-md);
	}
	.signed-in {
		display: flex;
		align-items: center;
		gap: var(--space-md);
	}
	.signed-in img {
		width: 56px;
		height: 56px;
		border-radius: var(--radius-lg);
		object-fit: cover;
	}
	.who {
		display: flex;
		flex-direction: column;
	}
	.name {
		font: var(--text-title-sm);
		color: var(--color-on-dark);
	}
	.sub {
		font: var(--text-caption);
		color: var(--color-muted);
	}
	.actions {
		margin-left: auto;
		display: flex;
		gap: var(--space-xs);
	}
	.hint {
		font: var(--text-body-sm);
		color: var(--color-muted);
	}
	.hint code {
		color: var(--color-primary);
	}
	.expired {
		color: var(--color-down);
		font: var(--text-body-md);
	}
	.error {
		color: var(--color-down);
		font: var(--text-body-md);
		margin-top: var(--space-md);
	}
	.notifications {
		margin-top: var(--space-lg);
	}
	.toggle {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		font: var(--text-body-md);
		color: var(--color-body);
		cursor: pointer;
		user-select: none;
	}
	.toggle input {
		accent-color: var(--color-primary);
	}
	.note {
		max-width: 640px;
		margin-top: var(--space-lg);
		color: var(--color-muted);
		font: var(--text-body-sm);
	}
	a {
		color: var(--color-primary);
	}
	@media (max-width: 767px) {
		h1 {
			font: var(--text-title-lg);
		}
		.signed-in {
			flex-wrap: wrap;
		}
		.row {
			flex-wrap: wrap;
			gap: var(--space-sm);
		}
		.redirect {
			word-break: break-all;
		}
	}
	.provider-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-md);
		flex-wrap: wrap;
	}

	.sources {
		list-style: none;
		margin: var(--space-md) 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}
	.source-row {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: var(--space-sm) var(--space-md);
		background: var(--color-surface-raised, rgba(255, 255, 255, 0.03));
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-md, 6px);
	}
	.source-row.off {
		opacity: 0.55;
	}
	.source-row .toggle {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		margin: 0;
		cursor: pointer;
	}
	.source-row .name {
		font-weight: 600;
	}
	.source-row .meta {
		margin-left: auto;
		font-size: var(--font-size-sm, 0.85rem);
		color: var(--color-text-muted);
		white-space: nowrap;
	}
	.reorder {
		display: flex;
		gap: 2px;
	}
	.reorder button {
		width: 26px;
		height: 26px;
		line-height: 1;
		background: transparent;
		color: var(--color-text-muted);
		border: 1px solid var(--color-hairline);
		border-radius: var(--radius-sm, 4px);
		cursor: pointer;
	}
	.reorder button:disabled {
		opacity: 0.35;
		cursor: default;
	}

	/* The meta column is the first thing to go when there's no room. */
	@media (max-width: 560px) {
		.source-row {
			flex-wrap: wrap;
		}
		.source-row .meta {
			margin-left: 0;
			width: 100%;
			order: 3;
		}
		.reorder {
			margin-left: auto;
		}
	}
</style>
