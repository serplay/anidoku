import { test, expect } from '@playwright/test';
import { mockTauri } from './tauri-mock';

test.describe('app shell', () => {
	test('boots and renders the brand with Tauri mocked', async ({ page }) => {
		await mockTauri(page);
		await page.goto('/');
		// The layout brand is always present once the SPA has hydrated.
		await expect(page.locator('a.brand')).toHaveText('AniDoku');
		// isDesktop() must report true under the mock (no "desktop app only" errors).
		expect(await page.evaluate(() => '__TAURI_INTERNALS__' in window)).toBe(true);
	});

	test('navigates to the search page', async ({ page }) => {
		await mockTauri(page);
		await page.goto('/search');
		// The search controls render: the query input and the Dub toggle. (The
		// submit button's label toggles to "Searching…", so assert on stable
		// elements instead.)
		await expect(page.getByPlaceholder('Search anime…')).toBeVisible();
		await expect(page.getByText('Dub', { exact: false })).toBeVisible();
	});
});
