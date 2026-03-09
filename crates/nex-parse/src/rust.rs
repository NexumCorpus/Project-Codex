//! Rust semantic extractor.
//!
//! Implements SemanticExtractor using tree-sitter-rust.
//! Extracts: function_item, impl methods, struct_item, enum_item, trait_item,
//! and inline mod_item declarations.

use crate::SemanticExtractor;
use nex_core::{CodexError, CodexResult, DepKind, SemanticId, SemanticUnit, UnitKind};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

const EXTENSIONS: &[&str] = &["rs"];

/// Rust semantic extractor backed by tree-sitter.
#[derive(Debug, Default, Clone, Copy)]
pub struct RustExtractor;

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

impl RustExtractor {
    /// Create a new Rust extractor.
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE.into();
        parser
            .set_language(&language)
            .expect("tree-sitter Rust grammar must load");
        Self
    }

    fn parse(&self, path: &Path, source: &[u8]) -> CodexResult<Tree> {
        let mut parser = Parser::new();
        let language = tree_sitter_rust::LANGUAGE.into();
        parser
            .set_language(&language)
            .map_err(|err| parse_error(path, format!("failed to load Rust grammar: {err}")))?;

        parser
            .parse(source, None)
            .ok_or_else(|| parse_error(path, "tree-sitter returned no parse tree"))
    }

    fn analyze(&self, path: &Path, source: &[u8]) -> CodexResult<(Tree, Vec<UnitInfo>)> {
        let tree = self.parse(path, source)?;
        let root = tree.root_node();
        let mut units = Vec::new();
        self.collect_program_units(root, path, source, &mut units, &[])?;
        Ok((tree, units))
    }

    fn collect_program_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
        namespace: &[String],
    ) -> CodexResult<()> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "function_item" => {
                    if let Some(unit) = self.function_unit(child, path, source, namespace) {
                        units.push(unit);
                    }
                }
                "struct_item" => {
                    if let Some(unit) = self.struct_unit(child, path, source, namespace) {
                        units.push(unit);
                    }
                }
                "enum_item" => {
                    if let Some(unit) = self.enum_unit(child, path, source, namespace) {
                        units.push(unit);
                    }
                }
                "trait_item" => self.collect_trait_units(child, path, source, units, namespace)?,
                "mod_item" => self.collect_module_units(child, path, source, units, namespace)?,
                "impl_item" => self.collect_impl_units(child, path, source, units, namespace)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn function_unit(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        namespace: &[String],
    ) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");
        let qualified_name = qualify(namespace, &name);

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Function,
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

    fn struct_unit(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        namespace: &[String],
    ) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Struct,
                name.clone(),
                qualify(namespace, &name),
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage: Vec::new(),
        })
    }

    fn enum_unit(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        namespace: &[String],
    ) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Enum,
                name.clone(),
                qualify(namespace, &name),
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage: Vec::new(),
        })
    }

    fn collect_trait_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
        namespace: &[String],
    ) -> CodexResult<()> {
        let Some(name_node) = node.child_by_field_name("name") else {
            return Ok(());
        };
        let Some(name) = node_text(name_node, source) else {
            return Ok(());
        };
        let body = node.child_by_field_name("body");
        let qualified_name = qualify(namespace, &name);

        units.push(UnitInfo {
            unit: build_unit(
                UnitKind::Trait,
                name.clone(),
                qualified_name.clone(),
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage: collect_trait_heritage(node, source),
        });

        let Some(body) = body else {
            return Ok(());
        };

        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if !matches!(child.kind(), "function_item" | "function_signature_item") {
                continue;
            }
            if let Some(unit) = self.method_unit(child, &qualified_name, path, source) {
                units.push(unit);
            }
        }

        Ok(())
    }

    fn collect_module_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
        namespace: &[String],
    ) -> CodexResult<()> {
        let Some(name_node) = node.child_by_field_name("name") else {
            return Ok(());
        };
        let Some(name) = node_text(name_node, source) else {
            return Ok(());
        };
        let body = node.child_by_field_name("body");
        let qualified_name = qualify(namespace, &name);

        units.push(UnitInfo {
            unit: build_unit(
                UnitKind::Module,
                name.clone(),
                qualified_name,
                path,
                node.byte_range(),
                hash_u64(b""),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: None,
            heritage: Vec::new(),
        });

        let Some(body) = body else {
            return Ok(());
        };

        let mut nested = namespace.to_vec();
        nested.push(name);
        self.collect_program_units(body, path, source, units, &nested)
    }

    fn collect_impl_units(
        &self,
        node: Node<'_>,
        path: &Path,
        source: &[u8],
        units: &mut Vec<UnitInfo>,
        namespace: &[String],
    ) -> CodexResult<()> {
        let Some(type_node) = node.child_by_field_name("type") else {
            return Ok(());
        };
        let Some(type_name) = reference_name(type_node, source) else {
            return Ok(());
        };
        let Some(body) = node.child_by_field_name("body") else {
            return Ok(());
        };

        let container_name = qualify(namespace, &type_name);
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            if child.kind() != "function_item" {
                continue;
            }
            if let Some(unit) = self.method_unit(child, &container_name, path, source) {
                units.push(unit);
            }
        }

        Ok(())
    }

    fn method_unit(
        &self,
        node: Node<'_>,
        container_name: &str,
        path: &Path,
        source: &[u8],
    ) -> Option<UnitInfo> {
        let name_node = node.child_by_field_name("name")?;
        let name = node_text(name_node, source)?;
        let body = node.child_by_field_name("body");

        Some(UnitInfo {
            unit: build_unit(
                UnitKind::Method,
                name.clone(),
                format!("{container_name}::{name}"),
                path,
                node.byte_range(),
                signature_hash(node, source),
                body.map_or_else(|| hash_u64(b""), |body| normalized_body_hash(body, source)),
            ),
            body_range: body.map(|body| body.byte_range()),
            heritage: Vec::new(),
        })
    }
}

