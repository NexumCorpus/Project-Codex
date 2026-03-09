//! Integration tests for the Python SemanticExtractor.
//!
//! These tests extend the Phase 0 acceptance criteria to the Python P0
//! extractor called out in the implementation spec.

use nex_core::{DepKind, UnitKind};
use nex_parse::SemanticExtractor;
use std::path::Path;

fn py_extractor() -> Box<dyn SemanticExtractor> {
    nex_parse::python_extractor()
}

#[test]
fn extracts_top_level_function_and_class_methods() {
    let source = br#"
def greet(name: str) -> str:
    return f"hello {name}"

class AuthManager:
    def validate_token(self, token: str) -> bool:
        return len(token) > 0

    async def refresh_session(self) -> str:
        return "ok"
"#;

    let extractor = py_extractor();
    let units = extractor.extract(Path::new("auth.py"), source).unwrap();

    assert!(units.iter().any(|unit| {
        unit.name == "greet" && unit.kind == UnitKind::Function && unit.qualified_name == "greet"
    }));
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "AuthManager" && unit.kind == UnitKind::Class)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "validate_token" && unit.kind == UnitKind::Method)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "refresh_session" && unit.kind == UnitKind::Method)
    );
}

#[test]
fn extracts_decorated_function_and_named_lambda() {
    let source = br#"
@cached
def load_user(user_id: str) -> str:
    return user_id

normalize_name = lambda name: name.strip()
"#;

    let extractor = py_extractor();
    let units = extractor.extract(Path::new("users.py"), source).unwrap();

    assert!(
        units
            .iter()
            .any(|unit| unit.name == "load_user" && unit.kind == UnitKind::Function)
    );
    assert!(
        units
            .iter()
            .any(|unit| unit.name == "normalize_name" && unit.kind == UnitKind::Function)
    );
}

#[test]
fn method_qualified_name_includes_class() {
    let source = br#"
class PaymentProcessor:
    def process_payment(self, amount: int) -> int:
        return amount
"#;

    let extractor = py_extractor();
    let units = extractor.extract(Path::new("payment.py"), source).unwrap();
    let method = units
        .iter()
        .find(|unit| unit.name == "process_payment")
        .unwrap();

    assert_eq!(method.qualified_name, "PaymentProcessor::process_payment");
}

#[test]
fn same_source_produces_same_hashes() {
    let source = br#"
def add(a: int, b: int) -> int:
    return a + b
"#;

    let extractor = py_extractor();
    let first = extractor.extract(Path::new("math.py"), source).unwrap();
    let second = extractor.extract(Path::new("math.py"), source).unwrap();

    assert_eq!(first[0].signature_hash, second[0].signature_hash);
    assert_eq!(first[0].body_hash, second[0].body_hash);
    assert_eq!(first[0].id, second[0].id);
}

#[test]
fn body_change_detected_signature_stable() {
    let before = br#"
def add(a: int, b: int) -> int:
    return a + b
"#;
    let after = br#"
def add(a: int, b: int) -> int:
    print("adding")
    return a + b
"#;

    let extractor = py_extractor();
    let first = extractor.extract(Path::new("math.py"), before).unwrap();
    let second = extractor.extract(Path::new("math.py"), after).unwrap();

    assert_eq!(first[0].signature_hash, second[0].signature_hash);
    assert_ne!(first[0].body_hash, second[0].body_hash);
}

#[test]
fn signature_change_detected() {
    let before = br#"
def validate(token: str) -> bool:
    return len(token) > 0
"#;
    let after = br#"
def validate(token: str, strict: bool) -> bool:
    return len(token.strip()) > 0 if strict else len(token) > 0
"#;

    let extractor = py_extractor();
    let first = extractor.extract(Path::new("auth.py"), before).unwrap();
    let second = extractor.extract(Path::new("auth.py"), after).unwrap();

    assert_ne!(first[0].signature_hash, second[0].signature_hash);
}

#[test]
fn extracts_call_and_inheritance_dependencies() {
    let source = br#"
class BaseHandler:
    pass

class RequestHandler(BaseHandler):
    def handle(self, request: str) -> bool:
        return validate(request)

def validate(token: str) -> bool:
    return len(token) > 0
"#;

    let extractor = py_extractor();
    let units = extractor.extract(Path::new("handler.py"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    let validate_id = units
        .iter()
        .find(|unit| unit.name == "validate")
        .unwrap()
        .id;
    let handle_id = units.iter().find(|unit| unit.name == "handle").unwrap().id;
    let base_id = units
        .iter()
        .find(|unit| unit.name == "BaseHandler")
        .unwrap()
        .id;
    let request_handler_id = units
        .iter()
        .find(|unit| unit.name == "RequestHandler")
        .unwrap()
        .id;

    assert!(deps.iter().any(|(from, to, kind)| {
        *from == handle_id && *to == validate_id && *kind == DepKind::Calls
    }));
    assert!(deps.iter().any(|(from, to, kind)| {
        *from == request_handler_id && *to == base_id && *kind == DepKind::Inherits
    }));
}

#[test]
fn extracts_import_dependencies() {
    let source = br#"
from validators import validate as validate_input
import logging

def process_request(req: str) -> None:
    validate_input(req)
    logging.info(req)
"#;

    let extractor = py_extractor();
    let units = extractor.extract(Path::new("handler.py"), source).unwrap();
    let deps = extractor.dependencies(&units, source).unwrap();

    assert!(deps.iter().any(|(_, _, kind)| *kind == DepKind::Imports));
}

#[test]
fn extractor_reports_correct_extensions() {
    let extractor = py_extractor();
    assert_eq!(extractor.extensions(), ["py"]);
}

#[test]
fn empty_file_produces_no_units() {
    let extractor = py_extractor();
    let units = extractor.extract(Path::new("empty.py"), b"").unwrap();
    assert!(units.is_empty());
}
