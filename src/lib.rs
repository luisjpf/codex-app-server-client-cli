pub mod approval;
pub mod cli;
pub mod client;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;
pub mod pending_approval;
pub mod policy;
pub mod protocol;
pub mod session;
pub mod tracing_setup;

use cli::Cli;
use commands::CommandExecution;
use config::ResolvedConfig;
use error::AppError;

pub async fn run(cli: Cli) -> Result<CommandExecution, AppError> {
    let config = ResolvedConfig::load(&cli)?;
    tracing_setup::init(&config)?;
    commands::execute(cli.command, config).await
}
