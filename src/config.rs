use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::cli::{Cli, OutputFormat, Transport};
use crate::error::AppError;

const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 60_000;
const DEFAULT_APPROVAL_POLICY: &str = "on-request";
const DEFAULT_SANDBOX_POLICY: &str = "workspace-write";
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:4500";

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedConfig {
    pub source: ConfigSource,
    pub config_path: Option<PathBuf>,
    pub connection: ConnectionConfig,
    pub session: SessionConfig,
    pub output: OutputConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSource {
    pub file_loaded: bool,
    pub env_considered: bool,
    pub flags_applied: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionConfig {
    pub transport: Transport,
    pub url: String,
    #[serde(skip_serializing)]
    pub bearer_token: Option<String>,
    pub bearer_token_set: bool,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionConfig {
    pub model: Option<String>,
    pub cwd: Option<PathBuf>,
    pub reasoning_effort: Option<String>,
    pub approval_policy: String,
    pub sandbox: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputConfig {
    pub format: OutputFormat,
    pub pretty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoggingConfig {
    pub verbose: bool,
    pub filter: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileConfig {
    #[serde(default)]
    connection: FileConnectionConfig,
    #[serde(default)]
    session: FileSessionConfig,
    #[serde(default)]
    output: FileOutputConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct FileConnectionConfig {
    #[serde(default = "default_transport")]
    transport: Transport,
    url: Option<String>,
    bearer_token: Option<String>,
    #[serde(default = "default_connect_timeout_ms")]
    connect_timeout_ms: u64,
    #[serde(default = "default_request_timeout_ms")]
    request_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct FileSessionConfig {
    model: Option<String>,
    cwd: Option<PathBuf>,
    reasoning_effort: Option<String>,
    #[serde(default = "default_approval_policy")]
    approval_policy: String,
    #[serde(default = "default_sandbox_policy")]
    sandbox: String,
}

#[derive(Debug, Clone, Deserialize)]
struct FileOutputConfig {
    #[serde(default = "default_output_format")]
    default_format: OutputFormat,
}

impl Default for FileConnectionConfig {
    fn default() -> Self {
        Self {
            transport: default_transport(),
            url: Some(DEFAULT_WS_URL.to_owned()),
            bearer_token: None,
            connect_timeout_ms: default_connect_timeout_ms(),
            request_timeout_ms: default_request_timeout_ms(),
        }
    }
}

impl Default for FileSessionConfig {
    fn default() -> Self {
        Self {
            model: None,
            cwd: None,
            reasoning_effort: None,
            approval_policy: default_approval_policy(),
            sandbox: default_sandbox_policy(),
        }
    }
}

impl Default for FileOutputConfig {
    fn default() -> Self {
        Self {
            default_format: default_output_format(),
        }
    }
}

impl ResolvedConfig {
    pub fn load(cli: &Cli) -> Result<Self, AppError> {
        let config_path = cli.config.clone().or_else(default_config_path);
        let file_config = load_file_config(config_path.as_ref())?;

        let transport = cli.transport.unwrap_or(file_config.connection.transport);
        let url = cli
            .url
            .clone()
            .or_else(|| std::env::var("CODEX_APP_SERVER_URL").ok())
            .or_else(|| file_config.connection.url.clone())
            .unwrap_or_else(|| DEFAULT_WS_URL.to_owned());
        let bearer_token = cli
            .bearer_token
            .clone()
            .or_else(|| std::env::var("CODEX_APP_SERVER_BEARER_TOKEN").ok())
            .or_else(|| file_config.connection.bearer_token.clone());
        let bearer_token_set = bearer_token.is_some();
        let model = cli
            .model
            .clone()
            .or_else(|| std::env::var("CODEX_APP_SERVER_MODEL").ok())
            .or_else(|| file_config.session.model.clone());
        let cwd = cli
            .cwd
            .clone()
            .or_else(|| {
                std::env::var("CODEX_APP_SERVER_CWD")
                    .ok()
                    .map(PathBuf::from)
            })
            .or_else(|| file_config.session.cwd.clone());
        let format = cli.output.unwrap_or(file_config.output.default_format);

        Ok(Self {
            source: ConfigSource {
                file_loaded: config_path.as_ref().is_some_and(|path| path.exists()),
                env_considered: true,
                flags_applied: true,
            },
            config_path,
            connection: ConnectionConfig {
                transport,
                url,
                bearer_token,
                bearer_token_set,
                connect_timeout_ms: file_config.connection.connect_timeout_ms,
                request_timeout_ms: file_config.connection.request_timeout_ms,
            },
            session: SessionConfig {
                model,
                cwd,
                reasoning_effort: file_config.session.reasoning_effort.clone(),
                approval_policy: file_config.session.approval_policy.clone(),
                sandbox: file_config.session.sandbox.clone(),
            },
            output: OutputConfig {
                format,
                pretty: cli.pretty,
            },
            logging: LoggingConfig {
                verbose: cli.verbose,
                filter: if cli.verbose {
                    "debug".to_owned()
                } else {
                    "info".to_owned()
                },
            },
        })
    }
}

fn load_file_config(path: Option<&PathBuf>) -> Result<FileConfig, AppError> {
    let Some(path) = path else {
        return Ok(FileConfig::default());
    };

    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|source| AppError::config_io(path.clone(), source))?;
    toml::from_str(&content).map_err(|source| AppError::config_parse(path.clone(), source))
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("codex-app-server-client-cli/config.toml"))
}

fn default_transport() -> Transport {
    Transport::Ws
}

fn default_output_format() -> OutputFormat {
    OutputFormat::Json
}

fn default_connect_timeout_ms() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_MS
}

fn default_request_timeout_ms() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_MS
}

fn default_approval_policy() -> String {
    DEFAULT_APPROVAL_POLICY.to_owned()
}

fn default_sandbox_policy() -> String {
    DEFAULT_SANDBOX_POLICY.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_defaults_to_expected_output() {
        let cli = Cli::parse_from(["bin", "health"]);
        let resolved = ResolvedConfig::load(&cli).expect("config should load");
        assert!(matches!(resolved.output.format, OutputFormat::Json));
        assert_eq!(resolved.connection.url, DEFAULT_WS_URL);
    }
}
