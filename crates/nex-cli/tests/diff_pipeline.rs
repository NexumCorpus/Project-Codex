//! Integration tests for the `nex diff` pipeline.
//!
//! These tests create temporary git repositories, commit TypeScript files
//! at two refs, then run the pipeline and assert on the SemanticDiff output.
//!
//! Test categories:
//! 1. Pipeline unit tests (build_graph without git)
//! 2. Git integration tests (full end-to-end)
//! 3. Output formatting tests
//!
//! IMPORTANT: These tests exercise the entire Phase 0 stack:
//! git2 → nex-parse → nex-graph → SemanticDiff → output

use nex_cli::output::format_diff;
use nex_cli::pipeline::{build_graph, collect_files_at_ref, run_diff};
use nex_core::ChangeKind;
use nex_parse::{default_extractors, typescript_extractor};
use std::path::Path;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a temporary git repo, returning the tempdir (keeps it alive) and the repo.
fn init_temp_repo() -> (tempfile::TempDir, git2::Repository) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let repo = git2::Repository::init(dir.path()).expect("init repo");

    // Configure author for commits.
    let mut config = repo.config().expect("get config");
    config.set_str("user.name", "Test").expect("set name");
    config
        .set_str("user.email", "test@example.com")
        .expect("set email");

    (dir, repo)
}

/// Write a file into the repo working directory, stage it, and return the path.
fn write_and_stage(
    repo: &git2::Repository,
    relative_path: &str,
    content: &str,
) -> std::path::PathBuf {
    let full_path = repo.workdir().unwrap().join(relative_path);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).expect("create dirs");
    }
    std::fs::write(&full_path, content).expect("write file");

    let mut index = repo.index().expect("get index");
    index
        .add_path(Path::new(relative_path))
        .expect("add to index");
    index.write().expect("write index");

    full_path
}

