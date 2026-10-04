//! The Rust side of the JNI boundary: exported functions and calls into Java.

use super::*;

pub(super) struct RustExport {
    pub(super) class: String,
    pub(super) method: String,
    pub(super) is_static: bool,
    pub(super) descriptor: std::result::Result<String, String>,
}

/// `Java_<package>_<Class>_<method>` exports and their JNI descriptors.
pub(super) fn rust_exports(source: &RustSource) -> Vec<RustExport> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let prefix = format!("Java_{}_", PACKAGE.replace('.', "_"));
    let mut exports = Vec::new();
    for (start, _) in structural.match_indices(&prefix) {
        if !structural[..start].trim_end().ends_with("fn") {
            continue;
        }
        let name_end = identifier_end(bytes, start);
        let Some((class, method)) = decode_export_name(&structural[start + prefix.len()..name_end])
        else {
            continue;
        };
        let Some(params_open) = structural[name_end..]
            .find('(')
            .map(|offset| name_end + offset)
        else {
            continue;
        };
        let params_close = source.matching_close(params_open).unwrap_or(bytes.len());
        let params = split_top_level(&structural[params_open + 1..params_close], ',');
        let after = &structural[params_close + 1..];
        let return_type = after
            .split('{')
            .next()
            .and_then(|head| head.split("->").nth(1))
            .map(str::trim);
        let types = params
            .iter()
            .filter_map(|param| param.split_once(':').map(|(_, ty)| ty.trim()))
            .collect::<Vec<_>>();
        let is_static = types.get(1).is_some_and(|ty| base_type(ty) == "JClass");
        let descriptor = (|| {
            if types.len() < 2 {
                return Err("exports take a JNIEnv and a jclass or jobject".to_owned());
            }
            let mut descriptor = String::from("(");
            for ty in &types[2..] {
                descriptor.push_str(&rust_value_descriptor(ty)?);
            }
            descriptor.push(')');
            descriptor.push_str(&match return_type {
                Some(ty) => rust_value_descriptor(ty)?,
                None => "V".to_owned(),
            });
            Ok(descriptor)
        })();
        exports.push(RustExport {
            class: format!("{PACKAGE}.{class}"),
            method,
            is_static,
            descriptor,
        });
    }
    exports
}

/// Decodes `Class_method` from a JNI short name (`_1` is an escaped `_`).
pub(super) fn decode_export_name(mangled: &str) -> Option<(String, String)> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut characters = mangled.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '_' {
            match characters.peek() {
                Some('1') => {
                    characters.next();
                    current.push('_');
                }
                Some('2') | Some('3') | Some('0') | Some('_') => return None,
                _ => parts.push(std::mem::take(&mut current)),
            }
        } else {
            current.push(character);
        }
    }
    parts.push(current);
    let [class, method] = parts.as_slice() else {
        return None;
    };
    Some((class.clone(), method.clone()))
}

/// The last path segment of a Rust type without generic arguments.
fn base_type(ty: &str) -> &str {
    let ty = ty.trim().trim_start_matches('&').trim();
    let ty = ty.split('<').next().unwrap_or(ty).trim();
    ty.rsplit("::").next().unwrap_or(ty)
}

/// JNI descriptor of an export parameter or return type.
fn rust_value_descriptor(ty: &str) -> std::result::Result<String, String> {
    let descriptor = match base_type(ty) {
        "()" => "V",
        "jboolean" => "Z",
        "jbyte" => "B",
        "jchar" => "C",
        "jshort" => "S",
        "jint" => "I",
        "jlong" => "J",
        "jfloat" => "F",
        "jdouble" => "D",
        "JString" | "jstring" => "Ljava/lang/String;",
        "JObject" | "jobject" => "Ljava/lang/Object;",
        "JClass" | "jclass" => "Ljava/lang/Class;",
        "JThrowable" => "Ljava/lang/Throwable;",
        "JByteArray" | "jbyteArray" => "[B",
        "JIntArray" | "jintArray" => "[I",
        "JLongArray" | "jlongArray" => "[J",
        "JFloatArray" | "jfloatArray" => "[F",
        "JObjectArray" | "jobjectArray" => "[Ljava/lang/Object;",
        other => return Err(format!("unsupported JNI parameter type {other}")),
    };
    Ok(descriptor.to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallKind {
    Method,
    StaticMethod,
    Field,
    StaticField,
}

impl CallKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Method => "call_method",
            Self::StaticMethod => "call_static_method",
            Self::Field => "get_field",
            Self::StaticField => "get_static_field",
        }
    }

    pub(super) const fn member(self) -> &'static str {
        match self {
            Self::Method => "instance method",
            Self::StaticMethod => "static method",
            Self::Field => "instance field",
            Self::StaticField => "static field",
        }
    }
}

