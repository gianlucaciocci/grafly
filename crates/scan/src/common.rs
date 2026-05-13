//! Shared helper for per-language scanners.

use grafly_core::{
    ArtifactKind, Confidence, DependencyKind, RawArtifact, RawDependency, ScanResult,
    UnresolvedReference,
};
use tree_sitter::Node;

/// Carries state through a single file's scan.
pub struct Scanner<'src> {
    pub source: &'src [u8],
    pub file_id: String,
    pub result: ScanResult,
}

impl<'src> Scanner<'src> {
    pub fn new(file_id: impl Into<String>, source: &'src str) -> Self {
        Self {
            source: source.as_bytes(),
            file_id: file_id.into(),
            result: ScanResult::default(),
        }
    }

    pub fn text<'a>(&self, node: &Node<'a>) -> &str {
        node.utf8_text(self.source).unwrap_or("")
    }

    pub fn field_text(&self, node: &Node, field: &str) -> String {
        node.child_by_field_name(field)
            .map(|n| self.text(&n).to_string())
            .unwrap_or_default()
    }

    pub fn add_file(&mut self, label: impl Into<String>) {
        self.result.artifacts.push(RawArtifact {
            id: self.file_id.clone(),
            label: label.into(),
            kind: ArtifactKind::File,
            source_file: self.file_id.clone(),
            source_line: 0,
        });
    }

    pub fn add_artifact(&mut self, id: String, label: String, kind: ArtifactKind, line: usize) {
        self.result.artifacts.push(RawArtifact {
            id,
            label,
            kind,
            source_file: self.file_id.clone(),
            source_line: line,
        });
    }

    pub fn add_dependency(
        &mut self,
        src: impl Into<String>,
        dst: impl Into<String>,
        kind: DependencyKind,
        conf: Confidence,
        line: usize,
    ) {
        self.result.dependencies.push(RawDependency {
            source_id: src.into(),
            target_id: dst.into(),
            kind,
            confidence: conf,
            source_line: line,
        });
    }

    pub fn contains(
        &mut self,
        parent: impl Into<String>,
        child: impl Into<String>,
        line: usize,
    ) {
        self.add_dependency(parent, child, DependencyKind::Contains, Confidence::Extracted, line);
    }

    // ── Unresolved references (resolved at build time) ───────────────────────

    /// Record a reference where the target is given by simple name.
    /// `receiver` is the enclosing type for receiver-typed method calls
    /// (e.g. `Some("Foo")` for `self.bar()` inside `impl Foo`).
    pub fn record_unresolved(
        &mut self,
        source_id: impl Into<String>,
        target_name: impl Into<String>,
        receiver: Option<String>,
        kind: DependencyKind,
        line: usize,
    ) {
        let target_name = target_name.into();
        if target_name.is_empty() {
            return;
        }
        self.result.unresolved.push(UnresolvedReference {
            source_id: source_id.into(),
            target_name,
            receiver,
            kind,
            source_line: line,
        });
    }

    /// Record a function/method call.
    /// `receiver` should be `Some("Foo")` for `self.bar()` inside `impl Foo`
    /// or for `Foo::bar()` qualified calls. Bare calls pass `None`.
    pub fn record_call(
        &mut self,
        caller: impl Into<String>,
        callee_name: impl Into<String>,
        receiver: Option<String>,
        line: usize,
    ) {
        self.record_unresolved(caller, callee_name, receiver, DependencyKind::Calls, line);
    }

    pub fn record_extends(
        &mut self,
        child: impl Into<String>,
        parent_name: impl Into<String>,
        line: usize,
    ) {
        self.record_unresolved(child, parent_name, None, DependencyKind::Extends, line);
    }

    pub fn record_implements(
        &mut self,
        impl_id: impl Into<String>,
        trait_name: impl Into<String>,
        line: usize,
    ) {
        self.record_unresolved(impl_id, trait_name, None, DependencyKind::Implements, line);
    }

    /// Record a named import. Replaces creating an `Import`-kind artifact node:
    /// instead of storing the raw `use crate::msgbus::get_message_bus` text as
    /// a graph node (noise), we record an `Imports` reference to each name
    /// inside the import statement, to be resolved against the project's
    /// artifacts at build time.
    pub fn record_import(
        &mut self,
        source_id: impl Into<String>,
        target_name: impl Into<String>,
        line: usize,
    ) {
        self.record_unresolved(source_id, target_name, None, DependencyKind::Imports, line);
    }

    /// Record an in-body reference (non-call use of a symbol).
    /// Currently unused by scanners; reserved for future symbol-use tracking.
    pub fn record_reference(
        &mut self,
        source_id: impl Into<String>,
        target_name: impl Into<String>,
        line: usize,
    ) {
        self.record_unresolved(source_id, target_name, None, DependencyKind::References, line);
    }
}

