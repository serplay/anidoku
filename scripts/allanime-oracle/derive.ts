/**
 * allanime rotation oracle.
 *
 * Drives the real allanime web client (mkissa.to) in headless Chromium with
 * WebCrypto hooked, and derives every rotatable value in allanime-config.json
 * from what the client actually *does* — not from reading its obfuscated
 * bundle. Identifier renames, string-array rotations, and anti-debug tricks
 * therefore can't break it; only a genuine change of algorithm can, and that
 * is reported as "scheme drift" (exit 2) instead of a wrong config.
 *
 * What is captured:
 *   - SubtleCrypto.importKey(HMAC) + sign(): the two chained HMACs behind the
 *     `x-aa-boot` header. The inner key IS the QD mask; the inner data is
 *     `<label><buildId>`; the outer data is the signature string.
 *   - SubtleCrypto.importKey(AES-GCM): the episode key, cross-checked as
 *     `partB XOR mask`.
 *   - The bootstrap request (buildId, lane, x-aa-boot) and response (epoch,
 *     epochMs, partB).
 *
 * Self-check before anything is written: HMAC(HMAC(mask, label+buildId), sig)
 * recomputed in Node must equal the `x-aa-boot` the client sent.
 *
 * Usage: node --experimental-strip-types scripts/allanime-oracle/derive.ts
 *          [--out allanime-config.json] [--dry-run] [--show <providerId>]
 *          [--site https://mkissa.to] [--headed]
 * Exit:  0 derived + verified (file written unless --dry-run)
 *        2 scheme drift — captures don't fit the known shapes; needs a human
 *        3 site unreachable / blocked / no bootstrap observed
 */
import { chromium, type Page, type Request, type Response } from '@playwright/test';
import { createHmac } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync, appendFileSync } from 'node:fs';
import { deriveBootTemplate } from './boot-template.ts';

type Sign = { keyHex: string; data: string };
type Capture = {
	hmacKeys: string[];
	signs: Sign[];
	aesKeys: string[];
	bootstrapReq?: { url: string; headers: Record<string, string> };
	bootstrapRes?: { epoch: number; partB: string; epochMs?: number; graceMs?: number; k?: string };
	apiOrigins: Set<string>;
	pageOrigin?: string;
};

const args = new Map<string, string>();
for (let i = 2; i < process.argv.length; i++) {
	const a = process.argv[i];
	if (!a.startsWith('--')) continue;
	const next = process.argv[i + 1];
	if (next && !next.startsWith('--')) {
		args.set(a.slice(2), next);
		i++;
	} else args.set(a.slice(2), 'true');
}
const OUT = args.get('out') ?? 'allanime-config.json';
const DRY = args.has('dry-run');
const SITE = args.get('site') ?? 'https://mkissa.to';
const SHOW = args.get('show') ?? 'ReooPAxPMsHM4KPMY'; // One Piece — long-lived
const HEADED = args.has('headed');
const UA =
	'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36';

function log(msg: string) {
	process.stderr.write(`[oracle] ${msg}\n`);
}
function fail(code: 2 | 3, msg: string): never {
	log(`FAIL: ${msg}`);
	ghOutput({ status: code === 2 ? 'scheme-drift' : 'unreachable', reason: msg });
	process.exit(code);
}
function ghOutput(kv: Record<string, string | number | boolean>) {
	const f = process.env.GITHUB_OUTPUT;
	if (!f) return;
	for (const [k, v] of Object.entries(kv)) appendFileSync(f, `${k}=${String(v)}\n`);
}
const hmac = (keyHex: string, data: string) =>
	createHmac('sha256', Buffer.from(keyHex, 'hex')).update(data).digest('hex');

