import { defineConfig, devices } from '@playwright/test';

/**
 * E2E tests drive the SvelteKit frontend in a real browser with the Tauri IPC
 * layer mocked (see tests/e2e/tauri-mock.ts). This exercises the frontend logic
 * — routing, search, the player's source auto-advance / no-sources handling —
 * without needing the Rust backend or a running Tauri shell.
 */
export default defineConfig({
	testDir: './tests/e2e',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 2 : 0,
	workers: process.env.CI ? 1 : undefined,
	reporter: process.env.CI ? [['github'], ['list']] : 'list',
	use: {
		baseURL: 'http://localhost:1420',
		trace: 'on-first-retry'
	},
	projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
	webServer: {
		command: 'npm run dev',
		url: 'http://localhost:1420',
		reuseExistingServer: !process.env.CI,
		timeout: 120_000
	}
});