pub(super) struct RustCall {
    pub(super) kind: CallKind,
    pub(super) name: String,
    pub(super) line: usize,
    pub(super) receiver_class: Option<String>,
    pub(super) descriptor: std::result::Result<String, String>,
}

/// Helper function name to the class of the object it passes to its closure.
pub(super) fn helper_classes(
    activity: &str,
    rust: &[(String, RustSource)],
) -> BTreeMap<String, String> {
    let mut helpers = BTreeMap::from([("with_activity".to_owned(), activity.to_owned())]);
    for (_, source) in rust {
        let structural = source.structural();
        for (start, _) in structural.match_indices("fn with_") {
            let name_start = start + 3;
            let name_end = identifier_end(structural.as_bytes(), name_start);
            let name = &structural[name_start..name_end];
            if name == "with_activity" {
                continue;
            }
            let body_end = source.item_end(start);
            if let Some(class) = first_get_field_type(source, name_end..body_end) {
                helpers.insert(name.to_owned(), class);
            }
        }
    }
    helpers
}

fn first_get_field_type(source: &RustSource, range: std::ops::Range<usize>) -> Option<String> {
    let structural = &source.structural()[range.clone()];
    let offset = structural.find("get_field")? + range.start;
    let open = source.structural()[offset..].find('(')? + offset;
    let close = source.matching_close(open)?;
    let args = split_top_level(&source.structural()[open + 1..close], ',');
    let signature = macro_argument(source, open + 1, args.get(2)?, "jni_sig")?;
    let descriptor = jni_sig_field_descriptor(&signature).ok()?;
    descriptor
        .strip_prefix('L')
        .and_then(|class| class.strip_suffix(';'))
        .map(|class| class.replace(['/', '$'], "."))
}

/// `call_method`, `call_static_method`, `get_field` and `get_static_field`
/// calls whose member and signature are given with `jni_str!`/`jni_sig!`.
pub(super) fn rust_calls(source: &RustSource, helpers: &BTreeMap<String, String>) -> Vec<RustCall> {
    let structural = source.structural();
    let bytes = structural.as_bytes();
    let mut calls = Vec::new();
    for (keyword, kind) in [
        ("call_method", CallKind::Method),
        ("call_static_method", CallKind::StaticMethod),
        ("get_field", CallKind::Field),
        ("get_static_field", CallKind::StaticField),
    ] {
        for (start, _) in structural.match_indices(keyword) {
            if !is_word_at(bytes, start, keyword.len())
                || !structural[..start].trim_end().ends_with('.')
            {
                continue;
            }
            let Some(open) = structural[start..].find('(').map(|offset| start + offset) else {
                continue;
            };
            let Some(close) = source.matching_close(open) else {
                continue;
            };
            let arguments = split_top_level(&structural[open + 1..close], ',');
            if arguments.len() < 3 {
                continue;
            }
            let names = match macro_argument(source, open + 1, arguments[1], "jni_str") {
                Some(name) => vec![Ok(name.trim_matches('"').to_owned())],
                None => forwarded_jni_str_arguments(source, start, arguments[1].trim()),
            };
            let descriptor = match macro_argument(source, open + 1, arguments[2], "jni_sig") {
                Some(signature) => match kind {
                    CallKind::Method | CallKind::StaticMethod => {
                        jni_sig_method_descriptor(&signature)
                    }
                    CallKind::Field | CallKind::StaticField => jni_sig_field_descriptor(&signature),
                },
                None => Err("the signature is not a jni_sig! literal".to_owned()),
            };
            let receiver = arguments[0]
                .trim()
                .trim_start_matches('&')
                .trim()
                .to_owned();
            let receiver_class = receiver_class(source, start, &receiver, helpers);
            for name in names {
                let (name, descriptor) = match name {
                    Ok(name) => (name, descriptor.clone()),
                    Err(error) => (arguments[1].trim().to_owned(), Err(error)),
                };
                calls.push(RustCall {
                    kind,
                    name,
                    line: source.line_of(start) + 1,
                    receiver_class: receiver_class.clone(),
                    descriptor,
                });
            }
        }
    }
    calls.sort_by_key(|call| call.line);
    calls
}