async function capture(): Promise<Capture> {
	const cap: Capture = { hmacKeys: [], signs: [], aesKeys: [], apiOrigins: new Set() };
	const browser = await chromium.launch({
		headless: !HEADED,
		args: ['--disable-blink-features=AutomationControlled']
	});
	try {
		const ctx = await browser.newContext({ userAgent: UA, viewport: { width: 1280, height: 800 } });
		const page: Page = await ctx.newPage();
		// The bundle overrides `console`, so hook output travels over a binding.
		let lastHmacKey = '';
		await page.exposeBinding('__oracle', (_src, kind: string, a: string, b: string) => {
			if (kind === 'hmacKey') {
				lastHmacKey = a;
				cap.hmacKeys.push(a);
			} else if (kind === 'sign') cap.signs.push({ keyHex: lastHmacKey, data: a });
			else if (kind === 'aesKey') cap.aesKeys.push(a);
			void b;
		});
		await page.addInitScript(() => {
			const hex = (b: ArrayBuffer | ArrayBufferView) => {
				const u = ArrayBuffer.isView(b)
					? new Uint8Array(b.buffer, b.byteOffset, b.byteLength)
					: new Uint8Array(b);
				return Array.from(u)
					.map((x) => x.toString(16).padStart(2, '0'))
					.join('');
			};
			const txt = (b: ArrayBuffer | ArrayBufferView) => {
				const u = ArrayBuffer.isView(b)
					? new Uint8Array(b.buffer, b.byteOffset, b.byteLength)
					: new Uint8Array(b);
				return new TextDecoder().decode(u);
			};
			const w = window as unknown as { __oracle: (k: string, a: string, b?: string) => void };
			const S = SubtleCrypto.prototype;
			const oImport = S.importKey;
			S.importKey = function (this: SubtleCrypto, ...a: unknown[]) {
				try {
					const alg = a[2] as { name?: string } | string;
					const name = typeof alg === 'string' ? alg : alg?.name;
					const key = a[1] as ArrayBuffer;
					if (name === 'HMAC') w.__oracle('hmacKey', hex(key));
					else if (name === 'AES-GCM') w.__oracle('aesKey', hex(key));
				} catch {
					/* never break the page */
				}
				return (oImport as (...x: unknown[]) => Promise<CryptoKey>).apply(this, a);
			} as typeof S.importKey;
			const oSign = S.sign;
			S.sign = function (this: SubtleCrypto, ...a: unknown[]) {
				try {
					w.__oracle('sign', txt(a[2] as ArrayBuffer));
				} catch {
					/* ignore */
				}
				return (oSign as (...x: unknown[]) => Promise<ArrayBuffer>).apply(this, a);
			} as typeof S.sign;
		});
		page.on('request', (r: Request) => {
			const u = r.url();
			if (/\/client-crypto\/v1\/bootstrap/.test(u)) {
				cap.bootstrapReq = { url: u, headers: r.headers() };
			} else if (/\/api(\?|$)/.test(u) && r.headers()['x-build-id']) {
				cap.apiOrigins.add(new URL(u).origin);
			}
		});
		page.on('response', async (r: Response) => {
			if (/\/client-crypto\/v1\/bootstrap/.test(r.url()) && r.ok()) {
				try {
					cap.bootstrapRes = (await r.json()) as Capture['bootstrapRes'];
				} catch {
					/* ignore */
				}
			}
		});
		const url = `${SITE}/anime/${SHOW}`;
		log(`opening ${url}`);
		const resp = await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });
		if (!resp || !resp.ok()) fail(3, `page load failed: HTTP ${resp?.status()}`);
		cap.pageOrigin = new URL(page.url()).origin;
		// The show page bootstraps crypto on load; give it time (and one scroll
		// nudge for lazy paths).
		for (let i = 0; i < 30 && !cap.bootstrapRes; i++) {
			await page.waitForTimeout(1000);
			if (i === 5) await page.mouse.wheel(0, 600);
		}
		await page.waitForTimeout(1500);
		await ctx.close();
	} finally {
		await browser.close();
	}
	return cap;
}

type Config = Record<string, unknown> & {
	build_id: string;
	qd_mask_hex: string;
	boot_label: string;
	boot_sig_template: string;
	key_group: string;
	referer_host: string;
	referer: string;
	episode_lane: string;
	epoch_bucket_ms: number;
	api_url: string;
	bootstrap_url: string;
};

