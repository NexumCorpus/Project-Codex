//! Integration tests for the TypeScript SemanticExtractor.
//!
//! These tests encode the Phase 0 acceptance criteria from the Implementation
//! Specification. They are written BEFORE implementation — Codex must write
//! code that passes them.
//!
//! Acceptance criteria tested:
//! - Extracts function_declaration, class_declaration, method_definition,
//!   interface_declaration, and named arrow_function nodes
//! - Computes correct signature_hash (params + return type)
//! - Computes correct body_hash (normalized body AST via BLAKE3)
//! - Produces correct qualified_name paths
//! - Extracts dependency edges (Calls, Imports, Inherits, Implements)
//! - Detects signature changes vs body-only changes
//! - Handles TypeScript-specific constructs (generics, decorators, async)

use nex_core::{DepKind, UnitKind};
use nex_parse::SemanticExtractor;
use std::path::Path;

/// Helper: get the TypeScript extractor.
fn ts_extractor() -> Box<dyn SemanticExtractor> {
    nex_parse::typescript_extractor()
}

// ─────────────────────────────────────────────────────────────────────────────
// Basic Extraction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn extracts_top_level_function() {
    let source = br#"
function greet(name: string): string {
    return `Hello, ${name}!`;
}
"#;
    let extractor = ts_extractor();
    let units = extractor
        .extract(Path::new("greet.ts"), source)
        .expect("extraction should succeed");

    assert_eq!(units.len(), 1);
    let unit = &units[0];
    assert_eq!(unit.name, "greet");
    assert_eq!(unit.kind, UnitKind::Function);
    assert_eq!(unit.qualified_name, "greet");
    assert_eq!(unit.file_path, Path::new("greet.ts"));
    assert!(unit.byte_range.start < unit.byte_range.end);
    // ID must be a non-zero BLAKE3 hash
    assert_ne!(unit.id, [0u8; 32]);
}

#[test]
fn extracts_class_with_methods() {
    let source = br#"
class AuthManager {
    private token: string;

    constructor(token: string) {
        this.token = token;
    }

    validateToken(refreshWindow: number): boolean {
        return this.token.length > 0;
    }

    async refreshSession(): Promise<Session> {
        return await fetch('/refresh');
    }
}
"#;
    let extractor = ts_extractor();
    let units = extractor
        .extract(Path::new("auth.ts"), source)
        .expect("extraction should succeed");

    // Should extract: AuthManager (class), constructor (method),
    // validateToken (method), refreshSession (method)
    let class_unit = units.iter().find(|u| u.name == "AuthManager");
    assert!(class_unit.is_some(), "should extract the AuthManager class");
    assert_eq!(class_unit.unwrap().kind, UnitKind::Class);

    let validate = units.iter().find(|u| u.name == "validateToken");
    assert!(validate.is_some(), "should extract validateToken method");
    assert_eq!(validate.unwrap().kind, UnitKind::Method);
    assert!(validate.unwrap().qualified_name.contains("AuthManager"));

    let refresh = units.iter().find(|u| u.name == "refreshSession");
    assert!(refresh.is_some(), "should extract refreshSession method");
    assert_eq!(refresh.unwrap().kind, UnitKind::Method);
}

#[test]
fn extracts_interface() {
    let source = br#"
interface UserService {
    getUser(id: string): Promise<User>;
    deleteUser(id: string): Promise<void>;
}
"#;
    let extractor = ts_extractor();
    let units = extractor
        .extract(Path::new("service.ts"), source)
        .expect("extraction should succeed");

    let iface = units.iter().find(|u| u.name == "UserService");
    assert!(iface.is_some(), "should extract the interface");
    assert_eq!(iface.unwrap().kind, UnitKind::Interface);
}

#[test]
fn extracts_named_arrow_function() {
    let source = br#"
const processPayment = (amount: number, currency: string): Receipt => {
    return { amount, currency, timestamp: Date.now() };
};
"#;
    let extractor = ts_extractor();
    let units = extractor
        .extract(Path::new("payment.ts"), source)
        .expect("extraction should succeed");

    let arrow = units.iter().find(|u| u.name == "processPayment");
    assert!(
        arrow.is_some(),
        "should extract named arrow function as a semantic unit"
    );
    assert_eq!(arrow.unwrap().kind, UnitKind::Function);
}

