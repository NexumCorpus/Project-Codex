//! TypeScript semantic extractor.
//!
//! Implements SemanticExtractor using tree-sitter-typescript.
//! Extracts: function_declaration, class_declaration, method_definition,
//! interface_declaration, named arrow_function.

use crate::SemanticExtractor;
use nex_core::{CodexError, CodexResult, DepKind, SemanticId, SemanticUnit, UnitKind};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

const EXTENSIONS: &[&str] = &["ts", "tsx"];

/// TypeScript/TSX semantic extractor backed by tree-sitter.
#[derive(Debug, Default, Clone, Copy)]
pub struct TypeScriptExtractor;

#[derive(Debug, Clone)]
struct UnitInfo {
    unit: SemanticUnit,
    body_range: Option<Range<usize>>,
    heritage: Vec<(String, DepKind)>,
}

#[derive(Debug, Clone)]
struct ImportSymbol {
    local_name: String,
    module_name: String,
}

impl TypeScriptExtractor {
    /// Create a new TypeScript extractor.
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        parser
            .set_language(&language)
            .expect("tree-sitter TypeScript grammar must load");
        Self
    }

    fn parse(&self, path: &Path, source: &[u8]) -> CodexResult<Tree> {
        let mut parser = Parser::new();
        let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        parser.set_language(&language).map_err(|err| {
            parse_error(path, format!("failed to load TypeScript grammar: {err}"))
        })?;

        parser
            .parse(source, None)
            .ok_or_else(|| parse_error(path, "tree-sitter returned no parse tree"))
    }

    fn analyze(&self, path: &Path, source: &[u8]) -> CodexResult<(Tree, Vec<UnitInfo>)> {
        let tree = self.parse(path, source)?;
        let root = tree.root_node();
        let mut units = Vec::new();
        self.collect_program_units(root, path, source, &mut units)?;
        Ok((tree, units))
    }

    fn collect_program_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
    ) -> CodexResult<()> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_declaration" => {
                    if let Some(unit) = self.function_unit(child, path, source) {
                        units.push(unit);
                    }
                }
                "class_declaration" => self.collect_class_units(child, path, source, units)?,
                "interface_declaration" => {
                    if let Some(unit) = self.interface_unit(child, path, source) {
                        units.push(unit);
                    }
                }
                "lexical_declaration" => self.collect_arrow_units(child, path, source, units),
                "export_statement" | "export_default_declaration" => {
                    self.collect_program_units(child, path, source, units)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn collect_class_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
    ) -> CodexResult<()> {
        let Some(class_name_node) = node.child_by_field_name("name") else {
            return Ok(());
        };
        let Some(class_name) = node_text(class_name_node, source) else {
            return Ok(());
        };

        let body = node.child_by_field_name("body");
        let heritage = collect_class_heritage(node, source);
        units.push(UnitInfo {
            unit: build_unit(
                UnitKind::Class,
                class_name.clone(),
                class_name.clone(),
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage,
        });

        let Some(body) = body else {
            return Ok(());
        };

        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() != "method_definition" {
                continue;
            }
            if let Some(method_unit) = self.method_unit(child, &class_name, path, source) {
                units.push(method_unit);
            }
        }

        Ok(())
    }

    fn collect_arrow_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
    ) {
        let Some(kind_node) = node.child_by_field_name("kind") else {
            return;
        };
        let Some(kind_text) = node_text(kind_node, source) else {
            return;
        };
        if kind_text != "const" {
            return;
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "variable_declarator" {
                continue;
            }
            if let Some(unit) = self.arrow_function_unit(child, path, source) {
                units.push(unit);
            }
        }
    }

    fn function_unit(&self, node: Node<'_>, path: &Path, source: &[u8]) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Function,
                name.clone(),
                name,
                path,
                node.byte_range(),
                signature_hash(node, source),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: body.map(|body| body.byte_range()),
            heritage: Vec::new(),
        })
    }

    fn method_unit(
        &self,
        node: Node<'_>,
        class_name: &str,
        path: &Path,
        source: &[u8],
    ) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = property_name(name_node, source)?;
        let qualified_name = format!("{class_name}::{name}");
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Method,
                name,
                qualified_name,
                path,
                node.byte_range(),
                signature_hash(node, source),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: body.map(|body| body.byte_range()),
            heritage: Vec::new(),
        })
    }

    fn interface_unit(&self, node: Node<'_>, path: &Path, source: &[u8]) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Interface,
                name.clone(),
                name,
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage: Vec::new(),
        })
    }

    fn arrow_function_unit(&self, node: Node<'_>, path: &Path, source: &[u8]) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        if name_node.kind() != "identifier" {
            return None;
        }
        let name = node_text(name_node, source)?;
        let value = node.child_by_field_name("value")?;
        if value.kind() != "arrow_function" {
            return None;
        }

        let body = value.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Function,
                name.clone(),
                name,
                path,
                node.byte_range(),
                signature_hash(value, source),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: body.map(|body| body.byte_range()),
            heritage: Vec::new(),
        })
    }
}