function derive(cap: Capture): Config {
	if (!cap.bootstrapReq || !cap.bootstrapRes) {
		fail(3, `no bootstrap observed (signs=${cap.signs.length}, hmacKeys=${cap.hmacKeys.length})`);
	}
	const bu = new URL(cap.bootstrapReq.url);
	const build_id = cap.bootstrapReq.headers['x-build-id'] ?? bu.searchParams.get('buildId') ?? '';
	const lane = bu.searchParams.get('k') ?? '';
	const sentBoot = cap.bootstrapReq.headers['x-aa-boot'] ?? '';
	if (!build_id || !lane || !sentBoot) {
		fail(2, `bootstrap request shape changed: ${cap.bootstrapReq.url} headers=${JSON.stringify(cap.bootstrapReq.headers)}`);
	}
	const epoch = cap.bootstrapRes.epoch;

	// Find the chained pair: inner.sign -> outer.key, outer.sign -> x-aa-boot.
	let inner: Sign | undefined, outer: Sign | undefined;
	for (const a of cap.signs) {
		if (a.keyHex.length !== 64) continue;
		const derivedKey = hmac(a.keyHex, a.data);
		const b = cap.signs.find((s) => s.keyHex === derivedKey && hmac(s.keyHex, s.data) === sentBoot);
		if (b) {
			inner = a;
			outer = b;
			break;
		}
	}
	if (!inner || !outer) {
		fail(2, `could not find the HMAC chain reproducing x-aa-boot=${sentBoot.slice(0, 12)}… (signs: ${JSON.stringify(cap.signs.map((s) => s.data))})`);
	}
	if (!inner.data.endsWith(build_id)) {
		fail(2, `inner HMAC data "${inner.data}" does not end with buildId ${build_id}`);
	}
	const boot_label = inner.data.slice(0, inner.data.length - build_id.length);
	const qd_mask_hex = inner.keyHex;

	// Tokenise the outer signature into a template.
	const referer_host = new URL(cap.pageOrigin ?? SITE).hostname.replace(/^www\./, '');
	const known: Array<[string, string]> = [
		['build_id', build_id],
		['epoch', String(epoch)],
		['lane', lane],
		['referer_host', referer_host]
	];
	// Separator and field order both rotate — infer them (see boot-template.ts).
	let key_group: string, boot_sig_template: string;
	try {
		const t = deriveBootTemplate(outer.data, known);
		({ key_group, template: boot_sig_template } = t);
		log(`boot signature template: ${boot_sig_template} (separator ${JSON.stringify(t.separator)})`);
	} catch (e) {
		fail(2, e instanceof Error ? e.message : String(e));
	}

	// Key derivation cross-check (warn-only: the show page may not import the
	// episode key before we stop capturing).
	const partB = Buffer.from(cap.bootstrapRes.partB, 'base64');
	const mask = Buffer.from(qd_mask_hex, 'hex');
	if (partB.length === 32) {
		const xored = Buffer.alloc(32);
		for (let i = 0; i < 32; i++) xored[i] = partB[i] ^ mask[i];
		if (cap.aesKeys.length && !cap.aesKeys.includes(xored.toString('hex'))) {
			fail(2, `partB XOR mask is not among the AES-GCM keys the client imported — key derivation changed`);
		}
		log(cap.aesKeys.length ? 'AES key derivation (partB XOR mask) verified' : 'no AES key captured; XOR check skipped');
	}

	const epoch_bucket_ms = cap.bootstrapRes.epochMs ?? 0;
	if (epoch_bucket_ms) {
		const expected = Math.floor(Date.now() / epoch_bucket_ms);
		if (Math.abs(expected - epoch) > 1) fail(2, `epoch ${epoch} inconsistent with epochMs ${epoch_bucket_ms} (expected ~${expected})`);
	}

	return {
		build_id,
		qd_mask_hex,
		boot_label,
		boot_sig_template,
		key_group,
		referer_host,
		referer: cap.pageOrigin ?? SITE,
		episode_lane: lane,
		epoch_bucket_ms,
		api_url: `${bu.origin}/api`,
		bootstrap_url: `${bu.origin}${bu.pathname}`
	};
}

async function main() {
	const cap = await capture();
	log(`captured: ${cap.signs.length} HMAC signs, ${cap.aesKeys.length} AES keys, bootstrap=${cap.bootstrapRes ? 'yes' : 'no'}`);
	const d = derive(cap);
	// Final proof, independent of the search above.
	const check = hmac(
		hmac(d.qd_mask_hex, d.boot_label + d.build_id),
		d.boot_sig_template
			.replace('{build_id}', d.build_id)
			.replace('{key_group}', d.key_group)
			.replace('{referer_host}', d.referer_host)
			.replace('{epoch}', String(cap.bootstrapRes!.epoch))
			.replace('{lane}', d.episode_lane)
	);
	if (check !== cap.bootstrapReq!.headers['x-aa-boot']) fail(2, 'self-check failed: recomputed x-aa-boot differs');
	log(`verified x-aa-boot for buildId ${d.build_id} (epoch ${cap.bootstrapRes!.epoch})`);

	const existing: Record<string, unknown> = existsSync(OUT) ? JSON.parse(readFileSync(OUT, 'utf8')) : {};
	const merged: Record<string, unknown> = { ...existing };
	const changed: string[] = [];
	for (const [k, v] of Object.entries(d)) {
		if (k === 'epoch_bucket_ms' && !v) continue; // server didn't say; keep existing
		if (JSON.stringify(existing[k]) !== JSON.stringify(v)) changed.push(`${k}: ${JSON.stringify(existing[k])} -> ${JSON.stringify(v)}`);
		merged[k] = v;
	}
	const summary = { build_id: d.build_id, changed: changed.length > 0, changes: changed };
	process.stdout.write(JSON.stringify(summary, null, 2) + '\n');
	ghOutput({ status: 'ok', build_id: d.build_id, changed: changed.length > 0 });
	if (!DRY) {
		writeFileSync(OUT, JSON.stringify(merged, null, 2) + '\n');
		log(`${changed.length ? 'updated' : 'unchanged'} ${OUT}`);
	}
}

main().catch((e) => {
	log(`unexpected: ${e instanceof Error ? e.stack ?? e.message : String(e)}`);
	ghOutput({ status: 'unreachable', reason: String(e) });
	process.exit(3);
});