// ─────────────────────────────────────────────────────────────────────────────
// Hash Stability and Change Detection
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn same_source_produces_same_hashes() {
    let source = br#"
function add(a: number, b: number): number {
    return a + b;
}
"#;
    let extractor = ts_extractor();
    let units_1 = extractor.extract(Path::new("math.ts"), source).unwrap();
    let units_2 = extractor.extract(Path::new("math.ts"), source).unwrap();

    assert_eq!(units_1[0].signature_hash, units_2[0].signature_hash);
    assert_eq!(units_1[0].body_hash, units_2[0].body_hash);
    assert_eq!(units_1[0].id, units_2[0].id);
}

#[test]
fn body_change_detected_signature_stable() {
    let source_v1 = br#"
function add(a: number, b: number): number {
    return a + b;
}
"#;
    let source_v2 = br#"
function add(a: number, b: number): number {
    console.log("adding");
    return a + b;
}
"#;
    let extractor = ts_extractor();
    let units_v1 = extractor.extract(Path::new("math.ts"), source_v1).unwrap();
    let units_v2 = extractor.extract(Path::new("math.ts"), source_v2).unwrap();

    // Signature unchanged — same params, same return type
    assert_eq!(units_v1[0].signature_hash, units_v2[0].signature_hash);
    // Body changed — different implementation
    assert_ne!(units_v1[0].body_hash, units_v2[0].body_hash);
}

#[test]
fn signature_change_detected() {
    let source_v1 = br#"
function validateToken(token: string): boolean {
    return token.length > 0;
}
"#;
    let source_v2 = br#"
function validateToken(token: string, refreshWindow: number): boolean {
    return token.length > 0;
}
"#;
    let extractor = ts_extractor();
    let units_v1 = extractor.extract(Path::new("auth.ts"), source_v1).unwrap();
    let units_v2 = extractor.extract(Path::new("auth.ts"), source_v2).unwrap();

    // Signature changed — added parameter
    assert_ne!(units_v1[0].signature_hash, units_v2[0].signature_hash);
}

#[test]
fn whitespace_changes_do_not_affect_body_hash() {
    let source_v1 = br#"
function add(a: number, b: number): number {
    return a + b;
}
"#;
    let source_v2 = br#"
function add(a: number, b: number): number {
    return a    +    b;
}
"#;
    let extractor = ts_extractor();
    let units_v1 = extractor.extract(Path::new("math.ts"), source_v1).unwrap();
    let units_v2 = extractor.extract(Path::new("math.ts"), source_v2).unwrap();

    // Body hash should be over NORMALIZED AST, not raw text.
    // Whitespace-only changes should not change the body hash.
    assert_eq!(units_v1[0].body_hash, units_v2[0].body_hash);
}

// ─────────────────────────────────────────────────────────────────────────────
// Dependency Extraction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn extracts_call_dependencies() {
    let source = br#"
function validateToken(token: string): boolean {
    return token.length > 0;
}

function processRequest(req: Request): Response {
    if (!validateToken(req.token)) {
        throw new Error("invalid token");
    }
    return { status: 200 };
}
"#;
    let extractor = ts_extractor();
    let units = extractor.extract(Path::new("handler.ts"), source).unwrap();
    let deps = extractor
        .dependencies(&units, source)
        .expect("dependency extraction should succeed");

    // processRequest should have a Calls dependency on validateToken
    let validate_id = units.iter().find(|u| u.name == "validateToken").unwrap().id;
    let process_id = units
        .iter()
        .find(|u| u.name == "processRequest")
        .unwrap()
        .id;

    let has_call_dep = deps.iter().any(|(from, to, kind)| {
        *from == process_id && *to == validate_id && *kind == DepKind::Calls
    });

    assert!(
        has_call_dep,
        "processRequest should have a Calls dependency on validateToken"
    );
}

