use tracing_subscriber::EnvFilter;

use crate::config::ResolvedConfig;
use crate::error::AppError;

pub fn init(config: &ResolvedConfig) -> Result<(), AppError> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| config.logging.filter.clone());
    let env_filter =
        EnvFilter::try_new(filter).map_err(|err| AppError::TracingInit(err.to_string()))?;

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|err| AppError::TracingInit(err.to_string()))
}