/// Resolves a member name passed in through a parameter of the enclosing
/// function: each caller must pass a `jni_str!` literal at that position.
fn forwarded_jni_str_arguments(
    source: &RustSource,
    position: usize,
    argument: &str,
) -> Vec<std::result::Result<String, String>> {
    let unresolved = || {
        vec![Err(format!(
            "member name `{argument}` is neither a jni_str! literal nor a forwarded parameter"
        ))]
    };
    let structural = source.structural();
    let bytes = structural.as_bytes();
    if argument.is_empty() || identifier_end(argument.as_bytes(), 0) != argument.len() {
        return unresolved();
    }
    // The innermost function whose item contains the call.
    let Some((name, parameter_index)) = structural
        .match_indices("fn ")
        .filter(|(start, _)| *start < position && is_word_at(bytes, *start, 2))
        .filter(|(start, _)| source.item_end(*start) > position)
        .last()
        .and_then(|(start, _)| {
            let name_start = start + 3;
            let name_end = identifier_end(bytes, name_start);
            let open = name_end + structural[name_end..].find('(')?;
            let close = source.matching_close(open)?;
            let index = split_top_level(&structural[open + 1..close], ',')
                .iter()
                .position(|parameter| {
                    parameter
                        .split(':')
                        .next()
                        .map(|name| name.trim().trim_start_matches("mut ").trim())
                        == Some(argument)
                })?;
            Some((structural[name_start..name_end].to_owned(), index))
        })
    else {
        return unresolved();
    };
    let mut names = Vec::new();
    for (call, _) in structural.match_indices(name.as_str()) {
        let after = call + name.len();
        if !is_word_at(bytes, call, name.len())
            || bytes.get(after) != Some(&b'(')
            || structural[..call].trim_end().ends_with("fn")
        {
            continue;
        }
        let Some(close) = source.matching_close(after) else {
            continue;
        };
        let arguments = split_top_level(&structural[after + 1..close], ',');
        names.push(
            arguments
                .get(parameter_index)
                .and_then(|argument| macro_argument(source, after + 1, argument, "jni_str"))
                .map(|name| name.trim_matches('"').to_owned())
                .ok_or_else(|| {
                    format!(
                        "caller of {name} on line {} passes no jni_str! literal",
                        source.line_of(call) + 1
                    )
                }),
        );
    }
    if names.is_empty() {
        unresolved()
    } else {
        names
    }
}

/// The original text of `macro!(...)` when `argument` is such an invocation.
fn macro_argument(
    source: &RustSource,
    list_start: usize,
    argument: &str,
    macro_name: &str,
) -> Option<String> {
    let structural = source.structural();
    let argument_start = list_start + structural[list_start..].find(argument.trim_start())?;
    let marker = format!("{macro_name}!");
    let relative =
        structural[argument_start..argument_start + argument.trim_start().len()].find(&marker)?;
    let open = argument_start + relative + marker.len();
    let open = open + structural[open..].find('(')?;
    let close = source.matching_close(open)?;
    Some(source.text()[open + 1..close].trim().to_owned())
}

/// The class of `receiver` when it is the object a `with_*` helper passes to
/// the closure that encloses `position`.
pub(super) fn receiver_class(
    source: &RustSource,
    position: usize,
    receiver: &str,
    helpers: &BTreeMap<String, String>,
) -> Option<String> {
    let structural = source.structural();
    let mut best: Option<(usize, String)> = None;
    for (helper, class) in helpers {
        for (start, _) in structural.match_indices(helper.as_str()) {
            if !is_word_at(structural.as_bytes(), start, helper.len()) || start >= position {
                continue;
            }
            let Some(open) = structural[start..].find('(').map(|offset| start + offset) else {
                continue;
            };
            if open != start + helper.len() {
                continue;
            }
            let Some(close) = source.matching_close(open) else {
                continue;
            };
            if close < position {
                continue;
            }
            // The closure's second parameter names the helper's object.
            let arguments = &structural[open + 1..close];
            let Some(first_bar) = arguments.find('|') else {
                continue;
            };
            let Some(second_bar) = arguments[first_bar + 1..].find('|') else {
                continue;
            };
            let parameters = &arguments[first_bar + 1..first_bar + 1 + second_bar];
            let object = parameters.split(',').nth(1).map(str::trim);
            if object == Some(receiver) && best.as_ref().is_none_or(|(at, _)| start > *at) {
                best = Some((start, class.clone()));
            }
        }
    }
    best.map(|(_, class)| class)
}

