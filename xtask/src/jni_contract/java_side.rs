//! The Java side of the JNI boundary: classes, native methods, fields and type resolution.

use super::*;

pub(super) struct JavaModel {
    /// Fully qualified binary-ish class name (`a.b.Outer.Inner`) to its members.
    pub(super) classes: BTreeMap<String, JavaClass>,
}

pub(super) struct JavaClass {
    pub(super) file: String,
    pub(super) methods: Vec<JavaMethod>,
    fields: Vec<JavaField>,
}

pub(super) struct JavaMethod {
    pub(super) name: String,
    pub(super) is_static: bool,
    pub(super) native: bool,
    pub(super) descriptor: std::result::Result<String, String>,
}

struct JavaField {
    pub(super) name: String,
    pub(super) is_static: bool,
    pub(super) descriptor: std::result::Result<String, String>,
}

impl JavaModel {
    pub(super) fn load(directory: &Path) -> Result<Self> {
        let files = source_files(directory, &["java"])?;
        let mut parsed = Vec::new();
        for file in &files {
            let text = fs::read_to_string(file)?;
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            parsed.push((name, text));
        }
        Ok(Self::parse(&parsed))
    }

    pub(super) fn parse(files: &[(String, String)]) -> Self {
        // Pass 1: declared types, so pass 2 can resolve same-package names.
        let mut declared = BTreeSet::new();
        let mut sources = Vec::new();
        for (file, text) in files {
            let source = RustSource::new(text.clone());
            for (name, _) in java_type_declarations(&source) {
                declared.insert(name);
            }
            sources.push((file.clone(), source));
        }
        let mut classes = BTreeMap::new();
        for (file, source) in &sources {
            let imports = java_imports(source);
            let declarations = java_type_declarations(source);
            let resolver = TypeResolver {
                imports: &imports,
                declared: &declared,
            };
            for (class, body) in &declarations {
                let (methods, fields) =
                    java_members(source, body.clone(), &declarations, &resolver);
                classes.insert(
                    class.clone(),
                    JavaClass {
                        file: file.clone(),
                        methods,
                        fields,
                    },
                );
            }
        }
        Self { classes }
    }

    pub(super) fn member_matches(&self, class: &str, call: &RustCall, descriptor: &str) -> bool {
        let Some(class) = self.classes.get(class) else {
            return false;
        };
        match call.kind {
            CallKind::Method | CallKind::StaticMethod => class.methods.iter().any(|method| {
                method.name == call.name
                    && method.is_static == (call.kind == CallKind::StaticMethod)
                    && method.descriptor.as_deref() == Ok(descriptor)
            }),
            CallKind::Field | CallKind::StaticField => class.fields.iter().any(|field| {
                field.name == call.name
                    && field.is_static == (call.kind == CallKind::StaticField)
                    && field.descriptor.as_deref() == Ok(descriptor)
            }),
        }
    }

    pub(super) fn declared(&self, class: &str, name: &str) -> String {
        let Some(class) = self.classes.get(class) else {
            return "class not found".to_owned();
        };
        let mut found = class
            .methods
            .iter()
            .filter(|method| method.name == name)
            .map(|method| {
                format!(
                    "{}method {}",
                    if method.is_static { "static " } else { "" },
                    describe(&method.descriptor)
                )
            })
            .chain(
                class
                    .fields
                    .iter()
                    .filter(|field| field.name == name)
                    .map(|field| format!("field {}", describe(&field.descriptor))),
            )
            .collect::<Vec<_>>();
        if found.is_empty() {
            found.push("nothing with that name".to_owned());
        }
        found.join("; ")
    }
}

struct TypeResolver<'a> {
    imports: &'a BTreeMap<String, String>,
    pub(super) declared: &'a BTreeSet<String>,
}

