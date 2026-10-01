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
					source: 'animegg',
					quality: '720',
					provider_name: 'Animegg',
					url: 'http://127.0.0.1:9/gg.mp4'
				})
			]
		});
		await page.goto('/watch/allanime:show-one-piece/1?dub=0');

		// Each source gets its own labelled row instead of one flat "Quality".
		await expect(page.getByText('AllAnime', { exact: true })).toBeVisible();
		await expect(page.getByText('AnimeGG', { exact: true })).toBeVisible();
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

/**
 * The point of having several sources: when the one a show was opened from is
 * down, the backend answers with links from another, and the page has to play
 * them and say so — not show an outage for a show that is in fact playable.
 */
test.describe('watch page — failover to another source', () => {
	const ANIZONE = sourceFixture({
		source: 'anizone',
		provider_name: 'AniZone',
		quality: 'auto',
		url: 'http://127.0.0.1:9/master.mp4'
	});

	test('plays links from another source and says which one', async ({ page }) => {
		// Opened from allanime; every link the backend found is AniZone's.
		await mockTauri(page, { get_sources: [ANIZONE] });
		await page.goto('/watch/allanime:show-frieren/1?dub=0');

		await expect(page.locator('video')).toBeVisible();
		await expect(page.getByTestId('provider-outage')).toHaveCount(0);
		const note = page.getByTestId('failover-note');
		await expect(note).toBeVisible();
		await expect(note).toContainText("AllAnime isn't available for this episode");
		await expect(note).toContainText('playing from AniZone');
	});

	test('names every source in use when failover merged several', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				ANIZONE,
				sourceFixture({
					source: 'animegg',
					quality: '1080',
					provider_name: 'Animegg',
					url: 'http://127.0.0.1:9/gg.mp4'
				})
			]
		});
		await page.goto('/watch/allanime:show-frieren/1?dub=0');

		await expect(page.getByTestId('failover-note')).toContainText('AniZone, AnimeGG');
		// And the picker still offers both, grouped.
		await expect(page.locator('.chips .chip')).toHaveCount(2);
	});

	test('no note when the show plays from the source it was opened from', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				sourceFixture({ source: 'allanime' }),
				sourceFixture({ ...ANIZONE, url: 'http://127.0.0.1:9/z.mp4' })
			]
		});
		await page.goto('/watch/allanime:show-frieren/1?dub=0');

		await expect(page.locator('video')).toBeVisible();
		await expect(page.getByTestId('failover-note')).toHaveCount(0);
	});

	test('no note for a pre-migration id, whose owner is unknown', async ({ page }) => {
		await mockTauri(page, { get_sources: [ANIZONE] });
		await page.goto('/watch/ReooPAxPMsHM4KPMY/1?dub=0');

		await expect(page.locator('video')).toBeVisible();
		await expect(page.getByTestId('failover-note')).toHaveCount(0);
	});

	test('a dead link from one source advances to the next source', async ({ page }) => {
		// Both URLs are unreachable, so the player walks the whole list: the
		// second source must be attempted before any error is shown.
		await mockTauri(page, {
			get_sources: [
				sourceFixture({ source: 'allanime', url: 'http://127.0.0.1:9/a.mp4' }),
				sourceFixture({ ...ANIZONE, url: 'http://127.0.0.1:9/z.mp4' })
			]
		});
		await page.goto('/watch/allanime:show-frieren/1?dub=0');

		await expect(page.locator('p.error')).toBeVisible({ timeout: 20_000 });
		// The AniZone chip ended up selected: it was the last one tried.
		await expect(page.locator('.chips .chip.active')).toContainText('AniZone');
	});

	test('everything down is still the outage screen, naming the source', async ({ page }) => {
		await mockTauri(page, { get_sources: reject('PROVIDER_ROTATED: bootstrap rejected') });
		await page.goto('/watch/allanime:show-frieren/1?dub=0');

		await expect(page.getByTestId('provider-outage')).toBeVisible();
		await expect(page.getByTestId('failover-note')).toHaveCount(0);
	});
});

test.describe('watch page — subtitles', () => {
	test("a source's default subtitle track is on without being asked", async ({ page }) => {
		// AniZone streams carry no burned-in subtitles, so its English track
		// has to start enabled or the episode plays with no text at all.
		await mockTauri(page, {
			get_sources: [
				sourceFixture({
					source: 'anizone',
					subtitles: [
						{ label: 'German', lang: 'de', url: 'https://cdn.example/1_de.ass' },
						{ label: 'English', lang: 'en', url: 'https://cdn.example/3_en.ass', default: true }
					]
				})
			]
		});
		await page.goto('/watch/anizone:mdkytdqp/1?dub=0');

		const tracks = page.locator('video track');
		await expect(tracks).toHaveCount(2);
		await expect(page.locator('video track[srclang="en"]')).toHaveAttribute('default', '');
		await expect(page.locator('video track[srclang="de"]')).not.toHaveAttribute('default');
		// Routed through the media server, which converts ASS to WebVTT.
		const src = await page.locator('video track[srclang="en"]').getAttribute('src');
		expect(src).toContain('http://mock.localhost/media?url=');
		expect(decodeURIComponent(src ?? '')).toContain('3_en.ass');
	});

	test('tracks without the flag stay off (hard-subbed sources)', async ({ page }) => {
		await mockTauri(page, {
			get_sources: [
				sourceFixture({
					subtitles: [{ label: 'English', lang: 'en', url: 'https://cdn.example/en.vtt' }]
				})
			]
		});
		await page.goto(WATCH_URL);

		await expect(page.locator('video track')).toHaveCount(1);
		await expect(page.locator('video track')).not.toHaveAttribute('default');
	});

	test('the external subtitle picker accepts ASS files', async ({ page }) => {
		await mockTauri(page, { get_sources: [sourceFixture()] });
		await page.goto(WATCH_URL);

		await expect(page.locator('input[type="file"]')).toHaveAttribute(
			'accept',
			'.srt,.vtt,.ass,.ssa'
		);
	});
});
