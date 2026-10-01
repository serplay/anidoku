/**
 * Tokenises allanime's `x-aa-boot` outer signature into a config template.
 *
 * The signature is a flat join of known fields plus one opaque `key_group`,
 * e.g. `mkissa:k7:2958:mkissa.to:166`. Both the separator and the field order
 * rotate: on 2026-09-14 allanime moved to `k7+2959+mkissa.to+mkissa+173`, and
 * the oracle's hard-coded `split(':')` turned a routine rotation into a
 * 4.5-day outage (issue #6). Nothing here may assume either.
 *
 * Kept in its own module so it is importable — and therefore testable —
 * without pulling in derive.ts, which launches Playwright at import time.
 */

/** Separators to fall back on when inference is inconclusive. */
const FALLBACK_SEPARATORS = [':', '+', '|', ',', ';', '-', '_', '/', '~', '.', '*', '!', '@', '#', '^', '&', '='];

export type BootTemplate = {
	/** e.g. `{lane}+{epoch}+{referer_host}+{key_group}+{build_id}` */
	template: string;
	/** The one part that matched no known field. */
	key_group: string;
	/** The separator the live client used. */
	separator: string;
};

/**
 * Separators worth trying, best guess first: any character present in the
 * signature but in none of the known field values is almost certainly the
 * join character, so try those before the fixed list. This is what makes the
 * function survive a separator nobody has seen yet.
 */
function candidateSeparators(outerData: string, known: Array<[string, string]>): string[] {
	const inKnown = new Set<string>();
	for (const [, v] of known) for (const ch of v) inKnown.add(ch);
	const inferred = [...new Set(outerData)].filter((ch) => !inKnown.has(ch));
	return [...new Set([...inferred, ...FALLBACK_SEPARATORS])];
}

/** Try one separator. Returns null when the split doesn't fit the shape. */
function tokenise(outerData: string, known: Array<[string, string]>, sep: string): BootTemplate | null {
	const parts = outerData.split(sep);
	// Exactly the known fields plus one key_group, each field used once.
	if (parts.length !== known.length + 1) return null;
	const used = new Set<string>();
	const unknown: string[] = [];
	const tpl = parts.map((p) => {
		const hit = known.find(([n, v]) => v === p && !used.has(n));
		if (hit) {
			used.add(hit[0]);
			return `{${hit[0]}}`;
		}
		unknown.push(p);
		return '{key_group}';
	});
	if (unknown.length !== 1 || used.size !== known.length) return null;
	// An empty key_group means we split inside a field, not between fields.
	if (!unknown[0]) return null;
	return { template: tpl.join(sep), key_group: unknown[0], separator: sep };
}

/**
 * Derive the `boot_sig_template` + `key_group` from a captured signature.
 * Separator- and order-agnostic. Throws when no separator yields
 * <every known field + exactly one key_group> — that is genuine scheme drift
 * and needs a human.
 */
export function deriveBootTemplate(outerData: string, known: Array<[string, string]>): BootTemplate {
	const tried = candidateSeparators(outerData, known);
	for (const sep of tried) {
		const hit = tokenise(outerData, known, sep);
		if (hit) return hit;
	}
	const missing = known.map(([n]) => n).filter((n) => !outerData.includes(known.find(([k]) => k === n)![1]));
	throw new Error(
		`signature ${JSON.stringify(outerData)} does not fit <known fields + one key_group> ` +
			`(fields not present at all: ${JSON.stringify(missing)}; separators tried: ${JSON.stringify(tried)})`
	);
}
