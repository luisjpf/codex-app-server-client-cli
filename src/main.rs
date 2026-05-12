use clap::Parser;
use codex_app_server_client_cli::cli::Cli;
use codex_app_server_client_cli::error::AppError;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            if let Err(print_err) = codex_app_server_client_cli::output::print_error(&err) {
                eprintln!("{print_err}");
                return print_err.exit_code().into();
            }
            if !matches!(err, AppError::ApprovalRequired { .. }) {
                eprintln!("{err}");
            }
            err.exit_code().into()
        }
    }
}

async fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    let outcome = codex_app_server_client_cli::run(cli).await?;
    codex_app_server_client_cli::output::print_execution(outcome).await?;
    Ok(())
}
