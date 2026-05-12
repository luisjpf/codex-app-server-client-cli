use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct WsTransportSkeleton {
    pub supports_health_probe: bool,
    pub supports_bearer_auth: bool,
}

impl Default for WsTransportSkeleton {
    fn default() -> Self {
        Self {
            supports_health_probe: true,
            supports_bearer_auth: true,
        }
    }
}