/// Create a commit with the current index state and return its OID.
fn commit(repo: &git2::Repository, message: &str) -> git2::Oid {
    let mut index = repo.index().expect("get index");
    let tree_oid = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let sig = repo.signature().expect("get signature");

    // Find parent if HEAD exists.
    let parents: Vec<git2::Commit> = match repo.head() {
        Ok(head) => vec![head.peel_to_commit().expect("peel to commit")],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
        .expect("create commit")
}

/// Tag a commit.
fn tag(repo: &git2::Repository, oid: git2::Oid, tag_name: &str) {
    let obj = repo.find_object(oid, None).expect("find object");
    repo.tag_lightweight(tag_name, &obj, false)
        .expect("create tag");
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Pipeline unit tests (build_graph without git)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn build_graph_single_function() {
    let source = b"function greet(name: string): string { return `Hello, ${name}`; }";
    let files = vec![("src/greet.ts".to_string(), source.to_vec())];
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let graph = build_graph(&files, &extractors).expect("build graph");
    assert_eq!(graph.unit_count(), 1);
    let units = graph.units();
    assert_eq!(units[0].name, "greet");
}

#[test]
fn build_graph_class_with_methods() {
    let source = br#"
class Calculator {
    add(a: number, b: number): number { return a + b; }
    subtract(a: number, b: number): number { return a - b; }
}
"#;
    let files = vec![("src/calc.ts".to_string(), source.to_vec())];
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let graph = build_graph(&files, &extractors).expect("build graph");
    // Class + 2 methods = 3 units
    assert_eq!(graph.unit_count(), 3);
}

#[test]
fn build_graph_multiple_files() {
    let file_a = ("src/a.ts".to_string(), b"function alpha() {}".to_vec());
    let file_b = ("src/b.ts".to_string(), b"function beta() {}".to_vec());
    let files = vec![file_a, file_b];
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let graph = build_graph(&files, &extractors).expect("build graph");
    assert_eq!(graph.unit_count(), 2);
}

#[test]
fn build_graph_skips_non_ts_files() {
    let ts_file = ("src/main.ts".to_string(), b"function main() {}".to_vec());
    let py_file = ("src/util.py".to_string(), b"def helper(): pass".to_vec());
    let files = vec![ts_file, py_file];
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let graph = build_graph(&files, &extractors).expect("build graph");
    // Only the .ts file should be parsed
    assert_eq!(graph.unit_count(), 1);
}

#[test]
fn build_graph_supports_python_files_in_default_set() {
    let py_file = (
        "src/main.py".to_string(),
        b"def main() -> None:\n    return None\n".to_vec(),
    );
    let files = vec![py_file];
    let extractors = default_extractors();

    let graph = build_graph(&files, &extractors).expect("build graph");
    assert_eq!(graph.unit_count(), 1);
    assert_eq!(graph.units()[0].name, "main");
}

#[test]
fn build_graph_supports_rust_files_in_default_set() {
    let rs_file = (
        "src/main.rs".to_string(),
        b"fn main() {\n    helper();\n}\nfn helper() {}\n".to_vec(),
    );
    let files = vec![rs_file];
    let extractors = default_extractors();

    let graph = build_graph(&files, &extractors).expect("build graph");
    assert_eq!(graph.unit_count(), 2);
    assert!(graph.edge_count() >= 1);
}

#[test]
fn build_graph_with_dependencies() {
    let source = br#"
function helper(): number { return 42; }
function main(): void { helper(); }
"#;
    let files = vec![("src/app.ts".to_string(), source.to_vec())];
    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let graph = build_graph(&files, &extractors).expect("build graph");
    assert_eq!(graph.unit_count(), 2);
    // Should have at least one dependency edge (main → helper)
    assert!(graph.edge_count() >= 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Git integration tests (full end-to-end pipeline)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn diff_detects_added_function() {
    let (_dir, repo) = init_temp_repo();

    // v1: one function
    write_and_stage(&repo, "src/app.ts", "function alpha() {}");
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: add another function
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha() {}\nfunction beta() {}",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.added.len(), 1);
    assert_eq!(diff.added[0].name, "beta");
    assert_eq!(diff.removed.len(), 0);
}

#[test]
fn diff_detects_removed_function() {
    let (_dir, repo) = init_temp_repo();

    // v1: two functions
    write_and_stage(
        &repo,
        "src/app.ts",
        "function alpha() {}\nfunction beta() {}",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: remove beta
    write_and_stage(&repo, "src/app.ts", "function alpha() {}");
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.removed[0].name, "beta");
    assert_eq!(diff.added.len(), 0);
}

#[test]
fn diff_detects_body_change() {
    let (_dir, repo) = init_temp_repo();

    // v1: original body
    write_and_stage(
        &repo,
        "src/app.ts",
        "function greet(): string { return 'hello'; }",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: changed body, same signature
    write_and_stage(
        &repo,
        "src/app.ts",
        "function greet(): string { return 'goodbye'; }",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.modified.len(), 1);
    assert_eq!(diff.modified[0].after.name, "greet");
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
}

#[test]
fn diff_detects_signature_change() {
    let (_dir, repo) = init_temp_repo();

    // v1: original signature
    write_and_stage(
        &repo,
        "src/app.ts",
        "function calc(a: number): number { return a; }",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: changed parameter type
    write_and_stage(
        &repo,
        "src/app.ts",
        "function calc(a: string): string { return a; }",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.modified.len(), 1);
    assert!(
        diff.modified[0]
            .changes
            .contains(&ChangeKind::SignatureChanged)
    );
}

#[test]
fn diff_detects_moved_function() {
    let (_dir, repo) = init_temp_repo();

    // v1: function in file A
    write_and_stage(&repo, "src/utils.ts", "function helper(): void {}");
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: same function moved to file B (delete from A, add to B)
    // Remove old file
    let old_path = repo.workdir().unwrap().join("src/utils.ts");
    std::fs::remove_file(&old_path).expect("remove old file");
    let mut index = repo.index().expect("get index");
    index
        .remove_path(Path::new("src/utils.ts"))
        .expect("remove from index");
    index.write().expect("write index");

    write_and_stage(&repo, "src/helpers.ts", "function helper(): void {}");
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.moved.len(), 1);
    assert_eq!(diff.moved[0].unit.name, "helper");
    assert!(
        diff.moved[0]
            .old_path
            .to_string_lossy()
            .contains("utils.ts")
    );
    assert!(
        diff.moved[0]
            .new_path
            .to_string_lossy()
            .contains("helpers.ts")
    );
}

#[test]
fn diff_unchanged_produces_empty_diff() {
    let (_dir, repo) = init_temp_repo();

    // v1: a function
    write_and_stage(&repo, "src/app.ts", "function stable(): void {}");
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: exact same content (new commit, no changes)
    let c2 = commit(&repo, "v2 (no change)");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.added.len(), 0);
    assert_eq!(diff.removed.len(), 0);
    assert_eq!(diff.modified.len(), 0);
    assert_eq!(diff.moved.len(), 0);
}

#[test]
fn diff_multiple_files_mixed_changes() {
    let (_dir, repo) = init_temp_repo();

    // v1: two files
    write_and_stage(
        &repo,
        "src/a.ts",
        "function unchanged(): void {}\nfunction toRemove(): void {}",
    );
    write_and_stage(
        &repo,
        "src/b.ts",
        "function toModify(): string { return 'old'; }",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    // v2: remove toRemove, modify toModify, add newFunc
    write_and_stage(
        &repo,
        "src/a.ts",
        "function unchanged(): void {}\nfunction newFunc(): void {}",
    );
    write_and_stage(
        &repo,
        "src/b.ts",
        "function toModify(): string { return 'new'; }",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.added.len(), 1, "expected 1 added");
    assert_eq!(diff.removed.len(), 1, "expected 1 removed");
    assert_eq!(diff.modified.len(), 1, "expected 1 modified");
    assert_eq!(diff.added[0].name, "newFunc");
    assert_eq!(diff.removed[0].name, "toRemove");
    assert_eq!(diff.modified[0].after.name, "toModify");
}

#[test]
fn diff_detects_python_body_change() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "src/app.py",
        "def greet(name: str) -> str:\n    return 'hello ' + name\n",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    write_and_stage(
        &repo,
        "src/app.py",
        "def greet(name: str) -> str:\n    return 'hi ' + name\n",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.modified.len(), 1);
    assert_eq!(diff.modified[0].after.name, "greet");
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
}

#[test]
fn diff_detects_rust_body_change() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(
        &repo,
        "src/lib.rs",
        "fn greet(name: &str) -> String {\n    format!(\"hello {}\", name)\n}\n",
    );
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    write_and_stage(
        &repo,
        "src/lib.rs",
        "fn greet(name: &str) -> String {\n    format!(\"hi {}\", name)\n}\n",
    );
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let diff = run_diff(repo.workdir().unwrap(), "v1", "v2").expect("run diff");
    assert_eq!(diff.modified.len(), 1);
    assert_eq!(diff.modified[0].after.name, "greet");
    assert!(diff.modified[0].changes.contains(&ChangeKind::BodyChanged));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Git file collection tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn collect_files_returns_only_supported_extensions() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(&repo, "src/main.ts", "function main() {}");
    write_and_stage(&repo, "README.md", "# Hello");
    write_and_stage(&repo, "data.json", "{}");
    let c1 = commit(&repo, "initial");
    tag(&repo, c1, "v1");

    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];
    let files = collect_files_at_ref(&repo, "v1", &extractors).expect("collect files");

    // Only main.ts should be collected
    assert_eq!(files.len(), 1);
    assert!(files[0].0.contains("main.ts"));
}

#[test]
fn collect_files_includes_python_when_extractor_available() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(&repo, "src/main.ts", "function main() {}");
    write_and_stage(
        &repo,
        "src/helper.py",
        "def helper() -> None:\n    return None\n",
    );
    let c1 = commit(&repo, "initial");
    tag(&repo, c1, "v1");

    let extractors = default_extractors();
    let files = collect_files_at_ref(&repo, "v1", &extractors).expect("collect files");

    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|(path, _)| path.contains("main.ts")));
    assert!(files.iter().any(|(path, _)| path.contains("helper.py")));
}