impl SemanticExtractor for TypeScriptExtractor {
    fn extensions(&self) -> &[&str] {
        EXTENSIONS
    }

    fn extract(&self, path: &Path, source: &[u8]) -> CodexResult<Vec<SemanticUnit>> {
        let (_, units) = self.analyze(path, source)?;
        Ok(units.into_iter().map(|info| info.unit).collect())
    }

    fn dependencies(
        &self,
        units: &[SemanticUnit],
        source: &[u8],
    ) -> CodexResult<Vec<(SemanticId, SemanticId, DepKind)>> {
        if units.is_empty() {
            return Ok(Vec::new());
        }

        let path = units
            .first()
            .map(|unit| unit.file_path.as_path())
            .unwrap_or_else(|| Path::new("<memory.ts>"));
        let (tree, analyzed_units) = self.analyze(path, source)?;
        let root = tree.root_node();

        let unit_by_id: HashMap<SemanticId, &UnitInfo> = analyzed_units
            .iter()
            .map(|unit| (unit.unit.id, unit))
            .collect();

        let mut ids_by_name: HashMap<&str, Vec<SemanticId>> = HashMap::new();
        for unit in units.iter().filter(|unit| is_typescript_unit(unit)) {
            ids_by_name
                .entry(unit.name.as_str())
                .or_default()
                .push(unit.id);
        }

        let imports = collect_imports(root, source);
        let mut edges: HashSet<(SemanticId, SemanticId, DepKind)> = HashSet::new();

        for unit in units.iter().filter(|unit| is_typescript_unit(unit)) {
            let Some(info) = unit_by_id.get(&unit.id).copied() else {
                continue;
            };

            for (target_name, dep_kind) in &info.heritage {
                if let Some(target_ids) = ids_by_name.get(target_name.as_str()) {
                    for target_id in target_ids {
                        if *target_id != unit.id {
                            edges.insert((unit.id, *target_id, *dep_kind));
                        }
                    }
                }
            }

            let Some(body_range) = &info.body_range else {
                continue;
            };
            let Some(body_node) = find_node_by_range(root, body_range) else {
                continue;
            };

            let called_names = collect_called_names(body_node, source);
            for called_name in called_names {
                if let Some(target_ids) = ids_by_name.get(called_name.as_str()) {
                    for target_id in target_ids {
                        if *target_id != unit.id {
                            edges.insert((unit.id, *target_id, DepKind::Calls));
                        }
                    }
                }
            }

            let used_identifiers = collect_identifier_names(body_node, source);
            for import in &imports {
                if used_identifiers.contains(import.local_name.as_str()) {
                    edges.insert((
                        unit.id,
                        external_unit_id(&import.module_name, &import.local_name),
                        DepKind::Imports,
                    ));
                }
            }
        }

        let mut edges: Vec<_> = edges.into_iter().collect();
        edges.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then(dep_kind_rank(left.2).cmp(&dep_kind_rank(right.2)))
        });
        Ok(edges)
    }
}

