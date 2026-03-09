use clap::Parser;
use nex_lsp::{CodexLspConfig, build_service};
use std::path::PathBuf;
use tower_lsp::Server;

#[derive(Debug, Parser)]
#[command(name = "nex-lsp", version, about)]
struct Args {
    /// Repository root used for git reads and `.nex/` state files.
    #[arg(long)]
    repo_path: Option<PathBuf>,
    /// Base ref used for semantic diff and validation requests.
    #[arg(long, default_value = "HEAD~1")]
    base_ref: String,
    /// Poll interval in milliseconds for semantic event notifications.
    #[arg(long, default_value_t = 500)]
    event_poll_ms: u64,
    /// Optional upstream stdio language server command.
    #[arg(long)]
    upstream_command: Option<String>,
    /// Repeated upstream stdio language server arguments.
    #[arg(long)]
    upstream_arg: Vec<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let config = CodexLspConfig {
        repo_path: args.repo_path,
        base_ref: args.base_ref,
        event_poll_ms: args.event_poll_ms,
        upstream_command: args.upstream_command,
        upstream_args: args.upstream_arg,
    };

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = build_service(config);
    Server::new(stdin, stdout, socket).serve(service).await;
}
