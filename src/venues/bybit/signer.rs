//! Bybit V5 HMAC-SHA256 request signing.
//!
//! Payload convention:
//!   GET  → `payload = query_string` (use the exact string you send on the wire)
//!   POST → `payload = raw_json_body` (exact bytes)
//!
//! String to sign: `timestamp_ms || api_key || recv_window || payload`
//! Output: lowercase hex of HMAC-SHA256(api_secret, string_to_sign).

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn sign(
    api_secret: &str,
    timestamp_ms: u64,
    api_key: &str,
    recv_window_ms: u64,
    payload: &str,
) -> String {
    let mut mac =
        HmacSha256::new_from_slice(api_secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(api_key.as_bytes());
    mac.update(recv_window_ms.to_string().as_bytes());
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector_matches_openssl_output() {
        // Computed via:
        //   printf "1700000000000api_key_example5000accountType=UNIFIED" \
        //     | openssl dgst -sha256 -hmac "test_secret_123"
        let got = sign(
            "test_secret_123",
            1_700_000_000_000,
            "api_key_example",
            5_000,
            "accountType=UNIFIED",
        );
        assert_eq!(
            got,
            "e6225ce434c96b074701455a2d73a900a6f3072ec09b29a0002667590cae19f5"
        );
    }

    #[test]
    fn output_is_64_hex_chars() {
        let s = sign("s", 1, "k", 5000, "");
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_payloads_produce_different_signatures() {
        let a = sign("s", 1, "k", 5000, "a=1");
        let b = sign("s", 1, "k", 5000, "a=2");
        assert_ne!(a, b);
    }

    #[test]
    fn empty_payload_is_valid() {
        let s = sign("secret", 1_700_000_000_000, "key", 5_000, "");
        assert_eq!(s.len(), 64);
    }
}