/// The descriptor of a method `jni_sig!`: `(JString, i32) -> void`.
pub(super) fn jni_sig_method_descriptor(signature: &str) -> std::result::Result<String, String> {
    let signature = strip_sig_prefix(signature);
    let open = signature
        .find('(')
        .ok_or_else(|| format!("method signature without parameters: {signature}"))?;
    let close = matching_paren(signature, open)
        .ok_or_else(|| format!("unbalanced method signature: {signature}"))?;
    let mut descriptor = String::from("(");
    for parameter in split_top_level(&signature[open + 1..close], ',') {
        let ty = parameter.split_once(':').map_or(parameter, |(_, ty)| ty);
        descriptor.push_str(&jni_sig_type_descriptor(ty.trim())?);
    }
    descriptor.push(')');
    let return_type = signature[close + 1..]
        .trim()
        .strip_prefix("->")
        .map_or("void", str::trim);
    descriptor.push_str(&jni_sig_type_descriptor(return_type)?);
    Ok(descriptor)
}

fn jni_sig_field_descriptor(signature: &str) -> std::result::Result<String, String> {
    jni_sig_type_descriptor(strip_sig_prefix(signature))
}

fn strip_sig_prefix(signature: &str) -> &str {
    let signature = signature.trim();
    signature
        .strip_prefix("sig")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('='))
        .map_or(signature, str::trim)
}

/// The descriptor of one `jni_sig!` type (see the jni crate's `jni_sig!` docs).
fn jni_sig_type_descriptor(ty: &str) -> std::result::Result<String, String> {
    let ty = ty.trim().trim_matches('"');
    if let Some(element) = ty.strip_suffix("[]") {
        return Ok(format!("[{}", jni_sig_type_descriptor(element)?));
    }
    if let Some(element) = ty.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        return Ok(format!("[{}", jni_sig_type_descriptor(element)?));
    }
    let primitive = match ty {
        "void" | "()" => Some("V"),
        "jboolean" | "boolean" | "bool" => Some("Z"),
        "jbyte" | "byte" | "i8" => Some("B"),
        "jchar" | "char" => Some("C"),
        "jshort" | "short" | "i16" => Some("S"),
        "jint" | "int" | "i32" => Some("I"),
        "jlong" | "long" | "i64" => Some("J"),
        "jfloat" | "float" | "f32" => Some("F"),
        "jdouble" | "double" | "f64" => Some("D"),
        _ => None,
    };
    if let Some(primitive) = primitive {
        return Ok(primitive.to_owned());
    }
    if ty.contains('.') {
        let class = ty.replace("::", "$").replace('.', "/");
        let class = class.strip_prefix('/').unwrap_or(&class);
        return Ok(format!("L{class};"));
    }
    let class = match base_type(ty) {
        "JString" => "java/lang/String",
        "JObject" => "java/lang/Object",
        "JClass" => "java/lang/Class",
        "JThrowable" => "java/lang/Throwable",
        "JList" => "java/util/List",
        "JMap" => "java/util/Map",
        other => return Err(format!("unsupported jni_sig! type {other}")),
    };
    Ok(format!("L{class};"))
}

pub(super) fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in text.char_indices().skip_while(|(index, _)| *index < open) {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits on `separator` outside `()`, `[]`, `{}` and generic `<>` (the `>`
/// of `->` and `=>` is not a bracket).
pub(super) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0isize;
    let mut start = 0;
    let mut previous = ' ';
    for (index, character) in text.char_indices() {
        let arrow = character == '>' && matches!(previous, '-' | '=');
        previous = character;
        match character {
            '>' if arrow => {}
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth -= 1,
            _ if character == separator && depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}

// ---------------------------------------------------------------------------
// Java side
