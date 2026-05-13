use crate::common::{classify_call_target, last_identifier, walk_descendants, Scanner};
use grafly_core::{ArtifactKind, DependencyKind, ScanResult, Visibility};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    scan_with_language(path, source, tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
}

pub fn scan_tsx(path: &Path, source: &str) -> ScanResult {
    scan_with_language(path, source, tree_sitter_typescript::LANGUAGE_TSX.into())
}

fn scan_with_language(
    path: &Path,
    source: &str,
    language: tree_sitter::Language,
) -> ScanResult {
    let mut parser = Parser::new();
    parser.set_language(&language).expect("TypeScript grammar");

    let tree = match parser.parse(source, None) {
        Some(t) => t,
        None => return ScanResult::default(),
    };

    let file_id = path.to_string_lossy().replace('\\', "/");
    let file_label = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_id.clone());

    let mut s = Scanner::new(file_id.clone(), source);
    s.add_file(file_label);

    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        scan_ts_node(&child, &file_id, false, &mut s);
    }

    s.result
}

fn scan_ts_node(child: &Node, file_id: &str, exported: bool, s: &mut Scanner) {
    let vis = if exported {
        Visibility::Public
    } else {
        Visibility::Private
    };
    match child.kind() {
        "class_declaration" | "abstract_class_declaration" => emit_class(child, file_id, vis, s),

        "interface_declaration" => {
            let name = s.field_text(child, "name");
            if name.is_empty() {
                return;
            }
            let line = child.start_position().row + 1;
            let id = format!("{}::interface::{}", file_id, name);
            s.add_artifact_with_visibility(id.clone(), name, ArtifactKind::Interface, line, vis);
            s.contains(file_id, &id, line);
        }

        "type_alias_declaration" => {
            let name = s.field_text(child, "name");
            if name.is_empty() {
                return;
            }
            let line = child.start_position().row + 1;
            let id = format!("{}::type::{}", file_id, name);
            s.add_artifact_with_visibility(id.clone(), name, ArtifactKind::Struct, line, vis);
            s.contains(file_id, &id, line);
        }

        "enum_declaration" => {
            let name = s.field_text(child, "name");
            if name.is_empty() {
                return;
            }
            let line = child.start_position().row + 1;
            let id = format!("{}::enum::{}", file_id, name);
            s.add_artifact_with_visibility(id.clone(), name, ArtifactKind::Enum, line, vis);
            s.contains(file_id, &id, line);
        }

        "function_declaration" => emit_function(child, file_id, file_id, None, vis, s),

        "import_statement" => {
            let line = child.start_position().row + 1;
            for name in extract_import_names(child, s) {
                s.record_import(file_id, name, line);
            }
        }

        "export_statement" => {
            let mut ec = child.walk();
            for inner in child.children(&mut ec) {
                scan_ts_node(&inner, file_id, true, s);
            }
        }

        _ => {}
    }
}

