//! Link deobfuscation and payload decryption, ported from ani-cli 4.14.1.

use crate::{Error, Result};
use aes::cipher::{KeyIvInit, StreamCipher};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};

type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

/// Bucket (ms) the aaReq timestamp is floored to (web client `Dm = 5 * 6e4`).
const TS_BUCKET_MS: u128 = 5 * 60 * 1000;

/// Deobfuscate an allanime `sourceUrl`.
///
/// Obfuscated ids start with `--` followed by hex pairs; each decoded byte is
/// XORed with 0x38. (ani-cli expresses this as a giant sed lookup table in
/// `provider_init`; the table is exactly "hex decode, then XOR 0x38".)
/// Afterwards the first `/clock` is rewritten to `/clock.json`, matching
/// ani-cli's final `sed "s/\/clock/\/clock\.json/"`.
///
/// Non-obfuscated ids are returned unchanged.
pub fn deobfuscate_source_url(source_url: &str) -> Result<String> {
    let Some(hex_part) = source_url.strip_prefix("--") else {
        return Ok(source_url.to_string());
    };
    let bytes = hex::decode(hex_part.trim())
        .map_err(|e| Error::Decrypt(format!("bad obfuscated source url hex: {e}")))?;
    let decoded: Vec<u8> = bytes.iter().map(|b| b ^ 0x38).collect();
    let s = String::from_utf8(decoded)
        .map_err(|e| Error::Decrypt(format!("obfuscated source url is not utf-8: {e}")))?;
    Ok(s.replacen("/clock", "/clock.json", 1))
}

/// AES-256-CTR key: SHA-256 of the allanime passphrase (ani-cli `$allanime_key`).
///
/// This was the whole story until allanime moved to per-epoch bootstrap keys
/// (see [`derive_key_xor`]). Retained only for the legacy roundtrip test.
#[cfg(test)]
pub fn derive_key(passphrase: &str) -> [u8; 32] {
    let digest = Sha256::digest(passphrase.as_bytes());
    digest.into()
}

/// Derive the current AES-256 key from a bootstrap response: the base64
/// `partB` XORed byte-for-byte with the static `mask_hex` (`qd` in the web
/// client). Both are 32 bytes.
pub fn derive_key_xor(part_b_b64: &str, mask_hex: &str) -> Result<[u8; 32]> {
    let part_b = base64::engine::general_purpose::STANDARD
        .decode(part_b_b64.trim())
        .map_err(|e| Error::Decrypt(format!("bad base64 in bootstrap partB: {e}")))?;
    let mask = hex::decode(mask_hex).map_err(|e| Error::Decrypt(format!("bad mask hex: {e}")))?;
    if part_b.len() != 32 || mask.len() != 32 {
        return Err(Error::Decrypt(format!(
            "key material wrong length: partB={} mask={}",
            part_b.len(),
            mask.len()
        )));
    }
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = part_b[i] ^ mask[i];
    }
    Ok(key)
}

/// Hex-encoded SHA-256 of `data`. Used to compute the persisted-query hash from
/// our own query text (Apollo APQ) so query and hash can never drift.
pub fn sha256_hex(data: &str) -> String {
    hex::encode(Sha256::digest(data.as_bytes()))
}

/// HMAC-SHA256 (`bg` in the web client's crypto bundle). Hand-rolled to avoid a
/// new dependency; standard construction over SHA-256's 64-byte block.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= block_key[i];
        opad[i] ^= block_key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// Sign the `x-aa-boot` header the bootstrap endpoint now requires (web client
/// `PS`). Two chained HMAC-SHA256s over the static mask, hex-encoded:
/// `hex(HMAC(HMAC(mask, "aa-boot:<buildId>"), "<buildId>:<keyGroup>:<refererHost>:<epoch>:<lane>"))`.
pub fn sign_aa_boot(
    mask_hex: &str,
    build_id: &str,
    key_group: &str,
    referer_host: &str,
    epoch: u64,
    lane: &str,
) -> Result<String> {
    let mask = hex::decode(mask_hex).map_err(|e| Error::Decrypt(format!("bad mask hex: {e}")))?;
    let inner_key = hmac_sha256(
        &mask,
        format!("{}{build_id}", super::AA_BOOT_PREFIX).as_bytes(),
    );
    let sig = format!("{build_id}:{key_group}:{referer_host}:{epoch}:{lane}");
    let mac = hmac_sha256(&inner_key, sig.as_bytes());
    Ok(hex::encode(mac))
}

