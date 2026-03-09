// nex-cli: CLI binary for Nexum Graph
// Phase 0: `nex diff <ref-a> <ref-b>`
// Phase 1: `nex check <branch-a> <branch-b>`
// Phase 2: `nex lock`, `nex unlock`, `nex locks`, `nex validate`
// Phase 3: `nex log`, `nex rollback`

use clap::Parser;
use nex_cli::cli::{Cli, Commands};
use nex_cli::{coordination_pipeline, eventlog_pipeline, output, serve_pipeline};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Diff {
            ref_a,
            ref_b,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match nex_cli::pipeline::run_diff(repo, &ref_a, &ref_b) {
                Ok(diff) => {
                    let out = output::format_diff(&diff, &format);
                    println!("{out}");
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Check {
            branch_a,
            branch_b,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match nex_coord::ConflictDetector::detect(repo, &branch_a, &branch_b) {
                Ok(report) => {
                    let exit = report.exit_code();
                    let out = output::format_report(&report, &format);
                    println!("{out}");
                    std::process::exit(exit);
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Lock {
            agent_name,
            target_name,
            kind,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match coordination_pipeline::run_lock(repo, &agent_name, &target_name, &kind) {
                Ok(result) => {
                    let out =
                        output::format_lock_result(&result, &agent_name, &target_name, &format);
                    println!("{out}");
                    if matches!(result, nex_core::LockResult::Denied { .. }) {
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Unlock {
            agent_name,
            target_name,
            repo_path,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match coordination_pipeline::run_unlock(repo, &agent_name, &target_name) {
                Ok(()) => println!("Lock released."),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Locks { repo_path, format } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match coordination_pipeline::run_locks(repo) {
                Ok(entries) => {
                    let out = output::format_locks(&entries, &format);
                    println!("{out}");
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Validate {
            agent_name,
            base,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match coordination_pipeline::run_validate(repo, &agent_name, &base) {
                Ok(report) => {
                    let exit = report.exit_code();
                    let out = output::format_validation_report(&report, &format);
                    println!("{out}");
                    std::process::exit(exit);
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Log {
            repo_path,
            intent_id,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match eventlog_pipeline::run_log(repo, intent_id.as_deref()).await {
                Ok(events) => {
                    let out = output::format_event_log(&events, &format);
                    println!("{out}");
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Rollback {
            intent_id,
            agent_name,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match eventlog_pipeline::run_rollback(repo, &intent_id, &agent_name).await {
                Ok(outcome) => {
                    let out = output::format_rollback_outcome(&outcome, &format);
                    println!("{out}");
                    if !outcome.is_clean() {
                        std::process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Replay {
            to,
            repo_path,
            format,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            match eventlog_pipeline::run_replay(repo, &to).await {
                Ok(units) => {
                    let out = output::format_replay_state(&units, &format);
                    println!("{out}");
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Serve {
            host,
            port,
            repo_path,
        } => {
            let repo = repo_path.as_deref().unwrap_or(std::path::Path::new("."));

            if let Err(e) = serve_pipeline::run_serve(repo, &host, port).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}
