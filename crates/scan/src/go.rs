use crate::common::{classify_call_target, walk_descendants, Scanner};
use grafly_core::{ArtifactKind, ScanResult};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .expect("Go grammar");

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

    // Detect `package main` so manifest discovery can flag the owning go.mod
    // as a binary. Tracked via parent directory of this file.
    if is_main_package(&root, source.as_bytes()) {
        if let Some(dir) = path.parent() {
            s.result
                .main_package_dirs
                .push(dir.to_string_lossy().replace('\\', "/"));
        }
    }

    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                let name = s.field_text(&child, "name");
                if name.is_empty() {
                    continue;
                }
                let fn_id = format!("{}::fn::{}", file_id, name);
                let line = child.start_position().row + 1;
                s.add_artifact(fn_id.clone(), name, ArtifactKind::Function, line);
                s.contains(&file_id, &fn_id, line);
                if let Some(body) = child.child_by_field_name("body") {
                    walk_for_calls(body, &fn_id, None, &mut s);
                }
            }

            "method_declaration" => {
                let name = s.field_text(&child, "name");
                let receiver_type = extract_receiver_type(&child, &s);

                if name.is_empty() {
                    continue;
                }
                let mid = if receiver_type.is_empty() {
                    format!("{}::method::{}", file_id, name)
                } else {
                    format!("{}::struct::{}::method::{}", file_id, receiver_type, name)
                };
                let line = child.start_position().row + 1;
                s.add_artifact(mid.clone(), name, ArtifactKind::Method, line);

                if receiver_type.is_empty() {
                    s.contains(&file_id, &mid, line);
                } else {
                    let struct_id = format!("{}::struct::{}", file_id, receiver_type);
                    s.contains(&struct_id, &mid, line);
                }

                if let Some(body) = child.child_by_field_name("body") {
                    let enclosing = if receiver_type.is_empty() {
                        None
                    } else {
                        Some(receiver_type.as_str())
                    };
                    walk_for_calls(body, &mid, enclosing, &mut s);
                }
            }

            "type_declaration" => {
                let mut tc = child.walk();
                for spec in child.children(&mut tc) {
                    if spec.kind() != "type_spec" {
                        continue;
                    }
                    let name = s.field_text(&spec, "name");
                    if name.is_empty() {
                        continue;
                    }
                    let kind = spec
                        .child_by_field_name("type")
                        .map(|t| match t.kind() {
                            "struct_type" => ArtifactKind::Struct,
                            "interface_type" => ArtifactKind::Interface,
                            _ => ArtifactKind::Struct,
                        })
                        .unwrap_or(ArtifactKind::Struct);

                    let prefix = if kind == ArtifactKind::Interface { "interface" } else { "struct" };
                    let line = spec.start_position().row + 1;
                    let id = format!("{}::{}::{}", file_id, prefix, name);
                    s.add_artifact(id.clone(), name, kind, line);
                    s.contains(&file_id, &id, line);
                }
            }

            "import_declaration" => {
                let line = child.start_position().row + 1;
                for name in extract_import_names(&child, &s) {
                    s.record_import(&file_id, name, line);
                }
            }

            _ => {}
        }
    }

    s.result
}

fn extract_receiver_type(child: &Node, s: &Scanner) -> String {
    let mut found = String::new();
    if let Some(r) = child.child_by_field_name("receiver") {
        let mut rc = r.walk();
        for n in r.children(&mut rc) {
            if n.kind() == "parameter_declaration" {
                if let Some(t) = n.child_by_field_name("type") {
                    found = s.text(&t).trim_start_matches('*').to_string();
                    break;
                }
            }
        }
    }
    found
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
        let (callee, receiver) = classify_call_target(raw, "", enclosing_type);

        if callee.is_empty() || GO_BUILTINS.contains(&callee.as_str()) {
            return;
        }
        s.record_call(caller_id, callee, receiver, node.start_position().row + 1);
    });
}

fn extract_import_names(node: &Node, s: &Scanner) -> Vec<String> {
    let mut names = Vec::new();
    walk_descendants(*node, |n| {
        if n.kind() == "interpreted_string_literal" {
            let raw = node_text(&n, s.source);
            let cleaned = raw.trim_matches('"');
            if let Some(last) = cleaned.rsplit('/').next() {
                if !last.is_empty() && !names.contains(&last.to_string()) {
                    names.push(last.to_string());
                }
            }
        }
    });
    names
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// True when the file's top-level `package` clause names `main`.
/// (Tree-sitter-go shape: source_file → package_clause → package_identifier.)
fn is_main_package(root: &Node, source: &[u8]) -> bool {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "package_clause" {
            let mut c = child.walk();
            for inner in child.children(&mut c) {
                if inner.kind() == "package_identifier" {
                    return inner.utf8_text(source).unwrap_or("") == "main";
                }
            }
        }
    }
    false
}

const GO_BUILTINS: &[&str] = &[
    "make", "new", "len", "cap", "append", "copy", "delete", "panic",
    "recover", "print", "println", "close", "complex", "real", "imag",
];
