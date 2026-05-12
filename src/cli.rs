use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "Automation-friendly Codex app-server client CLI"
)]
pub struct Cli {
    #[arg(long, global = true, env = "CODEX_APP_SERVER_CONFIG")]
    pub config: Option<PathBuf>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_TRANSPORT")]
    pub transport: Option<Transport>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_URL")]
    pub url: Option<String>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_BEARER_TOKEN")]
    pub bearer_token: Option<String>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_MODEL")]
    pub model: Option<String>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_CWD")]
    pub cwd: Option<PathBuf>,

    #[arg(long, global = true, env = "CODEX_APP_SERVER_OUTPUT")]
    pub output: Option<OutputFormat>,

    #[arg(long, global = true, default_value_t = false)]
    pub pretty: bool,

    #[arg(long, global = true, default_value_t = false)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Run(RunArgs),
    Resume(ResumeArgs),
    Approve(ApprovalResolveArgs),
    Deny(ApprovalRefArgs),
    Health,
    #[command(subcommand)]
    Models(ModelsCommand),
    #[command(subcommand)]
    Session(SessionCommand),
    #[command(subcommand)]
    Approval(ApprovalCommand),
    #[command(subcommand)]
    Fs(FsCommand),
    #[command(subcommand, hide = true)]
    Threads(ThreadsCommand),
    #[command(subcommand, hide = true)]
    Turns(TurnsCommand),
}

#[derive(Debug, Clone, Args)]
pub struct RunArgs {
    #[arg(value_name = "INPUT")]
    pub input: String,

    #[arg(long)]
    pub session: Option<String>,

    #[arg(long, default_value_t = false)]
    pub ephemeral: bool,

    #[arg(long, default_value_t = false)]
    pub yolo: bool,

    #[arg(long, default_value_t = false, conflicts_with = "yolo")]
    pub no_yolo: bool,

    #[arg(long)]
    pub cwd: Option<PathBuf>,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long = "approval-policy")]
    pub approval_policy: Option<String>,

    #[arg(long)]
    pub sandbox: Option<String>,

    #[arg(long, default_value_t = false)]
    pub watch: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ResumeArgs {
    #[arg(value_name = "SESSION")]
    pub session: String,

    #[arg(value_name = "INPUT")]
    pub input: Option<String>,

    #[arg(long, default_value_t = false)]
    pub yolo: bool,

    #[arg(long, default_value_t = false, conflicts_with = "yolo")]
    pub no_yolo: bool,

    #[arg(long)]
    pub cwd: Option<PathBuf>,

    #[arg(long)]
    pub model: Option<String>,

    #[arg(long = "approval-policy")]
    pub approval_policy: Option<String>,

    #[arg(long)]
    pub sandbox: Option<String>,

    #[arg(long, default_value_t = false)]
    pub watch: bool,
}

#[derive(Debug, Clone, Args)]
pub struct SessionRefArgs {
    #[arg(long, conflicts_with = "alias")]
    pub id: Option<String>,

    #[arg(long, conflicts_with = "id")]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ApprovalRefArgs {
    #[arg(long, conflicts_with = "token")]
    pub id: Option<String>,

    #[arg(long, conflicts_with = "id")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ApprovalResolveArgs {
    #[command(flatten)]
    pub reference: ApprovalRefArgs,

    #[arg(long, default_value_t = false)]
    pub no_resume: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ModelsCommand {
    List,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionCommand {
    List,
    Show(SessionRefArgs),
    Fork(SessionRefArgs),
}

#[derive(Debug, Clone, Subcommand)]
pub enum ApprovalCommand {
    List,
    Show(ApprovalRefArgs),
    Approve(ApprovalResolveArgs),
    Deny(ApprovalRefArgs),
}

#[derive(Debug, Clone, Subcommand)]
pub enum ThreadsCommand {
    List,
    Start(ThreadStartArgs),
    Resume(ThreadResumeArgs),
    Read(ThreadIdArgs),
}

#[derive(Debug, Clone, Subcommand)]
pub enum TurnsCommand {
    Start(TurnStartArgs),
    Interrupt(ThreadIdArgs),
}

#[derive(Debug, Clone, Subcommand)]
pub enum FsCommand {
    Ls(PathArgs),
    Cat(PathArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ThreadStartArgs {
    #[arg(long)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ThreadResumeArgs {
    #[arg(long)]
    pub thread_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ThreadIdArgs {
    #[arg(long)]
    pub thread_id: String,
}

#[derive(Debug, Clone, Args)]
pub struct TurnStartArgs {
    #[arg(long)]
    pub thread_id: String,

    #[arg(long)]
    pub prompt: String,

    #[arg(long, alias = "stream", default_value_t = false)]
    pub watch: bool,
}

#[derive(Debug, Clone, Args)]
pub struct PathArgs {
    #[arg(long)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Ws,
    Stdio,
    Unix,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Json,
    Jsonl,
    Text,
}
