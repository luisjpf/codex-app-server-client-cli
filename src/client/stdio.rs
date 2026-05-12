use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct StdioTransportSkeleton {
    pub command: &'static str,
    pub status: &'static str,
}

impl Default for StdioTransportSkeleton {
    fn default() -> Self {
        Self {
            command: "codex app-server",
            status: "planned",
        }
    }
}