fn build_unit(
    kind: UnitKind,
    name: String,
    qualified_name: String,
    path: &Path,
    byte_range: Range<usize>,
    signature_hash: u64,
    body_hash: u64,
) -> SemanticUnit {
    SemanticUnit {
        id: semantic_id(&qualified_name, path),
        kind,
        name,
        qualified_name,
        file_path: path.to_path_buf(),
        byte_range,
        signature_hash,
        body_hash,
        dependencies: Vec::new(),
    }
}

fn semantic_id(qualified_name: &str, path: &Path) -> SemanticId {
    let digest = blake3::hash(format!("{}:{}", qualified_name, path.display()).as_bytes());
    *digest.as_bytes()
}

fn external_unit_id(module_name: &str, symbol_name: &str) -> SemanticId {
    let digest = blake3::hash(format!("external:{module_name}:{symbol_name}").as_bytes());
    *digest.as_bytes()
}

fn hash_u64(bytes: &[u8]) -> u64 {
    let digest = blake3::hash(bytes);
    let mut truncated = [0u8; 8];
    truncated.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(truncated)
}

fn signature_hash(node: Node<'_>, source: &[u8]) -> u64 {
    let params = node
        .child_by_field_name("parameters")
        .or_else(|| node.child_by_field_name("parameter"))
        .and_then(|params| node_text(params, source))
        .unwrap_or_default();
    let return_type = node
        .child_by_field_name("return_type")
        .and_then(|return_type| node_text(return_type, source))
        .unwrap_or_default();

    let mut signature = params;
    signature.push_str(&return_type);
    hash_u64(signature.as_bytes())
}

fn normalized_body_hash(node: Node<'_>, source: &[u8]) -> u64 {
    let mut tokens = Vec::new();
    collect_leaf_tokens(node, source, &mut tokens);
    hash_u64(tokens.join(" ").as_bytes())
}