fn emit_class(node: &Node, file_id: &str, class_vis: Visibility, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let class_id = format!("{}::class::{}", file_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact_with_visibility(
        class_id.clone(),
        name.clone(),
        ArtifactKind::Class,
        line,
        class_vis,
    );
    s.contains(file_id, &class_id, line);

    if let Some(heritage) = node.child_by_field_name("heritage") {
        let mut hc = heritage.walk();
        for clause in heritage.children(&mut hc) {
            let kind = match clause.kind() {
                "extends_clause" => Some(DependencyKind::Extends),
                "implements_clause" => Some(DependencyKind::Implements),
                _ => None,
            };
            if let Some(rel) = kind {
                walk_descendants(clause, |n| {
                    if matches!(n.kind(), "identifier" | "type_identifier") {
                        let raw = node_text(&n, s.source);
                        let id = last_identifier(raw);
                        if !id.is_empty() && id != name {
                            s.record_unresolved(
                                &class_id,
                                id,
                                None,
                                rel.clone(),
                                n.start_position().row + 1,
                            );
                        }
                    }
                });
            }
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for method in body.children(&mut bc) {
            if matches!(
                method.kind(),
                "method_definition" | "public_field_definition"
            ) {
                let mname = s.field_text(&method, "name");
                if mname.is_empty() {
                    continue;
                }
                let mid = format!("{}::method::{}", class_id, mname);
                let mline = method.start_position().row + 1;
                // TS method visibility: explicit accessibility modifier wins,
                // else `#`-prefix is private, else inherit class visibility.
                let mvis = ts_method_visibility(&method, &mname, class_vis, s);
                s.add_artifact_with_visibility(
                    mid.clone(),
                    mname,
                    ArtifactKind::Method,
                    mline,
                    mvis,
                );
                s.contains(&class_id, &mid, mline);
                if let Some(mbody) = method.child_by_field_name("body") {
                    walk_for_calls(mbody, &mid, Some(&name), s);
                }
            }
        }
    }
}

/// Resolve a TS class member's visibility, in priority order:
/// 1. `#name` (ES2022 private field) → Private
/// 2. Explicit `public` / `protected` / `private` accessibility modifier
/// 3. Inherit class visibility (Public if class is exported, else Private)
fn ts_method_visibility(
    method: &Node,
    mname: &str,
    class_vis: Visibility,
    s: &Scanner,
) -> Visibility {
    if mname.starts_with('#') {
        return Visibility::Private;
    }
    let mut c = method.walk();
    for inner in method.children(&mut c) {
        if inner.kind() == "accessibility_modifier" {
            return match s.text(&inner).trim() {
                "public" => Visibility::Public,
                "protected" => Visibility::Crate,
                "private" => Visibility::Private,
                _ => class_vis,
            };
        }
    }
    class_vis
}

fn emit_function(
    node: &Node,
    file_id: &str,
    parent_id: &str,
    enclosing_type: Option<&str>,
    vis: Visibility,
    s: &mut Scanner,
) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let fn_id = format!("{}::fn::{}", file_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact_with_visibility(fn_id.clone(), name, ArtifactKind::Function, line, vis);
    s.contains(parent_id, &fn_id, line);
    if let Some(body) = node.child_by_field_name("body") {
        walk_for_calls(body, &fn_id, enclosing_type, s);
    }
}

fn walk_for_calls(body: Node, caller_id: &str, enclosing_type: Option<&str>, s: &mut Scanner) {
    walk_descendants(body, |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        let raw = node_text(&func, s.source);
        let (callee, receiver) = classify_call_target(raw, "this", enclosing_type);

        if callee.is_empty() || TS_BUILTINS.contains(&callee.as_str()) {
            return;
        }
        s.record_call(caller_id, callee, receiver, node.start_position().row + 1);
    });
}

fn extract_import_names(node: &Node, s: &Scanner) -> Vec<String> {
    let mut names = Vec::new();
    walk_descendants(*node, |n| {
        if matches!(
            n.kind(),
            "identifier" | "import_specifier" | "namespace_import" | "type_identifier"
        ) {
            let raw = node_text(&n, s.source);
            let id = last_identifier(raw);
            if !id.is_empty() && !names.contains(&id) {
                names.push(id);
            }
        }
    });
    names
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

const TS_BUILTINS: &[&str] = &[
    "console", "log", "error", "warn", "info", "debug", "trace",
    "Array", "Object", "String", "Number", "Boolean", "Date", "RegExp",
    "Promise", "Map", "Set", "Symbol", "JSON", "Math",
    "parseInt", "parseFloat", "isNaN", "isFinite",
    "setTimeout", "setInterval", "clearTimeout", "clearInterval",
    "require", "import", "fetch", "alert", "prompt", "confirm",
    "push", "pop", "shift", "unshift", "slice", "splice", "concat",
    "map", "filter", "reduce", "forEach", "find", "some", "every",
    "toString", "valueOf", "hasOwnProperty",
];
