pub mod auth;
pub mod client;
mod files;
mod framing;
pub mod protocol;
pub mod server;
pub mod session;
pub mod ticket;
pub mod update;

pub const NETWORK_LAYER: &str = "iroh";
pub const PROTOCOL_ALPN_STR: &str = "remotext/1";
pub const PROTOCOL_ALPN: &[u8] = PROTOCOL_ALPN_STR.as_bytes();
pub const PROTOCOL_VERSION: u16 = 1;