fn collect_leaf_tokens(node: Node<'_>, source: &[u8], tokens: &mut Vec<String>) {
    if node.is_extra() || node.kind() == "comment" {
        return;
    }

    if node.child_count() == 0 {
        if let Some(text) = node_text(node, source)
            && !text.trim().is_empty()
        {
            tokens.push(text);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_leaf_tokens(child, source, tokens);
    }
}

fn collect_class_heritage(node: Node<'_>, source: &[u8]) -> Vec<(String, DepKind)> {
    let mut heritage = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "class_heritage" {
            continue;
        }

        let mut heritage_cursor = child.walk();
        for clause in child.named_children(&mut heritage_cursor) {
            match clause.kind() {
                "extends_clause" => {
                    if let Some(value) = clause.child_by_field_name("value")
                        && let Some(name) = reference_name(value, source)
                    {
                        heritage.push((name, DepKind::Inherits));
                    }
                }
                "implements_clause" => {
                    let mut clause_cursor = clause.walk();
                    for implemented in clause.named_children(&mut clause_cursor) {
                        if let Some(name) = reference_name(implemented, source) {
                            heritage.push((name, DepKind::Implements));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    heritage
}

fn collect_imports(root: Node<'_>, source: &[u8]) -> Vec<ImportSymbol> {
    let mut imports = Vec::new();
    collect_imports_recursive(root, source, &mut imports);
    imports
}

fn collect_imports_recursive(node: Node<'_>, source: &[u8], imports: &mut Vec<ImportSymbol>) {
    if node.kind() == "import_statement" {
        imports.extend(import_symbols_from_statement(node, source));
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_imports_recursive(child, source, imports);
    }
}

fn import_symbols_from_statement(node: Node<'_>, source: &[u8]) -> Vec<ImportSymbol> {
    let module_name = node
        .child_by_field_name("source")
        .and_then(|source_node| node_text(source_node, source))
        .unwrap_or_default();
    if module_name.is_empty() {
        return Vec::new();
    }

    let mut symbols = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "import_clause" {
            continue;
        }

        let mut clause_cursor = child.walk();
        for clause_child in child.named_children(&mut clause_cursor) {
            match clause_child.kind() {
                "identifier" => {
                    if let Some(name) = node_text(clause_child, source) {
                        symbols.push(ImportSymbol {
                            local_name: name,
                            module_name: module_name.clone(),
                        });
                    }
                }
                "namespace_import" => {
                    let mut namespace_cursor = clause_child.walk();
                    for namespace_child in clause_child.named_children(&mut namespace_cursor) {
                        if namespace_child.kind() == "identifier"
                            && let Some(name) = node_text(namespace_child, source)
                        {
                            symbols.push(ImportSymbol {
                                local_name: name,
                                module_name: module_name.clone(),
                            });
                        }
                    }
                }
                "named_imports" => {
                    let mut import_cursor = clause_child.walk();
                    for specifier in clause_child.named_children(&mut import_cursor) {
                        if specifier.kind() != "import_specifier" {
                            continue;
                        }
                        let local_name = specifier
                            .child_by_field_name("alias")
                            .or_else(|| specifier.child_by_field_name("name"))
                            .and_then(|name_node| node_text(name_node, source));
                        if let Some(local_name) = local_name {
                            symbols.push(ImportSymbol {
                                local_name,
                                module_name: module_name.clone(),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    symbols
}

fn collect_called_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_called_names_recursive(node, source, &mut names);
    names
}

fn collect_called_names_recursive(node: Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    if node.kind() == "call_expression"
        && let Some(function_node) = node.child_by_field_name("function")
        && let Some(name) = callee_name(function_node, source)
    {
        names.insert(name);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_called_names_recursive(child, source, names);
    }
}

fn callee_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "property_identifier" | "private_property_identifier" => {
            node_text(node, source)
        }
        "member_expression" => node
            .child_by_field_name("property")
            .and_then(|property| callee_name(property, source)),
        "instantiation_expression"
        | "parenthesized_expression"
        | "non_null_expression"
        | "as_expression"
        | "satisfies_expression"
        | "type_assertion" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "type_arguments" {
                    continue;
                }
                if let Some(name) = callee_name(child, source) {
                    return Some(name);
                }
            }
            None
        }
        _ => None,
    }
}

fn collect_identifier_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_identifier_names_recursive(node, source, &mut names);
    names
}

fn collect_identifier_names_recursive(node: Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    if matches!(node.kind(), "identifier" | "type_identifier")
        && let Some(name) = node_text(node, source)
    {
        names.insert(name);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_names_recursive(child, source, names);
    }
}

fn reference_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "type_identifier" | "property_identifier" => node_text(node, source),
        "member_expression" => node
            .child_by_field_name("property")
            .and_then(|property| reference_name(property, source))
            .or_else(|| {
                node.child_by_field_name("object")
                    .and_then(|object| reference_name(object, source))
            }),
        "generic_type"
        | "type_annotation"
        | "nested_type_identifier"
        | "predefined_type"
        | "lookup_type" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(name) = reference_name(child, source) {
                    return Some(name);
                }
            }
            None
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if let Some(name) = reference_name(child, source) {
                    return Some(name);
                }
            }
            None
        }
    }
}

fn property_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "property_identifier" | "private_property_identifier" | "identifier" => {
            node_text(node, source)
        }
        "computed_property_name" => None,
        _ => node_text(node, source),
    }
}

fn node_text(node: Node<'_>, source: &[u8]) -> Option<String> {
    node.utf8_text(source).ok().map(ToOwned::to_owned)
}

fn find_node_by_range<'tree>(node: Node<'tree>, range: &Range<usize>) -> Option<Node<'tree>> {
    if node.start_byte() == range.start && node.end_byte() == range.end {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() <= range.start
            && child.end_byte() >= range.end
            && let Some(found) = find_node_by_range(child, range)
        {
            return Some(found);
        }
    }
    None
}

fn dep_kind_rank(kind: DepKind) -> u8 {
    match kind {
        DepKind::Calls => 0,
        DepKind::Imports => 1,
        DepKind::Inherits => 2,
        DepKind::Implements => 3,
        DepKind::Uses => 4,
    }
}

fn is_typescript_unit(unit: &SemanticUnit) -> bool {
    unit.file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext))
}

fn parse_error(path: &Path, message: impl Into<String>) -> CodexError {
    CodexError::Parse {
        path: path.display().to_string(),
        message: message.into(),
    }
}
