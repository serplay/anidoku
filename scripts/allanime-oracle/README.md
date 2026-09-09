# allanime rotation oracle

allanime (mkissa.to) rotates its anti-scraping crypto every few weeks: build id,
key mask, and — since 2026-09 — the shape of the `x-aa-boot` signature. Each
rotation used to mean hand-deobfuscating a renamed bundle. This script instead
runs the real web client in headless Chromium with `SubtleCrypto` hooked and
captures the values from what the client *does*:

| captured                                  | becomes                                  |
| ----------------------------------------- | ---------------------------------------- |
| `importKey(HMAC)` raw key                 | `qd_mask_hex`                            |
| first `sign()` data (`<label><buildId>`)  | `boot_label`                             |
| second `sign()` data                      | `boot_sig_template` + `key_group`        |
| bootstrap request `?buildId=&k=`          | `build_id`, `episode_lane`, `*_url`      |
| bootstrap response `epochMs`              | `epoch_bucket_ms`                        |
| `importKey(AES-GCM)` key == partB⊕mask    | key-derivation cross-check               |

It only writes `allanime-config.json` after recomputing the exact `x-aa-boot`
header the client sent from the derived values. Anything that doesn't fit the
known shapes exits 2 ("scheme drift") for a human to look at.

```sh
npm run oracle                 # derive + verify + write allanime-config.json
npm run oracle -- --dry-run    # print only
npm run oracle:apply           # copy the JSON values into constants.rs
cargo test -p anidoku-core --test allanime_live -- --ignored --nocapture
```

`provider-health.yml` runs exactly this sequence when the scheduled live test
fails, and opens an auto-merging PR when the live test passes with the new
values. Merging publishes the config (constants.rs `REMOTE_CONFIG_URL` points
at this file on `master`), so installed apps self-heal on their next play.
