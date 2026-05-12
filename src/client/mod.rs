pub mod connection;
pub mod rpc;
pub mod stdio;
pub mod ws;

use serde::Serialize;

pub use connection::{
    Connection, ConnectionState, RequestOutcome, ServerMetadata, connect, handshake,
};
pub use rpc::{JsonRpcRouter, PreparedRequest};

#[derive(Debug, Clone, Serialize)]
pub struct ClientPlan {
    pub initialized: bool,
    pub transport_ready: bool,
    pub notes: Vec<&'static str>,
}

pub fn scaffold_plan() -> ClientPlan {
    ClientPlan {
        initialized: false,
        transport_ready: false,
        notes: vec![
            "Connection lifecycle will own initialize/initialized sequencing.",
            "Transport backends stay behind a shared facade.",
        ],
    }
}
