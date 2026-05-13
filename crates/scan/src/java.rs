use crate::common::{classify_call_target, last_identifier, walk_descendants, Scanner};
use grafly_core::{ArtifactKind, ScanResult};
use std::path::Path;
use tree_sitter::{Node, Parser};

pub fn scan(path: &Path, source: &str) -> ScanResult {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .expect("Java grammar");

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
        match child.kind() {
            "class_declaration" => emit_class(&child, &file_id, &mut s),
            "interface_declaration" => emit_interface(&child, &file_id, &mut s),
            "enum_declaration" => {
                let name = s.field_text(&child, "name");
                if !name.is_empty() {
                    let line = child.start_position().row + 1;
                    let id = format!("{}::enum::{}", file_id, name);
                    s.add_artifact(id.clone(), name, ArtifactKind::Enum, line);
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

fn emit_class(node: &Node, file_id: &str, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let class_id = format!("{}::class::{}", file_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact(class_id.clone(), name.clone(), ArtifactKind::Class, line);
    s.contains(file_id, &class_id, line);

    if let Some(sup) = node.child_by_field_name("superclass") {
        walk_descendants(sup, |n| {
            if matches!(n.kind(), "type_identifier" | "scoped_type_identifier") {
                let raw = node_text(&n, s.source);
                let id = last_identifier(raw);
                if !id.is_empty() && id != name {
                    s.record_extends(&class_id, id, n.start_position().row + 1);
                }
            }
        });
    }
    if let Some(ifaces) = node.child_by_field_name("interfaces") {
        walk_descendants(ifaces, |n| {
            if matches!(n.kind(), "type_identifier" | "scoped_type_identifier") {
                let raw = node_text(&n, s.source);
                let id = last_identifier(raw);
                if !id.is_empty() && id != name {
                    s.record_implements(&class_id, id, n.start_position().row + 1);
                }
            }
        });
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            match member.kind() {
                "method_declaration" | "constructor_declaration" => {
                    emit_method(&member, &class_id, &name, s);
                }
                _ => {}
            }
        }
    }
}

fn emit_interface(node: &Node, file_id: &str, s: &mut Scanner) {
    let name = s.field_text(node, "name");
    if name.is_empty() {
        return;
    }
    let id = format!("{}::interface::{}", file_id, name);
    let line = node.start_position().row + 1;
    s.add_artifact(id.clone(), name.clone(), ArtifactKind::Interface, line);
    s.contains(file_id, &id, line);

    let mut nc = node.walk();
    for ch in node.children(&mut nc) {
        if ch.kind() == "extends_interfaces" {
            walk_descendants(ch, |n| {
                if matches!(n.kind(), "type_identifier" | "scoped_type_identifier") {
                    let raw = node_text(&n, s.source);
                    let parent = last_identifier(raw);
                    if !parent.is_empty() && parent != name {
                        s.record_extends(&id, parent, n.start_position().row + 1);
                    }
                }
            });
        }
    }

    if let Some(body) = node.child_by_field_name("body") {
        let mut bc = body.walk();
        for member in body.children(&mut bc) {
            if member.kind() == "method_declaration" {
                emit_method(&member, &id, &name, s);
            }
        }
    }
}

fn emit_method(node: &Node, parent_id: &str, enclosing_type: &str, s: &mut Scanner) {
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
        if node.kind() != "method_invocation" {
            return;
        }
        // Java method_invocation may have:
        //   object . name ( args )  → field receiver form
        //         name ( args )     → bare
        // We extract the full target text and let classify_call_target handle it.
        let name_node = node.child_by_field_name("name");
        let object_node = node.child_by_field_name("object");
        let (callee, receiver) = match (object_node, name_node) {
            (Some(o), Some(n)) => {
                let obj_text = node_text(&o, s.source);
                let combined = format!("{}.{}", obj_text, node_text(&n, s.source));
                classify_call_target(&combined, "this", enclosing_type)
            }
            (None, Some(n)) => (node_text(&n, s.source).to_string(), None),
            _ => return,
        };

        if callee.is_empty() || JAVA_BUILTINS.contains(&callee.as_str()) {
            return;
        }
        s.record_call(caller_id, callee, receiver, node.start_position().row + 1);
    });
}

fn extract_import_names(node: &Node, s: &Scanner) -> Vec<String> {
    let mut names = Vec::new();
    walk_descendants(*node, |n| {
        if matches!(n.kind(), "identifier" | "scoped_identifier") {
            let raw = node_text(&n, s.source);
            let id = last_identifier(raw);
            if !id.is_empty() && id != "*" && !names.contains(&id) {
                names.push(id);
            }
        }
    });
    names
}

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

const JAVA_BUILTINS: &[&str] = &[
    "println", "print", "printf", "format",
    "toString", "equals", "hashCode", "getClass", "wait", "notify", "notifyAll",
    "length", "size", "isEmpty", "contains", "add", "remove", "get", "put",
    "valueOf", "parseInt", "parseDouble", "parseLong",
];
