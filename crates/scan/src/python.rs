use crate::common::{
    classify_call_target, last_identifier, visibility_from_python_name, walk_descendants, Scanner,
};
use grafly_core::{ArtifactKind, ScanResult};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .expect("Python grammar");

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
        handle_top_level(&child, &file_id, &mut s);
    }

    s.result
}

fn handle_top_level(child: &Node, file_id: &str, s: &mut Scanner) {
    match child.kind() {
        "class_definition" => emit_class(child, file_id, s),

        "function_definition" | "async_function_definition" => {
            emit_function(child, file_id, None, s);
        }

        "decorated_definition" => {
            let mut dc = child.walk();
            for inner in child.children(&mut dc) {
                match inner.kind() {
                    "class_definition" => emit_class(&inner, file_id, s),
                    "function_definition" | "async_function_definition" => {
                        emit_function(&inner, file_id, None, s)
                    }
                    _ => {}
                }
            }
        }

        "import_statement" | "import_from_statement" => {
            let line = child.start_position().row + 1;
            for name in extract_import_names(child, s) {
                s.record_import(file_id, name, line);
            }
        }

        _ => {}
    }
}

fn emit_class(node: &Node, file_id: &str, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let class_id = format!("{}::class::{}", file_id, name);
    let line = node.start_position().row + 1;
    let vis = visibility_from_python_name(&name);
    s.add_artifact_with_visibility(
        class_id.clone(),
        name.clone(),
        ArtifactKind::Class,
        line,
        vis,
    );
    s.contains(file_id, &class_id, line);

    // class Foo(Base, Other): → Extends edges
    if let Some(args) = node.child_by_field_name("superclasses") {
        let mut ac = args.walk();
        for arg in args.children(&mut ac) {
            if matches!(arg.kind(), "identifier" | "attribute" | "subscript" | "call") {
                let raw = s.text(&arg).to_string();
                let base_name = last_identifier(&raw);
                if !base_name.is_empty() && base_name != name {
                    s.record_extends(&class_id, base_name, arg.start_position().row + 1);
                }
            }
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            match member.kind() {
                "function_definition" | "async_function_definition" => {
                    emit_method(&member, file_id, &class_id, &name, s);
                }
                "decorated_definition" => {
                    let mut mc = member.walk();
                    for inner in member.children(&mut mc) {
                        if matches!(
                            inner.kind(),
                            "function_definition" | "async_function_definition"
                        ) {
                            emit_method(&inner, file_id, &class_id, &name, s);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn emit_function(node: &Node, file_id: &str, _parent_class: Option<&str>, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let fn_id = format!("{}::fn::{}", file_id, name);
    let line = node.start_position().row + 1;
    let vis = visibility_from_python_name(&name);
    s.add_artifact_with_visibility(fn_id.clone(), name, ArtifactKind::Function, line, vis);
    s.contains(file_id, &fn_id, line);

    if let Some(body) = node.child_by_field_name("body") {
        walk_for_calls(body, &fn_id, None, s);
    }
}

fn emit_method(node: &Node, _file_id: &str, class_id: &str, class_name: &str, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let mid = format!("{}::method::{}", class_id, name);
    let line = node.start_position().row + 1;
    let vis = visibility_from_python_name(&name);
    s.add_artifact_with_visibility(mid.clone(), name, ArtifactKind::Method, line, vis);
    s.contains(class_id, &mid, line);

    if let Some(body) = node.child_by_field_name("body") {
        walk_for_calls(body, &mid, Some(class_name), s);
    }
}

fn walk_for_calls(body: Node, caller_id: &str, enclosing_type: Option<&str>, s: &mut Scanner) {
    walk_descendants(body, |node| {
        if node.kind() != "call" {
            return;
        }
        let Some(func) = node.child_by_field_name("function") else {
            return;
        };
        let raw = node_text(&func, s.source);
        let (callee, receiver) = classify_call_target(raw, "self", enclosing_type);

        if callee.is_empty() || PYTHON_BUILTINS.contains(&callee.as_str()) {
            return;
        }
        s.record_call(caller_id, callee, receiver, node.start_position().row + 1);
    });
}

fn extract_import_names(node: &Node, s: &Scanner) -> Vec<String> {
    let mut names = Vec::new();
    walk_descendants(*node, |n| {
        if matches!(n.kind(), "identifier" | "dotted_name") {
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

const PYTHON_BUILTINS: &[&str] = &[
    "print", "len", "range", "list", "dict", "set", "tuple", "str", "int", "float",
    "bool", "type", "isinstance", "hasattr", "getattr", "setattr", "open", "iter",
    "next", "map", "filter", "zip", "sorted", "reversed", "enumerate", "any", "all",
    "min", "max", "sum", "abs", "round", "repr", "id", "hash", "vars", "dir",
    "super", "callable", "format", "input",
    // Common method names that won't resolve usefully without proper type inference
    "append", "extend", "pop", "remove", "insert", "get", "keys", "values", "items",
    "join", "split", "strip", "lower", "upper", "replace", "startswith", "endswith",
];