impl SemanticExtractor for RustExtractor {
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
            .unwrap_or_else(|| Path::new("<memory.rs>"));
        let (tree, analyzed_units) = self.analyze(path, source)?;
        let root = tree.root_node();

        let unit_by_id: HashMap<SemanticId, &UnitInfo> = analyzed_units
            .iter()
            .map(|unit| (unit.unit.id, unit))
            .collect();

        let mut ids_by_name: HashMap<&str, Vec<SemanticId>> = HashMap::new();
        for unit in units.iter().filter(|unit| is_rust_unit(unit)) {
            ids_by_name
                .entry(unit.name.as_str())
                .or_default()
                .push(unit.id);
        }

        let imports = collect_imports(root, source);
        let impl_relationships = collect_impl_relationships(root, source);
        let mut edges: HashSet<(SemanticId, SemanticId, DepKind)> = HashSet::new();

        for (type_name, trait_name) in impl_relationships {
            let Some(type_ids) = ids_by_name.get(type_name.as_str()) else {
                continue;
            };
            let Some(trait_ids) = ids_by_name.get(trait_name.as_str()) else {
                continue;
            };
            for type_id in type_ids {
                for trait_id in trait_ids {
                    if type_id != trait_id {
                        edges.insert((*type_id, *trait_id, DepKind::Implements));
                    }
                }
            }
        }

        for unit in units.iter().filter(|unit| is_rust_unit(unit)) {
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

fn collect_trait_heritage(node: Node<'_>, source: &[u8]) -> Vec<(String, DepKind)> {
    let bounds = node
        .child_by_field_name("bounds")
        .filter(|bounds| bounds.kind() == "trait_bounds")
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|child| child.kind() == "trait_bounds")
        });
    let Some(bounds) = bounds else {
        return Vec::new();
    };

    let mut heritage = Vec::new();
    let mut cursor = bounds.walk();
    for child in bounds.named_children(&mut cursor) {
        if let Some(name) = reference_name(child, source) {
            heritage.push((name, DepKind::Inherits));
        }
    }
    heritage
}

fn collect_impl_relationships(root: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    collect_impl_relationships_recursive(root, source, &mut edges);
    edges
}

fn collect_impl_relationships_recursive(
    node: Node<'_>,
    source: &[u8],
    edges: &mut Vec<(String, String)>,
) {
    if node.kind() == "impl_item"
        && let Some(type_node) = node.child_by_field_name("type")
        && let Some(type_name) = reference_name(type_node, source)
        && let Some(trait_node) = node.child_by_field_name("trait")
        && let Some(trait_name) = reference_name(trait_node, source)
    {
        edges.push((type_name, trait_name));
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_impl_relationships_recursive(child, source, edges);
    }
}

fn collect_imports(root: Node<'_>, source: &[u8]) -> Vec<ImportSymbol> {
    let mut imports = Vec::new();
    collect_imports_recursive(root, source, &mut imports);
    imports
}

fn collect_imports_recursive(node: Node<'_>, source: &[u8], imports: &mut Vec<ImportSymbol>) {
    if node.kind() == "use_declaration"
        && let Some(statement) = node_text(node, source)
    {
        imports.extend(import_symbols_from_statement(&statement));
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_imports_recursive(child, source, imports);
    }
}