impl TypeResolver<'_> {
    /// JNI descriptor of a Java source type, with generics erased.
    pub(super) fn descriptor(&self, ty: &str, scope: &str) -> std::result::Result<String, String> {
        let ty = erase_generics(ty);
        let ty = ty.trim();
        if let Some(element) = ty.strip_suffix("[]") {
            return Ok(format!("[{}", self.descriptor(element, scope)?));
        }
        if let Some(element) = ty.strip_suffix("...") {
            return Ok(format!("[{}", self.descriptor(element, scope)?));
        }
        let primitive = match ty {
            "void" => Some("V"),
            "boolean" => Some("Z"),
            "byte" => Some("B"),
            "char" => Some("C"),
            "short" => Some("S"),
            "int" => Some("I"),
            "long" => Some("J"),
            "float" => Some("F"),
            "double" => Some("D"),
            _ => None,
        };
        if let Some(primitive) = primitive {
            return Ok(primitive.to_owned());
        }
        Ok(format!("L{};", self.binary_name(ty, scope)?))
    }

    /// `java/lang/String`, `de/duecki/calibraw/Outer$Inner`, ...
    fn binary_name(&self, ty: &str, scope: &str) -> std::result::Result<String, String> {
        let (head, rest) = ty
            .split_once('.')
            .map_or((ty, None), |(head, rest)| (head, Some(rest)));
        // Nested types visible from the enclosing class scope, innermost first.
        let mut enclosing = Some(scope);
        while let Some(current) = enclosing {
            let candidate = format!("{current}.{head}");
            if self.declared.contains(&candidate) {
                return Ok(self.with_rest(&candidate, rest));
            }
            enclosing = current
                .rsplit_once('.')
                .map(|(outer, _)| outer)
                .filter(|outer| outer.len() > PACKAGE.len());
        }
        if let Some(qualified) = self.imports.get(head) {
            return Ok(self.with_rest(qualified, rest));
        }
        let same_package = format!("{PACKAGE}.{head}");
        if self.declared.contains(&same_package) {
            return Ok(self.with_rest(&same_package, rest));
        }
        const JAVA_LANG: [&str; 14] = [
            "Object",
            "String",
            "Class",
            "Throwable",
            "Exception",
            "RuntimeException",
            "Integer",
            "Long",
            "Boolean",
            "Float",
            "Double",
            "Runnable",
            "Thread",
            "Void",
        ];
        if rest.is_none() && JAVA_LANG.contains(&head) {
            return Ok(format!("java/lang/{head}"));
        }
        if ty.contains('.') && ty.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            return Ok(ty.replace('.', "/"));
        }
        Err(format!("cannot resolve Java type {ty}"))
    }

    fn with_rest(&self, qualified: &str, rest: Option<&str>) -> String {
        let mut name = qualified.to_owned();
        if let Some(rest) = rest {
            name.push('.');
            name.push_str(rest);
        }
        binary_class_name(&name, self.declared)
    }
}

/// Converts `a.b.Outer.Inner` to `a/b/Outer$Inner` using the declared types.
fn binary_class_name(qualified: &str, declared: &BTreeSet<String>) -> String {
    let segments = qualified.split('.').collect::<Vec<_>>();
    // The first declared prefix is the top-level class; the rest are nested.
    for split in 1..=segments.len() {
        let prefix = segments[..split].join(".");
        if declared.contains(&prefix) {
            let mut name = segments[..split].join("/");
            for nested in &segments[split..] {
                name.push('$');
                name.push_str(nested);
            }
            return name;
        }
    }
    segments.join("/")
}

fn erase_generics(ty: &str) -> String {
    let mut erased = String::new();
    let mut depth = 0usize;
    for character in ty.chars() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => erased.push(character),
            _ => {}
        }
    }
    erased.split_whitespace().collect::<Vec<_>>().join("")
}

fn java_imports(source: &RustSource) -> BTreeMap<String, String> {
    let mut imports = BTreeMap::new();
    for line in source.structural().lines() {
        let Some(import) = line.trim().strip_prefix("import ") else {
            continue;
        };
        if import.trim_start().starts_with("static ") {
            continue;
        }
        let path = import.trim().trim_end_matches(';').trim();
        if let Some((_, simple)) = path.rsplit_once('.') {
            if simple != "*" {
                imports.insert(simple.to_owned(), path.to_owned());
            }
        }
    }
    imports
}

/// `(qualified name, body range)` of every class, interface and enum.
fn java_type_declarations(source: &RustSource) -> Vec<(String, std::ops::Range<usize>)> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let mut declarations: Vec<(String, std::ops::Range<usize>)> = Vec::new();
    let mut starts = Vec::new();
    for keyword in ["class", "interface", "enum"] {
        for (start, _) in structural.match_indices(keyword) {
            if is_word_at(bytes, start, keyword.len()) {
                starts.push((start, keyword.len()));
            }
        }
    }
    starts.sort_unstable();
    for (start, length) in starts {
        // Skip `.class` literals and keywords that do not start a declaration.
        if structural[..start].trim_end().ends_with('.') {
            continue;
        }
        let name_start = start + length;
        let name_start = name_start
            + structural[name_start..]
                .len()
                .saturating_sub(structural[name_start..].trim_start().len());
        let name_end = identifier_end(bytes, name_start);
        if name_end == name_start {
            continue;
        }
        let Some(open) = structural[name_end..]
            .find('{')
            .map(|offset| name_end + offset)
        else {
            continue;
        };
        let Some(close) = source.matching_close(open) else {
            continue;
        };
        let simple = &structural[name_start..name_end];
        let outer = declarations
            .iter()
            .filter(|(_, body)| body.start < start && start < body.end)
            .max_by_key(|(_, body)| body.start)
            .map(|(name, _)| name.clone());
        let name = match outer {
            Some(outer) => format!("{outer}.{simple}"),
            None => format!("{PACKAGE}.{simple}"),
        };
        declarations.push((name, open + 1..close));
    }
    declarations
}

