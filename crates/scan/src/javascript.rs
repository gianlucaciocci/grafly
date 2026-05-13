use crate::common::{classify_call_target, last_identifier, walk_descendants, Scanner};
use grafly_core::{ArtifactKind, ScanResult, Visibility};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .expect("JavaScript grammar");

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
        scan_js_node(&child, &file_id, false, &mut s);
    }

    s.result
}

fn scan_js_node(child: &Node, file_id: &str, exported: bool, s: &mut Scanner) {
    match child.kind() {
        "class_declaration" => emit_class(child, file_id, exported, s),

        "function_declaration" => emit_function(child, file_id, file_id, None, exported, s),

        "lexical_declaration" | "variable_declaration" => {
            let mut dc = child.walk();
            for decl in child.children(&mut dc) {
                if decl.kind() != "variable_declarator" {
                    continue;
                }
                let name = s.field_text(&decl, "name");
                if let Some(value) = decl.child_by_field_name("value") {
                    if matches!(value.kind(), "arrow_function" | "function") {
                        if name.is_empty() {
                            continue;
                        }
                        let fn_id = format!("{}::fn::{}", file_id, name);
                        let line = decl.start_position().row + 1;
                        let vis = if exported {
                            Visibility::Public
                        } else {
                            Visibility::Private
                        };
                        s.add_artifact_with_visibility(
                            fn_id.clone(),
                            name,
                            ArtifactKind::Function,
                            line,
                            vis,
                        );
                        s.contains(file_id, &fn_id, line);
                        if let Some(body) = value.child_by_field_name("body") {
                            walk_for_calls(body, &fn_id, None, s);
                        }
                    }
                }
            }
        }

        "import_statement" => {
            let line = child.start_position().row + 1;
            for name in extract_import_names(child, s) {
                s.record_import(file_id, name, line);
            }
        }

        "export_statement" => {
            let mut ec = child.walk();
            for inner in child.children(&mut ec) {
                if matches!(
                    inner.kind(),
                    "class_declaration"
                        | "function_declaration"
                        | "lexical_declaration"
                        | "variable_declaration"
                ) {
                    scan_js_node(&inner, file_id, true, s);
                }
            }
        }

        _ => {}
    }
}

fn emit_class(node: &Node, file_id: &str, exported: bool, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let class_id = format!("{}::class::{}", file_id, name);
    let line = node.start_position().row + 1;
    let class_vis = if exported {
        Visibility::Public
    } else {
        Visibility::Private
    };
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
        for inner in heritage.children(&mut hc) {
            walk_descendants(inner, |n| {
                if n.kind() == "identifier" {
                    let raw = node_text(&n, s.source);
                    let base = last_identifier(raw);
                    if !base.is_empty() && base != name {
                        s.record_extends(&class_id, base, n.start_position().row + 1);
                    }
                }
            });
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for method in body.children(&mut bc) {
            if method.kind() == "method_definition" {
                let mname = s.field_text(&method, "name");
                if mname.is_empty() {
                    continue;
                }
                let mid = format!("{}::method::{}", class_id, mname);
                let mline = method.start_position().row + 1;
                // JS methods inherit their class's visibility — no per-method
                // keyword in plain JS. Names starting with `#` are private
                // (ES2022 private fields).
                let mvis = if mname.starts_with('#') {
                    Visibility::Private
                } else {
                    class_vis
                };
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

fn emit_function(
    node: &Node,
    file_id: &str,
    parent_id: &str,
    enclosing_type: Option<&str>,
    exported: bool,
    s: &mut Scanner,
) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let fn_id = format!("{}::fn::{}", file_id, name);
    let line = node.start_position().row + 1;
    let vis = if exported {
        Visibility::Public
    } else {
        Visibility::Private
    };
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

        if callee.is_empty() || JS_BUILTINS.contains(&callee.as_str()) {
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
            "identifier" | "import_specifier" | "namespace_import"
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

const JS_BUILTINS: &[&str] = &[
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