fn import_symbols_from_statement(statement: &str) -> Vec<ImportSymbol> {
    let trimmed = statement.trim();
    let Some(use_index) = trimmed.find("use ") else {
        return Vec::new();
    };
    let body = trimmed[use_index + 4..].trim().trim_end_matches(';').trim();
    if body.is_empty() {
        return Vec::new();
    }

    expand_use_clause(body)
}

fn expand_use_clause(body: &str) -> Vec<ImportSymbol> {
    if let Some((prefix, inner)) = split_use_group(body) {
        let prefix = prefix.trim_end_matches("::");
        let mut imports = Vec::new();
        for item in split_top_level(inner, ',') {
            let item = item.trim();
            if item.is_empty() || item == "*" {
                continue;
            }
            if item == "self" {
                if !prefix.is_empty() {
                    imports.push(ImportSymbol {
                        local_name: tail_name(prefix).to_string(),
                        module_name: prefix.to_string(),
                    });
                }
                continue;
            }

            let combined = if prefix.is_empty() {
                item.to_string()
            } else {
                format!("{prefix}::{item}")
            };
            imports.extend(expand_use_clause(&combined));
        }
        return imports;
    }

    if body.ends_with("::*") {
        return Vec::new();
    }

    if let Some((path, alias)) = body.rsplit_once(" as ") {
        let local_name = alias.trim();
        if local_name.is_empty() {
            return Vec::new();
        }
        return vec![ImportSymbol {
            local_name: local_name.to_string(),
            module_name: path.trim().to_string(),
        }];
    }

    vec![ImportSymbol {
        local_name: tail_name(body).to_string(),
        module_name: body.to_string(),
    }]
}

fn split_use_group(body: &str) -> Option<(&str, &str)> {
    let start = body.find('{')?;
    let mut depth = 0usize;
    let mut end = None;

    for (index, ch) in body.char_indices().skip(start) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end?;
    Some((&body[..start], &body[start + 1..end]))
}

fn split_top_level(input: &str, delimiter: char) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;

    for (index, ch) in input.char_indices() {
        match ch {
            '{' | '(' | '[' | '<' => depth += 1,
            '}' | ')' | ']' | '>' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if ch == delimiter && depth == 0 {
            items.push(input[start..index].to_string());
            start = index + ch.len_utf8();
        }
    }

    if start <= input.len() {
        items.push(input[start..].to_string());
    }

    items
}

fn collect_called_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_called_names_recursive(node, source, &mut names);
    names
}

fn collect_called_names_recursive(node: Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    if node.kind() == "call_expression"
        && let Some(function_node) = node.child_by_field_name("function")
        && let Some(name) = reference_name(function_node, source)
    {
        names.insert(name);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_called_names_recursive(child, source, names);
    }
}

fn collect_identifier_names(node: Node<'_>, source: &[u8]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_identifier_names_recursive(node, source, &mut names);
    names
}

fn collect_identifier_names_recursive(node: Node<'_>, source: &[u8], names: &mut HashSet<String>) {
    if matches!(
        node.kind(),
        "identifier" | "type_identifier" | "field_identifier"
    ) && let Some(name) = node_text(node, source)
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
        "identifier" | "type_identifier" | "field_identifier" => node_text(node, source),
        "scoped_identifier" | "scoped_type_identifier" => node
            .child_by_field_name("name")
            .and_then(|name| reference_name(name, source))
            .or_else(|| {
                let mut last = None;
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    if let Some(name) = reference_name(child, source) {
                        last = Some(name);
                    }
                }
                last
            }),
        "field_expression" => node
            .child_by_field_name("field")
            .and_then(|field| reference_name(field, source))
            .or_else(|| {
                node.child_by_field_name("value")
                    .and_then(|value| reference_name(value, source))
            }),
        "generic_type"
        | "generic_function"
        | "reference_type"
        | "pointer_type"
        | "array_type"
        | "tuple_type"
        | "parenthesized_type"
        | "qualified_type"
        | "type_arguments"
        | "reference_expression"
        | "await_expression"
        | "try_expression"
        | "parenthesized_expression" => {
            for field_name in ["type", "value", "function"] {
                if let Some(child) = node.child_by_field_name(field_name)
                    && let Some(name) = reference_name(child, source)
                {
                    return Some(name);
                }
            }

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

fn is_rust_unit(unit: &SemanticUnit) -> bool {
    unit.file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| EXTENSIONS.contains(&ext))
}

fn qualify(namespace: &[String], name: &str) -> String {
    if namespace.is_empty() {
        return name.to_string();
    }
    format!("{}::{name}", namespace.join("::"))
}

fn tail_name(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn parse_error(path: &Path, message: impl Into<String>) -> CodexError {
    CodexError::Parse {
        path: path.display().to_string(),
        message: message.into(),
    }
}