/// Methods and fields declared directly in `body` (not in nested types).
fn java_members(
    source: &RustSource,
    body: std::ops::Range<usize>,
    declarations: &[(String, std::ops::Range<usize>)],
    resolver: &TypeResolver<'_>,
) -> (Vec<JavaMethod>, Vec<JavaField>) {
    let structural = source.structural();
    let scope = declarations
        .iter()
        .find(|(_, range)| *range == body)
        .map(|(name, _)| name.clone())
        .unwrap_or_default();
    // Split the body into member declarations at depth 0: each ends at `;`
    // or at the close of its `{}` block.
    let mut members = Vec::new();
    let mut start = body.start;
    let mut index = body.start;
    let bytes = structural.as_bytes();
    while index < body.end {
        match bytes[index] {
            b';' => {
                members.push(start..index);
                start = index + 1;
            }
            b'{' => {
                let close = source.matching_close(index).unwrap_or(body.end);
                members.push(start..index);
                start = close + 1;
                index = close;
            }
            _ => {}
        }
        index += 1;
    }

    let mut methods = Vec::new();
    let mut fields = Vec::new();
    for member in members {
        let text = strip_annotations(&structural[member.clone()]);
        let text = text.trim();
        if text.is_empty()
            || text.contains(" class ")
            || text.starts_with("class ")
            || text.contains("interface ")
            || text.contains("enum ")
            || text.starts_with("static") && !text.contains('(') && !text.contains(' ')
        {
            continue;
        }
        if let Some(open) = text.find('(') {
            // Method or constructor: `modifiers Type name(params) [throws ...]`.
            let head = text[..open].trim();
            let Some(close) = matching_paren(text, open) else {
                continue;
            };
            let tokens = head.split_whitespace().collect::<Vec<_>>();
            let Some((name, rest)) = tokens.split_last() else {
                continue;
            };
            let modifiers = rest
                .iter()
                .take_while(|token| is_java_modifier(token))
                .copied()
                .collect::<Vec<_>>();
            let return_type = rest[modifiers.len()..].join(" ");
            if return_type.is_empty() || text[..open].contains('=') {
                continue; // constructor or initializer expression
            }
            let parameters = split_top_level(&text[open + 1..close], ',');
            let descriptor = (|| {
                let mut descriptor = String::from("(");
                for parameter in &parameters {
                    let parameter = strip_annotations(parameter);
                    let tokens = parameter
                        .split_whitespace()
                        .filter(|token| *token != "final")
                        .collect::<Vec<_>>();
                    let Some((_, ty)) = tokens.split_last() else {
                        return Err(format!("malformed parameter {parameter}"));
                    };
                    descriptor.push_str(&resolver.descriptor(&ty.join(" "), &scope)?);
                }
                descriptor.push(')');
                descriptor.push_str(&resolver.descriptor(&return_type, &scope)?);
                Ok(descriptor)
            })();
            methods.push(JavaMethod {
                name: (*name).to_owned(),
                is_static: modifiers.contains(&"static"),
                native: modifiers.contains(&"native"),
                descriptor,
            });
        } else {
            // Field: `modifiers Type name [= value]`.
            let declaration = text.split('=').next().unwrap_or(text).trim();
            let tokens = declaration.split_whitespace().collect::<Vec<_>>();
            let Some((name, rest)) = tokens.split_last() else {
                continue;
            };
            let modifiers = rest
                .iter()
                .take_while(|token| is_java_modifier(token))
                .copied()
                .collect::<Vec<_>>();
            let ty = rest[modifiers.len()..].join(" ");
            if ty.is_empty() {
                continue;
            }
            fields.push(JavaField {
                name: (*name).to_owned(),
                is_static: modifiers.contains(&"static"),
                descriptor: resolver.descriptor(&ty, &scope),
            });
        }
    }
    (methods, fields)
}

fn strip_annotations(text: &str) -> String {
    let mut output = String::new();
    let mut characters = text.char_indices().peekable();
    while let Some((_, character)) = characters.next() {
        if character == '@' {
            // Skip the annotation name and an optional argument list.
            while characters
                .peek()
                .is_some_and(|(_, next)| next.is_alphanumeric() || *next == '.' || *next == '_')
            {
                characters.next();
            }
            if characters.peek().is_some_and(|(_, next)| *next == '(') {
                let mut depth = 0;
                for (_, next) in characters.by_ref() {
                    match next {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            output.push(' ');
        } else {
            output.push(character);
        }
    }
    output
}

fn is_java_modifier(token: &str) -> bool {
    matches!(
        token,
        "public"
            | "private"
            | "protected"
            | "static"
            | "final"
            | "abstract"
            | "native"
            | "synchronized"
            | "transient"
            | "volatile"
            | "strictfp"
            | "default"
    )
}
