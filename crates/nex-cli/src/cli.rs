//! CLI argument parsing via clap derive.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Nexum Graph — AI-native code coordination.
#[derive(Parser, Debug)]
#[command(name = "nex", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Compute semantic diff between two git refs.
    Diff {
        /// Base git ref (commit, branch, tag).
        ref_a: String,
        /// Target git ref (commit, branch, tag).
        ref_b: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json, text, or github.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Check for semantic conflicts between two branches (three-way merge analysis).
    Check {
        /// First branch ref.
        branch_a: String,
        /// Second branch ref.
        branch_b: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json, text, or github.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Request a semantic lock on a code unit.
    Lock {
        /// Agent name (human-readable identifier, e.g. "alice").
        agent_name: String,
        /// Target unit name (function, class, etc. by qualified_name or name).
        target_name: String,
        /// Lock kind: read, write, or delete.
        kind: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Release a semantic lock.
    Unlock {
        /// Agent name.
        agent_name: String,
        /// Target unit name.
        target_name: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
    /// List all active semantic locks.
    Locks {
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Validate that committed changes are covered by semantic locks.
    Validate {
        /// Agent name to validate lock coverage for.
        agent_name: String,
        /// Base git ref to compare against (default: HEAD~1).
        #[arg(long, default_value = "HEAD~1")]
        base: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Show semantic event history.
    Log {
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Optional intent id filter.
        #[arg(long)]
        intent_id: Option<String>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Generate a semantic rollback event for a prior intent.
    Rollback {
        /// Intent id to roll back.
        intent_id: String,
        /// Agent name recorded on the rollback event.
        agent_name: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Replay semantic state to a historical event boundary.
    Replay {
        /// Event id to replay to.
        #[arg(long)]
        to: String,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
        /// Output format: json or text.
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Start the coordination server.
    Serve {
        /// Host interface to bind.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// TCP port to bind.
        #[arg(long, default_value_t = 4000)]
        port: u16,
        /// Path to the git repository (defaults to ".").
        #[arg(long)]
        repo_path: Option<PathBuf>,
    },
}
