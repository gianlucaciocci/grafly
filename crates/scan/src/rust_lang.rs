use crate::common::{classify_call_target, last_identifier, walk_descendants, Scanner};
use grafly_core::{ArtifactKind, ScanResult};
use std::collections::HashMap;
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .expect("Rust grammar");

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

    // First pass: build name→artifact-ID lookup for top-level user-defined types
    // in this file. Lets `impl Foo` correctly anchor its methods to whichever
    // kind `Foo` actually is (struct / enum / trait). Without this, `impl Enum`
    // hardcoded a phantom `::struct::Enum` anchor and methods became orphans.
    let mut local_types: HashMap<String, String> = HashMap::new();
    {
        let mut c = root.walk();
        for child in root.children(&mut c) {
            let prefix = match child.kind() {
                "struct_item" => "struct",
                "enum_item" => "enum",
                "trait_item" => "trait",
                _ => continue,
            };
            let name = s.field_text(&child, "name");
            if !name.is_empty() {
                local_types.insert(name.clone(), format!("{}::{}::{}", file_id, prefix, name));
            }
        }
    }

    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        match child.kind() {
            "function_item" => emit_function(&child, &file_id, &file_id, None, &mut s),

            "struct_item" => emit_named_top(&child, &file_id, "struct", ArtifactKind::Struct, &mut s),
            "enum_item" => emit_named_top(&child, &file_id, "enum", ArtifactKind::Enum, &mut s),
            "trait_item" => emit_named_top(&child, &file_id, "trait", ArtifactKind::Trait, &mut s),

            "impl_item" => emit_impl(&child, &file_id, &local_types, &mut s),

            "mod_item" => {
                // Skip inline test modules (`#[cfg(test)] mod tests { ... }`). We
                // detect them by name — covers the standard Rust convention without
                // having to walk attribute siblings.
                let name = s.field_text(&child, "name");
                if !is_inline_test_mod(&name) {
                    emit_named_top(&child, &file_id, "mod", ArtifactKind::Namespace, &mut s);
                }
            }

            "use_declaration" => {
                let line = child.start_position().row + 1;
                for name in extract_use_names(&child, &s) {
                    s.record_import(&file_id, name, line);
                }
            }

            _ => {}
        }
    }

    s.result
}

fn emit_named_top(
    node: &Node,
    file_id: &str,
    prefix: &str,
    kind: ArtifactKind,
    s: &mut Scanner,
) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let line = node.start_position().row + 1;
    let id = format!("{}::{}::{}", file_id, prefix, name);
    s.add_artifact(id.clone(), name, kind, line);
    s.contains(file_id, &id, line);
}

fn emit_impl(
    node: &Node,
    file_id: &str,
    local_types: &HashMap<String, String>,
    s: &mut Scanner,
) {
    let type_name = s.field_text(node, "type");
    if type_name.is_empty() {
        return;
    }
    let type_simple = last_identifier(&type_name);

    // Detect `impl Trait for Type`.
    let trait_name = node
        .child_by_field_name("trait")
        .map(|t| s.text(&t).to_string());

    // Anchor methods to the real same-file struct/enum/trait artifact when one
    // exists; otherwise fall back to a `::struct::` anchor (consistent with
    // pre-fix behavior for types defined outside this file).
    let anchor_id = local_types
        .get(&type_simple)
        .cloned()
        .unwrap_or_else(|| format!("{}::struct::{}", file_id, type_simple));

    if let Some(tr) = trait_name.as_deref() {
        let trait_simple = last_identifier(tr);
        if !trait_simple.is_empty() {
            s.record_implements(&anchor_id, trait_simple, node.start_position().row + 1);
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for item in body.children(&mut bc) {
            if item.kind() == "function_item" {
                emit_method(&item, file_id, &anchor_id, &type_simple, s);
            }
        }
    }
}

fn emit_function(
    node: &Node,
    file_id: &str,
    parent_id: &str,
    enclosing_type: Option<&str>,
    s: &mut Scanner,
) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let fn_id = format!("{}::fn::{}", file_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact(fn_id.clone(), name, ArtifactKind::Function, line);
    s.contains(parent_id, &fn_id, line);

    if let Some(body) = node.child_by_field_name("body") {
        walk_for_calls(body, &fn_id, enclosing_type, s);
    }
}

fn emit_method(
    node: &Node,
    _file_id: &str,
    parent_id: &str,
    enclosing_type: &str,
    s: &mut Scanner,
) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let mid = format!("{}::method::{}", parent_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact(mid.clone(), name, ArtifactKind::Method, line);
    s.contains(parent_id, &mid, line);

    if let Some(body) = node.child_by_field_name("body") {
        walk_for_calls(body, &mid, Some(enclosing_type), s);
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
        let (callee, receiver) = classify_call_target(raw, "self", enclosing_type);

        if callee.is_empty() || RUST_BUILTINS.contains(&callee.as_str()) {
            return;
        }
        // Skip Rust macros — they look like calls but aren't function dispatch.
        // tree-sitter-rust distinguishes call_expression from macro_invocation,
        // so we shouldn't hit this in practice but stay defensive.
        s.record_call(caller_id, callee, receiver, node.start_position().row + 1);
    });
}

fn extract_use_names(node: &Node, s: &Scanner) -> Vec<String> {
    let mut names = Vec::new();
    walk_descendants(*node, |n| match n.kind() {
        "identifier" | "type_identifier" | "scoped_identifier" => {
            let raw = node_text(&n, s.source);
            let id = last_identifier(raw);
            if !id.is_empty()
                && id != "self"
                && id != "crate"
                && id != "super"
                && !names.contains(&id)
            {
                names.push(id);
            }
        }
        _ => {}
    });
    names
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// `mod` names that almost always correspond to `#[cfg(test)]` inline test
/// modules. Skipping these at scan time keeps test artifacts out of the
/// dependency map without parsing attribute siblings.
fn is_inline_test_mod(name: &str) -> bool {
    matches!(name, "tests" | "test" | "__tests__" | "test_helpers")
}

/// Rust prelude/macro names that almost never refer to user code. These are
/// filtered before resolution so they don't pollute the unresolved set.
const RUST_BUILTINS: &[&str] = &[
    "println", "print", "eprintln", "eprint", "format", "write", "writeln",
    "vec", "assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq",
    "panic", "unimplemented", "unreachable", "todo", "dbg", "matches",
    // Common methods on Option/Result/Vec/String that bloat the unresolved set.
    // We drop them rather than emit a Call we can't trust.
    "unwrap", "expect", "ok", "err", "is_some", "is_none", "is_ok", "is_err",
    "into", "from", "to_string", "clone", "as_ref", "as_str", "as_mut",
    "len", "is_empty", "iter", "into_iter", "collect", "map", "filter",
    "ok_or", "ok_or_else", "and_then", "or_else", "unwrap_or", "unwrap_or_else",
    "push", "pop", "insert", "remove", "get", "contains_key",
];
