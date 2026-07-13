import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},

			// Static adapter in SPA mode: Tauri serves the built assets from disk,
			// so there is no server runtime. All routing happens client-side.
			adapter: adapter({
				fallback: 'index.html'
			})
		})
	],

	// Tauri expects a fixed dev-server port and handles its own screen output.
	clearScreen: false,
	server: {
		port: 1420,
		strictPort: true
	},
	envPrefix: ['VITE_', 'TAURI_ENV_']
});