/// Sign an `aaReq` token the `episode(...)` GraphQL query now requires.
///
/// Mirrors the web client (`r2`): build a compact JSON payload, derive a 12-byte
/// nonce as `SHA-256("epoch:buildId:qh:ts:lane")[..12]`, AES-256-GCM encrypt the
/// payload, and base64 the envelope `0x01 || nonce(12) || ciphertext+tag`.
/// `now_ms` is floored to a 5-minute bucket so the token stays stable within a
/// window and the server accepts it as fresh.
pub fn sign_aa_req(
    key: &[u8; 32],
    epoch: u64,
    build_id: &str,
    query_hash: &str,
    lane: &str,
    now_ms: u128,
) -> Result<String> {
    let ts = (now_ms / TS_BUCKET_MS) * TS_BUCKET_MS;
    // Field order matches the web client's JSON.stringify; the server parses by
    // key so order is not load-bearing, but keep it identical to be safe.
    let payload = format!(
        r#"{{"v":1,"ts":{ts},"epoch":{epoch},"buildId":"{build_id}","qh":"{query_hash}","k":"{lane}"}}"#
    );

    let seed = format!("{epoch}:{build_id}:{query_hash}:{ts}:{lane}");
    let nonce_bytes = Sha256::digest(seed.as_bytes());
    let nonce = Nonce::from_slice(&nonce_bytes[..12]);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let ciphertext = cipher
        .encrypt(nonce, payload.as_bytes())
        .map_err(|e| Error::Decrypt(format!("aaReq encrypt failed: {e}")))?;

    let mut envelope = Vec::with_capacity(1 + 12 + ciphertext.len());
    envelope.push(0x01);
    envelope.extend_from_slice(&nonce_bytes[..12]);
    envelope.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(&envelope))
}