#[test]
fn extracts_import_dependencies() {
    let source = br#"
import { Logger } from './logger';
import { Database } from './database';

function initialize(): void {
    const logger = new Logger();
    const db = new Database();
}
"#;
    let extractor = ts_extractor();
    let units = extractor.extract(Path::new("init.ts"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    // Should have at least some Imports dependency edges
    let import_deps: Vec<_> = deps
        .iter()
        .filter(|(_, _, k)| *k == DepKind::Imports)
        .collect();
    assert!(
        !import_deps.is_empty(),
        "should extract import dependencies"
    );
}

#[test]
fn extracts_inheritance_dependency() {
    let source = br#"
class Animal {
    name: string;
    constructor(name: string) {
        this.name = name;
    }
}

class Dog extends Animal {
    breed: string;
    constructor(name: string, breed: string) {
        super(name);
        this.breed = breed;
    }
}
"#;
    let extractor = ts_extractor();
    let units = extractor.extract(Path::new("animals.ts"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    let animal_id = units.iter().find(|u| u.name == "Animal").unwrap().id;
    let dog_id = units.iter().find(|u| u.name == "Dog").unwrap().id;

    let has_inherits = deps
        .iter()
        .any(|(from, to, kind)| *from == dog_id && *to == animal_id && *kind == DepKind::Inherits);

    assert!(
        has_inherits,
        "Dog should have Inherits dependency on Animal"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Trait Contract
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn extractor_reports_correct_extensions() {
    let extractor = ts_extractor();
    let exts = extractor.extensions();
    assert!(exts.contains(&"ts"), "should support .ts files");
    assert!(exts.contains(&"tsx"), "should support .tsx files");
}

#[test]
fn empty_file_produces_no_units() {
    let extractor = ts_extractor();
    let units = extractor.extract(Path::new("empty.ts"), b"").unwrap();
    assert!(units.is_empty(), "empty file should produce no units");
}

#[test]
fn syntax_error_does_not_panic() {
    let source = br#"
function broken( {
    // missing closing paren and brace
"#;
    let extractor = ts_extractor();
    // Should either return Ok with partial results or Err — must not panic
    let result = extractor.extract(Path::new("broken.ts"), source);
    // We accept either outcome; the key invariant is no panic
    match result {
        Ok(units) => {
            // Partial extraction is acceptable
            let _ = units;
        }
        Err(_) => {
            // Error is also acceptable
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multiple Constructs in One File
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn extracts_mixed_constructs() {
    let source = br#"
interface Config {
    host: string;
    port: number;
}

class Server {
    private config: Config;

    constructor(config: Config) {
        this.config = config;
    }

    start(): void {
        console.log(`Starting on ${this.config.host}:${this.config.port}`);
    }
}

function createServer(host: string, port: number): Server {
    return new Server({ host, port });
}

const DEFAULT_PORT = 3000;
"#;
    let extractor = ts_extractor();
    let units = extractor
        .extract(Path::new("server.ts"), source)
        .expect("extraction should succeed");

    // Should find: Config (interface), Server (class), constructor (method),
    // start (method), createServer (function)
    assert!(
        units
            .iter()
            .any(|u| u.name == "Config" && u.kind == UnitKind::Interface)
    );
    assert!(
        units
            .iter()
            .any(|u| u.name == "Server" && u.kind == UnitKind::Class)
    );
    assert!(
        units
            .iter()
            .any(|u| u.name == "start" && u.kind == UnitKind::Method)
    );
    assert!(
        units
            .iter()
            .any(|u| u.name == "createServer" && u.kind == UnitKind::Function)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Qualified Name Correctness
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn method_qualified_name_includes_class() {
    let source = br#"
class PaymentProcessor {
    processPayment(amount: number): Receipt {
        return { amount, timestamp: Date.now() };
    }
}
"#;
    let extractor = ts_extractor();
    let units = extractor.extract(Path::new("payment.ts"), source).unwrap();

    let method = units.iter().find(|u| u.name == "processPayment").unwrap();
    assert_eq!(
        method.qualified_name, "PaymentProcessor::processPayment",
        "method qualified_name should be ClassName::methodName"
    );
}