#[test]
fn collect_files_reads_correct_content_at_ref() {
    let (_dir, repo) = init_temp_repo();

    write_and_stage(&repo, "src/app.ts", "function v1() {}");
    let c1 = commit(&repo, "v1");
    tag(&repo, c1, "v1");

    write_and_stage(&repo, "src/app.ts", "function v2() {}");
    let c2 = commit(&repo, "v2");
    tag(&repo, c2, "v2");

    let extractors: Vec<Box<dyn nex_parse::SemanticExtractor>> = vec![typescript_extractor()];

    let files_v1 = collect_files_at_ref(&repo, "v1", &extractors).expect("collect v1");
    let files_v2 = collect_files_at_ref(&repo, "v2", &extractors).expect("collect v2");

    let content_v1 = String::from_utf8_lossy(&files_v1[0].1);
    let content_v2 = String::from_utf8_lossy(&files_v2[0].1);
    assert!(content_v1.contains("v1"));
    assert!(content_v2.contains("v2"));
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Output formatting tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn format_json_produces_valid_json() {
    let diff = nex_core::SemanticDiff {
        added: vec![],
        removed: vec![],
        modified: vec![],
        moved: vec![],
    };
    let output = format_diff(&diff, "json");
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    assert!(parsed.get("added").is_some());
    assert!(parsed.get("removed").is_some());
    assert!(parsed.get("modified").is_some());
    assert!(parsed.get("moved").is_some());
}

#[test]
fn format_text_includes_counts() {
    let diff = nex_core::SemanticDiff {
        added: vec![],
        removed: vec![],
        modified: vec![],
        moved: vec![],
    };
    let output = format_diff(&diff, "text");
    // Text output should mention counts
    assert!(output.contains("0") || output.contains("no "));
}

#[test]
fn format_github_produces_markdown() {
    let diff = nex_core::SemanticDiff {
        added: vec![],
        removed: vec![],
        modified: vec![],
        moved: vec![],
    };
    let output = format_diff(&diff, "github");
    // GitHub output should contain markdown headers
    assert!(output.contains('#'));
}