/// Decrypt a `tobeparsed` payload from the episode-sources API response.
///
/// Layout of the base64-decoded blob (ani-cli `process_response`):
/// ```text
/// byte 0        : version/format marker (skipped)
/// bytes 1..13   : 12-byte IV
/// bytes 13..N-16: ciphertext
/// bytes N-16..N : auth tag (ignored; ani-cli decrypts as raw CTR)
/// ```
/// The CTR counter block is `IV || 0x00000002` (AES-GCM's convention of
/// starting payload encryption at counter 2, decrypted here as plain CTR).
pub fn decrypt_tobeparsed(payload_b64: &str, key: &[u8; 32]) -> Result<String> {
    let blob = base64::engine::general_purpose::STANDARD
        .decode(payload_b64.trim())
        .map_err(|e| Error::Decrypt(format!("bad base64 in tobeparsed: {e}")))?;

    if blob.len() < 13 + 16 {
        return Err(Error::Decrypt(format!(
            "tobeparsed blob too short: {} bytes",
            blob.len()
        )));
    }

    let mut counter_block = [0u8; 16];
    counter_block[..12].copy_from_slice(&blob[1..13]);
    counter_block[12..].copy_from_slice(&[0x00, 0x00, 0x00, 0x02]);

    let mut plaintext = blob[13..blob.len() - 16].to_vec();
    let mut cipher = Aes256Ctr::new(key.into(), &counter_block.into());
    cipher.apply_keystream(&mut plaintext);

    String::from_utf8(plaintext)
        .map_err(|e| Error::Decrypt(format!("decrypted payload is not utf-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn key_derivation_matches_ani_cli() {
        // ani-cli: printf 'Xot36i3lK3:v1' | openssl dgst -sha256
        let key = derive_key("Xot36i3lK3:v1");
        assert_eq!(
            hex::encode(key),
            "a254aa27c410f297bd04ba33a0c0df7ff4e706bf3ae27271c6703f84e750f552"
        );
    }

    #[test]
    fn deobfuscate_known_mappings() {
        // Spot-check pairs straight from ani-cli's sed table.
        let cases = [
            ("79", "A"),
            ("08", "0"),
            ("09", "1"),
            ("5a", "b"),
            ("17", "/"),
            ("02", ":"),
            ("54", "l"),
            ("07", "?"),
            ("05", "="),
            ("1e", "&"),
        ];
        for (hex_pair, ch) in cases {
            assert_eq!(
                deobfuscate_source_url(&format!("--{hex_pair}")).unwrap(),
                ch,
                "hex {hex_pair} should decode to {ch}"
            );
        }
    }

    #[test]
    fn deobfuscate_full_path_and_clock_rewrite() {
        // "/apivtwo/clock?id=abc" XOR 0x38, hex-encoded:
        let path = "/apivtwo/clock?id=abc";
        let obfuscated: String = path.bytes().map(|b| format!("{:02x}", b ^ 0x38)).collect();
        let result = deobfuscate_source_url(&format!("--{obfuscated}")).unwrap();
        assert_eq!(result, "/apivtwo/clock.json?id=abc");
    }

    #[test]
    fn plain_urls_pass_through() {
        let url = "https://tools.fast4speed.rsvp/video/xyz";
        assert_eq!(deobfuscate_source_url(url).unwrap(), url);
        // No /clock rewrite for non-obfuscated ids (matches ani-cli, which
        // only applies the sed to decoded "--" ids).
        let clock = "/apivtwo/clock?id=abc";
        assert_eq!(deobfuscate_source_url(clock).unwrap(), clock);
    }

    #[test]
    fn tobeparsed_roundtrip() {
        // CTR is symmetric, so we build a fixture by "encrypting" a known
        // GraphQL response with the same layout ani-cli decodes:
        // [1 marker byte][12-byte IV][ciphertext][16-byte tag].
        let key = derive_key("Xot36i3lK3:v1");
        let plaintext = r#"{"data":{"episode":{"episodeString":"1","sourceUrls":[{"sourceUrl":"--1748","sourceName":"Default"}]}}}"#;

        let iv: [u8; 12] = [7, 1, 4, 2, 0, 9, 3, 3, 5, 8, 6, 2];
        let mut counter_block = [0u8; 16];
        counter_block[..12].copy_from_slice(&iv);
        counter_block[12..].copy_from_slice(&[0, 0, 0, 2]);

        let mut ct = plaintext.as_bytes().to_vec();
        let mut cipher = Aes256Ctr::new((&key).into(), &counter_block.into());
        cipher.apply_keystream(&mut ct);

        let mut blob = vec![0x01u8];
        blob.extend_from_slice(&iv);
        blob.extend_from_slice(&ct);
        blob.extend_from_slice(&[0xAA; 16]); // fake auth tag, must be ignored

        let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
        let out = decrypt_tobeparsed(&b64, &key).unwrap();
        assert_eq!(out, plaintext);
    }

    #[test]
    fn tobeparsed_rejects_short_blobs() {
        let key = derive_key("x");
        let b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 10]);
        assert!(decrypt_tobeparsed(&b64, &key).is_err());
    }

    #[test]
    fn derive_key_xor_matches_web_client() {
        // partB XOR qd from a live bootstrap response (epoch 6884).
        let key = derive_key_xor(
            "UNQhzl5g9ARRgv6O25blL+lPqsuRMOJ2EA+mLXX2pWk=",
            "a39b86dbbcf57f884f3e9074969e7fe26656c74012e4545605896621ffa441c1",
        )
        .unwrap();
        // First byte: 0x50 ('P' in base64-decoded partB) XOR 0xa3 = 0xf3.
        assert_eq!(key[0], 0x50 ^ 0xa3);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn derive_key_xor_rejects_bad_lengths() {
        assert!(derive_key_xor("AAAA", "a39b").is_err());
    }

    #[test]
    fn aa_req_envelope_is_gcm_decryptable() {
        use aes_gcm::aead::Aead;
        use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};

        let key = [7u8; 32];
        let epoch = 6884u64;
        let qh = "d405d0edd690624b66baba3068e0edc3ac90f1597d898a1ec8db4e5c43c00fec";
        // A ts already aligned to the 5-minute bucket, so the token is exact.
        let now_ms = 1_784_600_000_000u128;
        let lane = "k7";
        let token = sign_aa_req(&key, epoch, "75", qh, lane, now_ms).unwrap();

        // Envelope: 0x01 || nonce(12) || ciphertext+tag. Recover and verify it
        // decrypts (i.e. the GCM tag is valid) back to the expected payload.
        let blob = base64::engine::general_purpose::STANDARD
            .decode(&token)
            .unwrap();
        assert_eq!(blob[0], 0x01);
        let ts = (now_ms / TS_BUCKET_MS) * TS_BUCKET_MS;
        let expected_nonce = Sha256::digest(format!("{epoch}:75:{qh}:{ts}:{lane}").as_bytes());
        assert_eq!(&blob[1..13], &expected_nonce[..12]);

        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&blob[1..13]), &blob[13..])
            .expect("aaReq envelope should authenticate and decrypt");
        assert_eq!(
            String::from_utf8(plaintext).unwrap(),
            format!(
                r#"{{"v":1,"ts":{ts},"epoch":{epoch},"buildId":"75","qh":"{qh}","k":"{lane}"}}"#
            )
        );
    }

    #[test]
    fn aa_boot_matches_web_client() {
        // Cross-checked against the live web client's `PS()` run in Node:
        // hex(HMAC(HMAC(mask,"aa-boot:75"), "75:mkissa:mkissa.to:6887:k7")).
        let got = sign_aa_boot(
            "ff65f1ba05d2556424dfec9f38f816e0a7d284a951845c865a609cb83bee7690",
            "75",
            "mkissa",
            "mkissa.to",
            6887,
            "k7",
        )
        .unwrap();
        assert_eq!(got.len(), 64);
        assert_eq!(
            got,
            "7eb17241a906398c25974817c90cbcf1ad9333ba5851161c5b2ac1c25f34e898"
        );
    }
}
