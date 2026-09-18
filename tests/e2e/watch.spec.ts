import { test, expect } from '@playwright/test';
import { mockTauri, reject, sourceFixture } from './tauri-mock';

const WATCH_URL = '/watch/show-one-piece/1?dub=0';

test.describe('watch page — sources', () => {
	test('shows a calm "no sources" empty state instead of erroring', async ({ page }) => {
		// The provider legitimately returns zero sources for this episode.
		await mockTauri(page, { get_sources: [] });
		await page.goto(WATCH_URL);

		await expect(page.getByText('No sources available')).toBeVisible();
		// It is an informational empty-state, not the red error path…
		await expect(page.locator('.empty-state')).toBeVisible();
		await expect(page.locator('p.error')).toHaveCount(0);
		// …and no <video> is mounted when there is nothing to play.
		await expect(page.locator('video')).toHaveCount(0);
	});

	test('renders quality chips when multiple sources exist', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				sourceFixture({ quality: '1080', provider_name: 'Default' }),
				sourceFixture({
					quality: '720',
					provider_name: 'Backup',
					url: 'http://127.0.0.1:9/other.mp4'
				})
			]
		});
		await page.goto(WATCH_URL);

		// The player mounts and the quality switcher lists both renditions.
		await expect(page.locator('video')).toBeVisible();
		await expect(page.getByText('Quality')).toBeVisible();
		await expect(page.locator('.chips .chip')).toHaveCount(2);
		await expect(page.locator('.chips .chip', { hasText: '1080' })).toBeVisible();
		await expect(page.locator('.chips .chip', { hasText: '720' })).toBeVisible();
	});

	test('surfaces a playback error when the only source fails to load', async ({ page }) => {
		// A single, unreachable source: the <video> will error, auto-advance finds
		// no next source, and the real playback error is surfaced.
		await mockTauri(page, { get_sources: [sourceFixture()] });
		await page.goto(WATCH_URL);

		await expect(page.locator('video')).toBeVisible();
		// The media element errors on the bogus URL; the page reports it rather
		// than silently spinning.
		await expect(page.locator('p.error')).toBeVisible({ timeout: 15_000 });
	});
});

test.describe('watch page — multiple sources', () => {
	test('groups the picker by source when links come from more than one', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				sourceFixture({ source: 'allanime', quality: '1080', provider_name: 'Default' }),
				sourceFixture({
					source: 'hianime',
					quality: '720',
					provider_name: 'Megacloud',
					url: 'http://127.0.0.1:9/hi.mp4'
				})
			]
		});
		await page.goto('/watch/allanime:show-one-piece/1?dub=0');

		// Each source gets its own labelled row instead of one flat "Quality".
		await expect(page.getByText('AllAnime', { exact: true })).toBeVisible();
		await expect(page.getByText('HiAnime', { exact: true })).toBeVisible();
		await expect(page.getByText('Quality', { exact: true })).toHaveCount(0);
	});

	test('a single source keeps the plain "Quality" label', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				sourceFixture({ source: 'allanime', quality: '1080' }),
				sourceFixture({
					source: 'allanime',
					quality: '720',
					url: 'http://127.0.0.1:9/b.mp4'
				})
			]
		});
		await page.goto('/watch/allanime:show-one-piece/1?dub=0');

		await expect(page.getByText('Quality', { exact: true })).toBeVisible();
	});

	test('the outage banner names the source that broke', async ({ page }) => {
		await mockTauri(page, {
			get_sources: reject('PROVIDER_ROTATED: bootstrap rejected')
		});
		await page.goto('/watch/allanime:show-one-piece/1?dub=0');

		await expect(page.getByTestId('provider-outage')).toBeVisible();
		await expect(page.getByText('AllAnime changed its access scheme')).toBeVisible();
	});

	test('a pre-migration bare id falls back to generic outage copy', async ({ page }) => {
		// No source segment in the id, so naming one would be a guess.
		await mockTauri(page, {
			get_sources: reject('PROVIDER_ROTATED: bootstrap rejected')
		});
		await page.goto('/watch/ReooPAxPMsHM4KPMY/1?dub=0');

		await expect(page.getByText('Streaming source changed')).toBeVisible();
	});
});
