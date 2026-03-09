//! Integration tests for the Rust SemanticExtractor.
//!
//! These tests cover the Rust P1 extractor slice called out in the
//! implementation spec.

use nex_core::{DepKind, UnitKind};
use nex_parse::SemanticExtractor;
use std::path::Path;

fn rs_extractor() -> Box<dyn SemanticExtractor> {
    nex_parse::rust_extractor()
}

#[test]
fn extracts_top_level_items_and_impl_methods() {
    let source = br#"
mod auth {
    pub struct AuthManager;

    pub enum Status {
        Ok,
        Denied,
    }

    pub trait Validator {
        fn validate(&self, input: &str) -> bool;
    }

    impl Validator for AuthManager {
        fn validate(&self, input: &str) -> bool {
            check(input)
        }
    }

    fn check(input: &str) -> bool {
        !input.is_empty()
    }
}
"#;

    let extractor = rs_extractor();
    let units = extractor.extract(Path::new("auth.rs"), source).unwrap();

    assert!(
        units
            .iter()
            .any(|unit| unit.name == "auth" && unit.kind == UnitKind::Module)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "AuthManager" && unit.kind == UnitKind::Struct)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "Status" && unit.kind == UnitKind::Enum)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "Validator" && unit.kind == UnitKind::Trait)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "validate" && unit.kind == UnitKind::Method)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "check" && unit.kind == UnitKind::Function)
    );
}

#[test]
fn method_and_function_qualified_names_include_module_scope() {
    let source = br#"
mod auth {
    pub struct AuthManager;

    impl AuthManager {
        pub fn validate(&self, input: &str) -> bool {
            helper(input)
        }
    }

    fn helper(input: &str) -> bool {
        !input.is_empty()
    }
}
"#;

    let extractor = rs_extractor();
    let units = extractor.extract(Path::new("auth.rs"), source).unwrap();

    let method = units.iter().find(|unit| unit.name == "validate").unwrap();
    let helper = units.iter().find(|unit| unit.name == "helper").unwrap();

    assert_eq!(method.qualified_name, "auth::AuthManager::validate");
    assert_eq!(helper.qualified_name, "auth::helper");
}

#[test]
fn same_source_produces_same_hashes() {
    let source = br#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;

    let extractor = rs_extractor();
    let first = extractor.extract(Path::new("math.rs"), source).unwrap();
    let second = extractor.extract(Path::new("math.rs"), source).unwrap();

    assert_eq!(first[0].signature_hash, second[0].signature_hash);
    assert_eq!(first[0].body_hash, second[0].body_hash);
    assert_eq!(first[0].id, second[0].id);
}

#[test]
fn body_change_detected_signature_stable() {
    let before = br#"
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    let after = br#"
fn add(a: i32, b: i32) -> i32 {
    println!("adding");
    a + b
}
"#;

    let extractor = rs_extractor();
    let first = extractor.extract(Path::new("math.rs"), before).unwrap();
    let second = extractor.extract(Path::new("math.rs"), after).unwrap();

    assert_eq!(first[0].signature_hash, second[0].signature_hash);
    assert_ne!(first[0].body_hash, second[0].body_hash);
}

#[test]
fn signature_change_detected() {
    let before = br#"
fn validate(input: &str) -> bool {
    !input.is_empty()
}
"#;
    let after = br#"
fn validate(input: &str, strict: bool) -> bool {
    if strict { !input.trim().is_empty() } else { !input.is_empty() }
}
"#;

    let extractor = rs_extractor();
    let first = extractor.extract(Path::new("auth.rs"), before).unwrap();
    let second = extractor.extract(Path::new("auth.rs"), after).unwrap();

    assert_ne!(first[0].signature_hash, second[0].signature_hash);
}

#[test]
fn extracts_call_and_implements_dependencies() {
    let source = br#"
trait Validator {
    fn validate(&self, input: &str) -> bool;
}

struct AuthManager;

impl Validator for AuthManager {
    fn validate(&self, input: &str) -> bool {
        check(input)
    }
}

fn check(input: &str) -> bool {
    !input.is_empty()
}
"#;

    let extractor = rs_extractor();
    let units = extractor.extract(Path::new("auth.rs"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    let check_id = units.iter().find(|unit| unit.name == "check").unwrap().id;
    let method_id = units
        .iter()
        .find(|unit| unit.qualified_name == "AuthManager::validate")
        .unwrap()
        .id;
    let auth_id = units
        .iter()
        .find(|unit| unit.name == "AuthManager" && unit.kind == UnitKind::Struct)
        .unwrap()
        .id;
    let trait_id = units
        .iter()
        .find(|unit| unit.name == "Validator" && unit.kind == UnitKind::Trait)
        .unwrap()
        .id;

    assert!(deps.iter().any(|(from, to, kind)| {
        *from == method_id && *to == check_id && *kind == DepKind::Calls
    }));
    assert!(deps.iter().any(|(from, to, kind)| {
        *from == auth_id && *to == trait_id && *kind == DepKind::Implements
    }));
}

#[test]
fn extracts_import_dependencies() {
    let source = br#"
use crate::parser::{parse_value as parse_input, ParserConfig};

fn run(raw: &str) -> bool {
    let _config = ParserConfig::default();
    parse_input(raw)
}
"#;

    let extractor = rs_extractor();
    let units = extractor.extract(Path::new("run.rs"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    assert!(deps.iter().any(|(_, _, kind)| *kind == DepKind::Imports));
}

#[test]
fn extractor_reports_correct_extensions() {
    let extractor = rs_extractor();
    assert_eq!(extractor.extensions(), ["rs"]);
}

#[test]
fn empty_file_produces_no_units() {
    let extractor = rs_extractor();
    let units = extractor.extract(Path::new("empty.rs"), b"").unwrap();
    assert!(units.is_empty());
}