/// Classify a call expression and return (callee_name, receiver_type).
/// `receiver_type` is set when the call is on `self`/`this`/`Self` (uses
/// `enclosing_type`) or qualified via `Type::method` (uses the path).
///
/// Returns `(callee, None)` for bare function calls or when we can't determine
/// the receiver — these will go through name-only resolution which requires
/// uniqueness for Calls (no Ambiguous edges).
pub fn classify_call_target(
    func_text: &str,
    receiver_keyword: &str,
    enclosing_type: Option<&str>,
) -> (String, Option<String>) {
    // Strip generics first: `Foo::<T>::bar` → `Foo::bar`
    let stripped: String = strip_generics(func_text);

    // `Type::Self::method` style or simple `name`
    if !stripped.contains("::") && !stripped.contains('.') {
        return (stripped, None);
    }

    if let Some(idx) = stripped.rfind("::") {
        let path = &stripped[..idx];
        let name = stripped[idx + 2..].to_string();
        let last_seg = path.rsplit("::").next().unwrap_or("");

        if last_seg == "Self" || last_seg == "self" {
            return (name, enclosing_type.map(String::from));
        }
        // Receiver is the last path segment if it looks like a type (starts upper)
        if last_seg
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            return (name, Some(last_seg.to_string()));
        }
        // Lower-case path segment → likely a module path. No receiver.
        return (name, None);
    }

    if let Some(idx) = stripped.rfind('.') {
        let receiver_expr = &stripped[..idx];
        let name = stripped[idx + 1..].to_string();

        if receiver_expr == receiver_keyword {
            return (name, enclosing_type.map(String::from));
        }
        // Capitalised plain identifier → likely a type used statically
        if !receiver_expr.contains('.') && !receiver_expr.contains('(') {
            if receiver_expr
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            {
                return (name, Some(receiver_expr.to_string()));
            }
        }
        return (name, None);
    }

    (stripped, None)
}

fn strip_generics(text: &str) -> String {
    // Strip turbofish like `::<T>` and angle brackets `<T>` from `Foo<T>::bar`
    let mut out = String::with_capacity(text.len());
    let mut depth: i32 = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => depth += 1,
            '>' => {
                if depth > 0 {
                    depth -= 1
                }
            }
            ':' if depth == 0 => {
                if matches!(chars.peek(), Some(':')) {
                    out.push(':');
                    out.push(chars.next().unwrap());
                    // skip turbofish `::<T>`
                    if matches!(chars.peek(), Some('<')) {
                        chars.next();
                        let mut d: i32 = 1;
                        while let Some(nx) = chars.next() {
                            if nx == '<' {
                                d += 1;
                            } else if nx == '>' {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                        }
                    }
                } else {
                    out.push(c);
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim()
        .trim_end_matches('(')
        .trim_end_matches(')')
        .trim()
        .to_string()
}

/// Take the last identifier segment from a dotted/path expression.
/// `"foo"` → `"foo"`, `"obj.method"` → `"method"`, `"crate::mod::fn"` → `"fn"`.
pub fn last_identifier(text: &str) -> String {
    let mut result = text;
    if let Some(idx) = result.rfind("::") {
        result = &result[idx + 2..];
    }
    if let Some(idx) = result.rfind('.') {
        result = &result[idx + 1..];
    }
    // strip generic params like `Foo<T>` → `Foo`
    if let Some(idx) = result.find('<') {
        result = &result[..idx];
    }
    // strip parens/whitespace
    result.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_string()
}

/// Iteratively walk every descendant of `root` (excluding `root` itself)
/// using a tree-cursor. Calls `f` on each descendant.
pub fn walk_descendants<F: FnMut(Node)>(root: Node, mut f: F) {
    let mut cursor = root.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        f(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            // bail when we've climbed back to root
            if cursor.node().id() == root.id() {
                return;
            }
        }
    }
}
