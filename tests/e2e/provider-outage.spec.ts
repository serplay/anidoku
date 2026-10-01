import { test, expect } from '@playwright/test';
import { mockTauri, reject, sequence, sourceFixture } from './tauri-mock';

const WATCH_URL = '/watch/show-one-piece/1?dub=0';
const ROTATED = 'PROVIDER_ROTATED: sources: bootstrap rejected epoch 2957 (404 Not Found)';

/**
 * The allanime provider rotates its anti-scraping scheme every few weeks. The
 * backend tags those failures with PROVIDER_ROTATED: and publishes a remote
 * config fix automatically; the frontend must turn that into an actionable
 * state (check for fix / retry) rather than a raw error string.
 */
test.describe('watch page — provider rotation', () => {
	test('a rotation error renders the outage state, not the raw error', async ({ page }) => {
		await mockTauri(page, { get_sources: reject(ROTATED) });
		await page.goto(WATCH_URL);

		const outage = page.getByTestId('provider-outage');
		await expect(outage).toBeVisible();
		await expect(outage.getByText('Streaming source changed')).toBeVisible();
		await expect(outage.getByRole('button', { name: 'Check for fix' })).toBeVisible();
		await expect(outage.getByRole('button', { name: 'Retry' })).toBeVisible();
		// The raw message is still available for bug reports…
		await outage.getByText('Technical details').click();
		await expect(outage.getByText('bootstrap rejected epoch 2957')).toBeVisible();
		// …but not shown as the generic error path, and nothing is mounted to play.
		await expect(page.locator('p.error')).toHaveCount(0);
		await expect(page.locator('video')).toHaveCount(0);
	});

	test('"Check for fix" applies the published config and reloads sources', async ({ page }) => {
		await mockTauri(page, {
			// First attempt fails (rotated); after the fix is applied the same
			// command returns playable sources.
			get_sources: sequence(reject(ROTATED), [sourceFixture({ quality: '1080' })]),
			refresh_provider_config: { changed: true, build_id: '167', config_source: 'remote' }
		});
		await page.goto(WATCH_URL);
		await expect(page.getByTestId('provider-outage')).toBeVisible();

		await page.getByRole('button', { name: 'Check for fix' }).click();

		await expect(page.locator('video')).toBeVisible();
		await expect(page.getByTestId('provider-outage')).toHaveCount(0);
		await expect(page.getByText('Provider fix applied (build 167)')).toBeVisible();

		const cmds = await page.evaluate(() =>
			(window as unknown as { __IPC_CALLS__: { cmd: string }[] }).__IPC_CALLS__.map((c) => c.cmd)
		);
		const refreshAt = cmds.indexOf('refresh_provider_config');
		expect(refreshAt).toBeGreaterThan(-1);
		expect(cmds.slice(refreshAt)).toContain('get_sources');
		expect(cmds.filter((c) => c === 'get_sources')).toHaveLength(2);
	});

	test('"Check for fix" with nothing published keeps the outage state and says so', async ({
		page
	}) => {
		await mockTauri(page, {
			get_sources: reject(ROTATED),
			refresh_provider_config: { changed: false, build_id: '166', config_source: 'baked' }
		});
		await page.goto(WATCH_URL);
		await page.getByRole('button', { name: 'Check for fix' }).click();

		await expect(page.getByText('No fix published yet')).toBeVisible();
		await expect(page.getByTestId('provider-outage')).toBeVisible();
		// No second sources fetch — the config didn't change.
		const n = await page.evaluate(
			() =>
				(window as unknown as { __IPC_CALLS__: { cmd: string }[] }).__IPC_CALLS__.filter(
					(c) => c.cmd === 'get_sources'
				).length
		);
		expect(n).toBe(1);
	});

	test('"Retry" re-fetches sources and recovers when the provider is back', async ({ page }) => {
		await mockTauri(page, {
			get_sources: sequence(reject(ROTATED), [sourceFixture()])
		});
		await page.goto(WATCH_URL);
		await page.getByRole('button', { name: 'Retry' }).click();
		await expect(page.locator('video')).toBeVisible();
	});

	test('a non-rotation error keeps the plain error path', async ({ page }) => {
		await mockTauri(page, {
			get_sources: reject('network error: error sending request for url (https://api.mkissa.net/api)')
		});
		await page.goto(WATCH_URL);

		await expect(page.locator('p.error')).toBeVisible();
		await expect(page.locator('p.error')).toContainText('error sending request');
		await expect(page.getByTestId('provider-outage')).toHaveCount(0);
	});
});

