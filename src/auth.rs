use anyhow::{Context, Result, anyhow};
use hmac::{Hmac, Mac};
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, Identifiers, RegistrationResponse, RegistrationUpload, ServerLogin,
    ServerLoginParameters, ServerRegistration, ServerSetup, argon2::Argon2,
    ciphersuite::CipherSuite, rand::rngs::OsRng,
};
use sha2::{Digest, Sha256, Sha512};

use crate::{PROTOCOL_ALPN, PROTOCOL_VERSION, protocol::Request};

type HmacSha256 = Hmac<Sha256>;

const OPAQUE_CONTEXT: &[u8] = b"RemoText OPAQUE v1";
const CREDENTIAL_CONTEXT: &[u8] = b"RemoText single-password credential v1";
const CLIENT_ID: &[u8] = b"remotext-client";
const REQUEST_MAC_CONTEXT: &[u8] = b"RemoText OPAQUE request v1";

pub struct RemoTextCipherSuite;

impl CipherSuite for RemoTextCipherSuite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, Sha512>;
    type Ksf = Argon2<'static>;
}

#[derive(Clone)]
pub struct ServerAuth {
    setup: ServerSetup<RemoTextCipherSuite>,
    password_file: ServerRegistration<RemoTextCipherSuite>,
    server_id: [u8; 32],
}

pub struct ServerLoginSession {
    state: ServerLogin<RemoTextCipherSuite>,
    credential_response: Vec<u8>,
    context: Vec<u8>,
    server_id: [u8; 32],
}

pub struct ClientLoginStart {
    state: ClientLogin<RemoTextCipherSuite>,
    credential_request: Vec<u8>,
}

impl ServerAuth {
    pub fn new(password: &str, server_id: [u8; 32]) -> Result<Self> {
        let mut rng = OsRng;
        let setup = ServerSetup::<RemoTextCipherSuite>::new(&mut rng);
        let password_file = register_password(&setup, password, &server_id)?;

        Ok(Self {
            setup,
            password_file,
            server_id,
        })
    }

    pub fn server_id(&self) -> &[u8; 32] {
        &self.server_id
    }

    pub fn start_login(&self, credential_request: &[u8]) -> Result<ServerLoginSession> {
        let request = CredentialRequest::<RemoTextCipherSuite>::deserialize(credential_request)
            .context("deserialize OPAQUE credential request")?;
        let mut rng = OsRng;
        let context = login_context(&self.server_id);
        let credential_id = credential_identifier(&self.server_id);
        let start = ServerLogin::start(
            &mut rng,
            &self.setup,
            Some(self.password_file.clone()),
            request,
            &credential_id,
            ServerLoginParameters {
                context: Some(&context),
                identifiers: identifiers(&self.server_id),
            },
        )
        .context("start OPAQUE server login")?;

        Ok(ServerLoginSession {
            state: start.state,
            credential_response: start.message.serialize().to_vec(),
            context,
            server_id: self.server_id,
        })
    }
}

impl ServerLoginSession {
    pub fn credential_response(&self) -> &[u8] {
        &self.credential_response
    }

    pub fn finish(self, credential_finalization: &[u8]) -> Result<Vec<u8>> {
        let finalization =
            CredentialFinalization::<RemoTextCipherSuite>::deserialize(credential_finalization)
                .context("deserialize OPAQUE credential finalization")?;
        let finish = self
            .state
            .finish(
                finalization,
                ServerLoginParameters {
                    context: Some(&self.context),
                    identifiers: identifiers(&self.server_id),
                },
            )
            .map_err(|_| anyhow!("authentication failed"))?;

        Ok(finish.session_key.to_vec())
    }
}

impl ClientLoginStart {
    pub fn new(password: &str) -> Result<Self> {
        let mut rng = OsRng;
        let start = ClientLogin::<RemoTextCipherSuite>::start(&mut rng, password.as_bytes())
            .context("start OPAQUE client login")?;

        Ok(Self {
            state: start.state,
            credential_request: start.message.serialize().to_vec(),
        })
    }

    pub fn credential_request(&self) -> &[u8] {
        &self.credential_request
    }

    pub fn finish(
        self,
        password: &str,
        server_id: &[u8; 32],
        credential_response: &[u8],
        request: &Request,
    ) -> Result<(Vec<u8>, [u8; 32])> {
        let response = CredentialResponse::<RemoTextCipherSuite>::deserialize(credential_response)
            .context("deserialize OPAQUE credential response")?;
        let context = login_context(server_id);
        let mut rng = OsRng;
        let finish = self
            .state
            .finish(
                &mut rng,
                password.as_bytes(),
                response,
                ClientLoginFinishParameters {
                    context: Some(&context),
                    identifiers: identifiers(server_id),
                    ksf: None,
                },
            )
            .map_err(|_| anyhow!("authentication failed"))?;

        let request_bytes = request_bytes(request)?;
        let request_mac = request_mac(&finish.session_key, server_id, &request_bytes);
        Ok((finish.message.serialize().to_vec(), request_mac))
    }
}

