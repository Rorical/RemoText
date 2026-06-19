use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh::EndpointAddr;

const PREFIX: &str = "rt1_";

pub fn encode_addr(addr: &EndpointAddr) -> Result<String> {
    let bytes = postcard::to_stdvec(addr).context("serialize iroh endpoint address")?;
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)))
}

pub fn decode_addr(input: &str) -> Result<EndpointAddr> {
    let encoded = input
        .strip_prefix(PREFIX)
        .ok_or_else(|| anyhow::anyhow!("RemoText address must start with {PREFIX}"))?;
    if encoded.is_empty() {
        bail!("RemoText address is empty");
    }

    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("decode RemoText address")?;
    postcard::from_bytes(&bytes).context("deserialize RemoText address")
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{EndpointAddr, SecretKey};

    #[test]
    fn ticket_roundtrip() {
        let key = SecretKey::generate();
        let addr = EndpointAddr::new(key.public());
        let ticket = encode_addr(&addr).unwrap();
        assert!(ticket.starts_with("rt1_"));
        assert_eq!(decode_addr(&ticket).unwrap(), addr);
    }
}