test.describe('settings — streaming provider', () => {
	test('shows the build and config source of each registered source', async ({ page }) => {
		await mockTauri(page, {
			get_settings: {
				client_id: null,
				redirect_url: 'http://127.0.0.1:8737/callback',
				sources: [{ source: 'allanime', display_name: 'AllAnime', build_id: '174', config_source: 'remote', config_url: '', enabled: true }]
			}
		});
		await page.goto('/settings');

		const card = page.getByTestId('provider-card');
		await expect(card).toBeVisible();
		await expect(card.getByTestId('provider-build')).toHaveText('174');
		await expect(card.getByTestId('provider-source')).toHaveText('remote');
		await expect(card.getByRole('button', { name: 'Check for provider update' })).toBeVisible();
	});

	test('lists every source, ordered, with the disabled ones marked', async ({ page }) => {
		await mockTauri(page, {
			get_settings: {
				client_id: null,
				redirect_url: 'x',
				sources: [{ source: 'allanime', display_name: 'AllAnime', build_id: '174', config_source: 'remote', config_url: '', enabled: true }, { source: 'anizone', display_name: 'AniZone', build_id: '', config_source: 'static', config_url: '', enabled: true }, { source: 'animegg', display_name: 'AnimeGG', build_id: '', config_source: 'static', config_url: '', enabled: false }]
			}
		});
		await page.goto('/settings');

		const rows = page.getByTestId('source-list').locator('li');
		await expect(rows).toHaveCount(3);
		await expect(page.getByTestId('source-animegg').locator('input')).not.toBeChecked();
		// A source with nothing volatile says so rather than showing a blank build.
		await expect(page.getByTestId('source-animegg')).toContainText('no rotating config');
		// The first source cannot move up, the last cannot move down.
		await expect(page.getByRole('button', { name: 'Move AllAnime up' })).toBeDisabled();
		await expect(page.getByRole('button', { name: 'Move AnimeGG down' })).toBeDisabled();
	});

	test('ships three independent sources, all on by default', async ({ page }) => {
		// The mock's default settings mirror what the app registers.
		await mockTauri(page);
		await page.goto('/settings');

		const rows = page.getByTestId('source-list').locator('li');
		await expect(rows).toHaveCount(3);
		for (const slug of ['allanime', 'anizone', 'animegg']) {
			await expect(page.getByTestId(`source-${slug}`).locator('input')).toBeChecked();
		}
	});

	test('turning one source off leaves the others on and is persisted', async ({ page }) => {
		await mockTauri(page);
		await page.goto('/settings');
		await page.getByTestId('source-allanime').locator('input').uncheck();

		await expect(page.getByText('At least one source has to stay on')).toHaveCount(0);
		const call = await page.evaluate(() =>
			(
				window as unknown as { __IPC_CALLS__: { cmd: string; args: unknown }[] }
			).__IPC_CALLS__.find((c) => c.cmd === 'set_source_enabled')
		);
		expect(call?.args).toEqual({ source: 'allanime', enabled: false });
	});

	test('moving a source down reorders the failover chain', async ({ page }) => {
		await mockTauri(page);
		await page.goto('/settings');
		await page.getByRole('button', { name: 'Move AllAnime down' }).click();

		const call = await page.evaluate(() =>
			(
				window as unknown as { __IPC_CALLS__: { cmd: string; args: unknown }[] }
			).__IPC_CALLS__.find((c) => c.cmd === 'set_source_order')
		);
		expect(call?.args).toEqual({ order: ['anizone', 'allanime', 'animegg'] });
	});

	test('refuses to turn off the last remaining source', async ({ page }) => {
		await mockTauri(page, {
			get_settings: {
				client_id: null,
				redirect_url: 'x',
				sources: [{ source: 'allanime', display_name: 'AllAnime', build_id: '174', config_source: 'baked', config_url: '', enabled: true }]
			}
		});
		await page.goto('/settings');
		await page.getByTestId('source-allanime').locator('input').uncheck();

		await expect(page.getByText('At least one source has to stay on')).toBeVisible();
	});

	test('"Check for provider update" reports the result', async ({ page }) => {
		await mockTauri(page, {
			get_settings: sequence(
				{ client_id: null, redirect_url: 'x', sources: [{ source: 'allanime', display_name: 'AllAnime', build_id: '174', config_source: 'baked', config_url: '', enabled: true }] },
				{ client_id: null, redirect_url: 'x', sources: [{ source: 'allanime', display_name: 'AllAnime', build_id: '175', config_source: 'remote', config_url: '', enabled: true }] }
			),
			refresh_provider_config: { changed: true, build_id: '175', config_source: 'remote' }
		});
		await page.goto('/settings');
		await page.getByRole('button', { name: 'Check for provider update' }).click();

		await expect(page.getByText('Source config updated (build 175)')).toBeVisible();
		await expect(page.getByTestId('provider-build')).toHaveText('175');
		await expect(page.getByTestId('provider-source')).toHaveText('remote');
	});
});