pub fn request_bytes(request: &Request) -> Result<Vec<u8>> {
    postcard::to_stdvec(request).context("serialize request for authentication")
}

pub fn verify_request_mac(
    session_key: &[u8],
    server_id: &[u8; 32],
    request: &Request,
    candidate: &[u8; 32],
) -> Result<bool> {
    let bytes = request_bytes(request)?;
    let expected = request_mac(session_key, server_id, &bytes);
    Ok(constant_time_eq(&expected, candidate))
}

fn register_password(
    setup: &ServerSetup<RemoTextCipherSuite>,
    password: &str,
    server_id: &[u8; 32],
) -> Result<ServerRegistration<RemoTextCipherSuite>> {
    let mut rng = OsRng;
    let credential_id = credential_identifier(server_id);
    let client_start =
        ClientRegistration::<RemoTextCipherSuite>::start(&mut rng, password.as_bytes())
            .context("start OPAQUE password registration")?;
    let server_start = ServerRegistration::<RemoTextCipherSuite>::start(
        setup,
        client_start.message,
        &credential_id,
    )
    .context("start OPAQUE server registration")?;
    let client_finish = client_start
        .state
        .finish(
            &mut rng,
            password.as_bytes(),
            RegistrationResponse::deserialize(&server_start.message.serialize())?,
            ClientRegistrationFinishParameters {
                identifiers: identifiers(server_id),
                ksf: None,
            },
        )
        .context("finish OPAQUE password registration")?;

    Ok(ServerRegistration::finish(RegistrationUpload::<
        RemoTextCipherSuite,
    >::deserialize(
        &client_finish.message.serialize(),
    )?))
}

fn identifiers(server_id: &[u8; 32]) -> Identifiers<'_> {
    Identifiers {
        client: Some(CLIENT_ID),
        server: Some(server_id),
    }
}

fn credential_identifier(server_id: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CREDENTIAL_CONTEXT.len() + server_id.len());
    out.extend_from_slice(CREDENTIAL_CONTEXT);
    out.extend_from_slice(server_id);
    out
}

fn login_context(server_id: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(
        OPAQUE_CONTEXT.len() + PROTOCOL_ALPN.len() + size_of::<u16>() + server_id.len(),
    );
    out.extend_from_slice(OPAQUE_CONTEXT);
    out.extend_from_slice(PROTOCOL_ALPN);
    out.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    out.extend_from_slice(server_id);
    out
}

fn request_mac(session_key: &[u8], server_id: &[u8; 32], request_bytes: &[u8]) -> [u8; 32] {
    let request_hash = Sha256::digest(request_bytes);
    let mut mac = HmacSha256::new_from_slice(session_key).expect("HMAC accepts any key size");
    mac.update(REQUEST_MAC_CONTEXT);
    mac.update(PROTOCOL_ALPN);
    mac.update(&PROTOCOL_VERSION.to_be_bytes());
    mac.update(server_id);
    mac.update(&request_hash);

    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    out
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
    fn pake_login_verifies_request_mac() {
        let server_id = [4; 32];
        let server_auth = ServerAuth::new("secret", server_id).unwrap();
        let client_start = ClientLoginStart::new("secret").unwrap();
        let server_start = server_auth
            .start_login(client_start.credential_request())
            .unwrap();
        let request = Request::Ping;
        let (finalization, mac) = client_start
            .finish(
                "secret",
                &server_id,
                server_start.credential_response(),
                &request,
            )
            .unwrap();

        let session_key = server_start.finish(&finalization).unwrap();
        assert!(verify_request_mac(&session_key, &server_id, &request, &mac).unwrap());
    }

    #[test]
    fn pake_rejects_wrong_password() {
        let server_id = [5; 32];
        let server_auth = ServerAuth::new("secret", server_id).unwrap();
        let client_start = ClientLoginStart::new("wrong").unwrap();
        let server_start = server_auth
            .start_login(client_start.credential_request())
            .unwrap();

        assert!(
            client_start
                .finish(
                    "wrong",
                    &server_id,
                    server_start.credential_response(),
                    &Request::Ping,
                )
                .is_err()
        );
    }

    #[test]
    fn request_mac_rejects_tampering() {
        let server_id = [6; 32];
        let request = Request::Ping;
        let request_bytes = request_bytes(&request).unwrap();
        let mut mac = request_mac(b"session key", &server_id, &request_bytes);
        mac[0] ^= 1;

        assert!(!verify_request_mac(b"session key", &server_id, &request, &mac).unwrap());
    }
}
