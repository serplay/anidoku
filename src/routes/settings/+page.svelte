<script lang="ts">
	import {
		getSettings,
		setClientId,
		anilistLogin,
		anilistLogout,
		anilistStatus,
		anilistSyncNow,
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

	$effect(() => {
		void init();
	});

	async function init() {
		if (!isDesktop()) return;
		try {
			settings = await getSettings();
			clientId = settings.client_id ?? '';
			setAuthStatus(await anilistStatus());
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
	.note {
		max-width: 640px;
		margin-top: var(--space-lg);
		color: var(--color-muted);
		font: var(--text-body-sm);
	}
	a {
		color: var(--color-primary);
	}
</style>
