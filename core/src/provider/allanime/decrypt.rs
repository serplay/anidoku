//! Link deobfuscation and payload decryption, ported from ani-cli 4.14.1.

use crate::{Error, Result};
use aes::cipher::{KeyIvInit, StreamCipher};
use base64::Engine;
use sha2::{Digest, Sha256};

type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

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
pub fn derive_key(passphrase: &str) -> [u8; 32] {
    let digest = Sha256::digest(passphrase.as_bytes());
    digest.into()
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
}
