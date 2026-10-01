/**
 * Regression tests for the x-aa-boot signature tokeniser.
 *
 * The 2026-09-14 outage (issue #6) was a hard-coded `split(':')` meeting a
 * `+`-joined signature with a reordered field list. Each recorded signature
 * below is a real capture; the synthetic cases pin the failure modes.
 *
 * Run: npm run test:oracle
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { deriveBootTemplate } from './boot-template.ts';

const KNOWN = (buildId: string, epoch: string): Array<[string, string]> => [
	['build_id', buildId],
	['epoch', epoch],
	['lane', 'k7'],
	['referer_host', 'mkissa.to']
];

test('buildId 166 — colon-joined, key_group first (pre-2026-09-14)', () => {
	const t = deriveBootTemplate('mkissa:k7:2958:mkissa.to:166', KNOWN('166', '2958'));
	assert.equal(t.separator, ':');
	assert.equal(t.key_group, 'mkissa');
	assert.equal(t.template, '{key_group}:{lane}:{epoch}:{referer_host}:{build_id}');
});

test('buildId 173 — plus-joined, key_group fourth (the outage)', () => {
	const t = deriveBootTemplate('k7+2959+mkissa.to+mkissa+173', KNOWN('173', '2959'));
	assert.equal(t.separator, '+');
	assert.equal(t.key_group, 'mkissa');
	assert.equal(t.template, '{lane}+{epoch}+{referer_host}+{key_group}+{build_id}');
});

test('buildId 177 — slash-joined, key_group first again (2026-10-01)', () => {
	// The rotation after the outage: a third separator and a fifth order,
	// derived with no code change — the point of inferring both.
	const t = deriveBootTemplate('mkissa/k7/mkissa.to/177/2960', KNOWN('177', '2960'));
	assert.equal(t.separator, '/');
	assert.equal(t.key_group, 'mkissa');
	assert.equal(t.template, '{key_group}/{lane}/{referer_host}/{build_id}/{epoch}');
});

test('a separator never seen before is inferred, not guessed from a list', () => {
	const t = deriveBootTemplate('k7§2960§mkissa.to§mkissa§174', KNOWN('174', '2960'));
	assert.equal(t.separator, '§');
	assert.equal(t.template, '{lane}§{epoch}§{referer_host}§{key_group}§{build_id}');
});

test('a dot separator does not split referer_host apart', () => {
	// '.' is in the fallback list and appears inside mkissa.to; splitting on it
	// must be rejected in favour of the real separator.
	const t = deriveBootTemplate('mkissa.to|k7|2958|mkissa|166', KNOWN('166', '2958'));
	assert.equal(t.separator, '|');
	assert.equal(t.template, '{referer_host}|{lane}|{epoch}|{key_group}|{build_id}');
});

test('genuine scheme drift throws (an extra unknown field)', () => {
	assert.throws(
		() => deriveBootTemplate('k7+2959+mkissa.to+mkissa+deadbeef+173', KNOWN('173', '2959')),
		/does not fit <known fields \+ one key_group>/
	);
});

test('a field vanishing from the signature throws and names it', () => {
	assert.throws(
		() => deriveBootTemplate('k7+2959+mkissa+173', KNOWN('173', '2959')),
		/referer_host/
	);
});

test('the reported error lists the separators that were tried', () => {
	assert.throws(() => deriveBootTemplate('totally-unrelated', KNOWN('173', '2959')), /separators tried/);
});
