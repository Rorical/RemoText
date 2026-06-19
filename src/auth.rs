use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::protocol::Request;

type HmacSha256 = Hmac<Sha256>;

const AUTH_CONTEXT: &[u8] = b"RemoText auth v1";

pub fn request_bytes(request: &Request) -> Result<Vec<u8>> {
    postcard::to_stdvec(request).context("serialize request for authentication")
}

pub fn proof(
    password: &str,
    server_id: &[u8; 32],
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    request_bytes: &[u8],
) -> [u8; 32] {
    let password_hash = Sha256::digest(password.as_bytes());
    let request_hash = Sha256::digest(request_bytes);
    let mut mac = HmacSha256::new_from_slice(&password_hash).expect("HMAC accepts any key size");

    mac.update(AUTH_CONTEXT);
    mac.update(server_id);
    mac.update(client_nonce);
    mac.update(server_nonce);
    mac.update(&request_hash);

    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
}

pub fn verify(
    password: &str,
    server_id: &[u8; 32],
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
    request: &Request,
    candidate: &[u8; 32],
) -> Result<bool> {
    let bytes = request_bytes(request)?;
    let expected = proof(password, server_id, client_nonce, server_nonce, &bytes);
    Ok(constant_time_eq(&expected, candidate))
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;

    #[test]
    fn proof_verifies_and_rejects_wrong_password() {
        let request = Request::Ping;
        let bytes = request_bytes(&request).unwrap();
        let server_id = [1; 32];
        let client_nonce = [2; 32];
        let server_nonce = [3; 32];
        let candidate = proof("secret", &server_id, &client_nonce, &server_nonce, &bytes);

        assert!(
            verify(
                "secret",
                &server_id,
                &client_nonce,
                &server_nonce,
                &request,
                &candidate
            )
            .unwrap()
        );
        assert!(
            !verify(
                "wrong",
                &server_id,
                &client_nonce,
                &server_nonce,
                &request,
                &candidate
            )
            .unwrap()
        );
    }
}
