// Copy the rotatable values from allanime-config.json into the baked-in
// defaults in core/src/provider/allanime/constants.rs. Each const is a single
// `pub const NAME: T = <literal>;` line, so this is a dumb, reviewable regex
// rewrite — the `shipped_sample_matches_defaults` unit test guarantees the two
// stay in lockstep.
import { readFileSync, writeFileSync } from 'node:fs';

const cfgPath = process.argv[2] ?? 'allanime-config.json';
const rsPath = process.argv[3] ?? 'core/src/provider/allanime/constants.rs';
const cfg = JSON.parse(readFileSync(cfgPath, 'utf8'));
let rs = readFileSync(rsPath, 'utf8');

const strConsts = {
	BUILD_ID: 'build_id',
	QD_MASK_HEX: 'qd_mask_hex',
	EPISODE_LANE: 'episode_lane',
	KEY_GROUP: 'key_group',
	REFERER_HOST: 'referer_host',
	REFERER: 'referer',
	API_URL: 'api_url',
	BOOTSTRAP_URL: 'bootstrap_url',
	BOOT_LABEL: 'boot_label',
	BOOT_SIG_TEMPLATE: 'boot_sig_template',
	AA_REQ_SEED_TEMPLATE: 'aa_req_seed_template'
};
const changed = [];
for (const [name, key] of Object.entries(strConsts)) {
	if (cfg[key] == null) continue;
	const re = new RegExp(`(pub const ${name}: &str =\\s*)"[^"]*";`);
	if (!re.test(rs)) {
		console.error(`apply-constants: ${name} not found in ${rsPath}`);
		process.exit(1);
	}
	const next = rs.replace(re, `$1${JSON.stringify(cfg[key])};`);
	if (next !== rs) changed.push(name);
	rs = next;
}
if (cfg.epoch_bucket_ms) {
	const lit = String(cfg.epoch_bucket_ms).replace(/\B(?=(\d{3})+(?!\d))/g, '_');
	const re = /(pub const EPOCH_BUCKET_MS: u128 =\s*)[\d_]+;/;
	const next = rs.replace(re, `$1${lit};`);
	if (next !== rs) changed.push('EPOCH_BUCKET_MS');
	rs = next;
}
writeFileSync(rsPath, rs);
console.log(changed.length ? `updated ${changed.join(', ')}` : 'constants already match');
