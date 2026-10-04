//! `cargo xtask jni-contract`: static checks of the Rust/Java JNI boundary.
//!
//! JNI mismatches compile on both sides and fail only at runtime with
//! `UnsatisfiedLinkError` or `NoSuchMethodError`. This check compares:
//!
//! * every `Java_de_duecki_calibraw_*` export in `calibraw-ffi` with a Java
//!   `native` declaration: class, name, static or instance receiver, parameter
//!   and return types (the implicit `JNIEnv` and `jclass`/`jobject` arguments
//!   are accounted for), and every Java `native` method with an export;
//! * every `call_method`/`call_static_method`/`get_field` that names its member
//!   with `jni_str!` and its type with `jni_sig!` against the Java declaration.
//!   The receiver class comes from the `with_*` helper whose object the call
//!   uses (`with_activity` is the manifest's activity; the other helpers read a
//!   typed activity field). Other receivers must match exactly one Java class.
//!
//! Java sources are parsed, not compiled, so the check needs no JDK or Android
//! SDK and runs in ordinary CI. Descriptors are built from erased source types
//! resolved through imports, nested classes, the package and `java.lang`.
//! Lifetimes, exception handling and thread attachment are not verified here;
//! they need an Android runtime smoke test.

use crate::process::{display_relative, source_files, workspace_root};
use crate::rust_source::{identifier_end, is_word_at, RustSource};
use crate::{Result, XtaskError};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const PACKAGE: &str = "de.duecki.calibraw";
const FFI_SOURCES: &str = "crates/calibraw-ffi/src";
const JAVA_SOURCES: &str = "android/app/src/main/java/de/duecki/calibraw";
const MANIFEST: &str = "android/app/src/main/AndroidManifest.xml";

pub(crate) fn run() -> Result<()> {
    let root = workspace_root();
    let java = JavaModel::load(&root.join(JAVA_SOURCES))?;
    let activity = manifest_activity(&root.join(MANIFEST))?;
    let mut rust = Vec::new();
    for file in source_files(&root.join(FFI_SOURCES), &["rs"])? {
        let source = RustSource::new(fs::read_to_string(&file)?);
        rust.push((display_relative(&root, &file), source));
    }
    let report = check(&java, &activity, &rust);
    for line in &report.passed {
        println!("ok    {line}");
    }
    for line in &report.failures {
        println!("FAIL  {line}");
    }
    println!(
        "{} JNI contracts checked, {} failed",
        report.passed.len() + report.failures.len(),
        report.failures.len()
    );
    if report.failures.is_empty() {
        Ok(())
    } else {
        Err(XtaskError::silent(1))
    }
}

#[derive(Default)]
struct Report {
    passed: Vec<String>,
    failures: Vec<String>,
}

fn check(java: &JavaModel, activity: &str, rust: &[(String, RustSource)]) -> Report {
    let mut report = Report::default();
    let mut exported = BTreeSet::new();
    let helpers = helper_classes(activity, rust);

    for (file, source) in rust {
        for export in rust_exports(source) {
            let label = format!("{file}: export {}.{}", export.class, export.method);
            exported.insert((export.class.clone(), export.method.clone()));
            let Some(class) = java.classes.get(&export.class) else {
                report
                    .failures
                    .push(format!("{label}: Java class {} not found", export.class));
                continue;
            };
            let natives = class
                .methods
                .iter()
                .filter(|method| method.native && method.name == export.method)
                .collect::<Vec<_>>();
            match natives.as_slice() {
                [] => report.failures.push(format!(
                    "{label}: no Java `native` declaration in {}",
                    export.class
                )),
                [method] => {
                    let descriptor = match &export.descriptor {
                        Ok(descriptor) => descriptor,
                        Err(error) => {
                            report.failures.push(format!("{label}: {error}"));
                            continue;
                        }
                    };
                    if method.is_static != export.is_static {
                        report.failures.push(format!(
                            "{label}: Rust takes {} but Java declares a{} method",
                            if export.is_static {
                                "a jclass"
                            } else {
                                "a jobject"
                            },
                            if method.is_static {
                                " static"
                            } else {
                                "n instance"
                            },
                        ));
                    } else if method.descriptor.as_ref() != Ok(descriptor) {
                        report.failures.push(format!(
                            "{label}: Rust signature {descriptor} differs from Java {}",
                            describe(&method.descriptor)
                        ));
                    } else {
                        report.passed.push(format!("{label} {descriptor}"));
                    }
                }
                _ => report.failures.push(format!(
                    "{label}: overloaded native methods need explicit JNI long names"
                )),
            }
        }
    }
    for (class_name, class) in &java.classes {
        for method in class.methods.iter().filter(|method| method.native) {
            if !exported.contains(&(class_name.clone(), method.name.clone())) {
                report.failures.push(format!(
                    "{}: Java native {class_name}.{} has no Rust export",
                    class.file, method.name
                ));
            }
        }
    }

    for (file, source) in rust {
        for call in rust_calls(source, &helpers) {
            let label = format!("{file}:{}: {} {}", call.line, call.kind.label(), call.name);
            let descriptor = match &call.descriptor {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    report.failures.push(format!("{label}: {error}"));
                    continue;
                }
            };
            let candidates: Vec<&str> = match &call.receiver_class {
                Some(class) => vec![class.as_str()],
                None => java.classes.keys().map(String::as_str).collect(),
            };
            let matches = candidates
                .iter()
                .filter(|class| java.member_matches(class, &call, descriptor))
                .collect::<Vec<_>>();
            match (matches.as_slice(), &call.receiver_class) {
                ([class], _) => report
                    .passed
                    .push(format!("{label} {descriptor} on {class}")),
                ([], Some(class)) => report.failures.push(format!(
                    "{label} {descriptor}: no matching {} in {class}; declared: {}",
                    call.kind.member(),
                    java.declared(class, &call.name)
                )),
                ([], None) => report.failures.push(format!(
                    "{label} {descriptor}: no Java class declares a matching {}",
                    call.kind.member()
                )),
                (many, _) => report.failures.push(format!(
                    "{label} {descriptor}: receiver class is ambiguous ({})",
                    many.iter()
                        .map(|class| class.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
    report
}

fn describe(descriptor: &std::result::Result<String, String>) -> String {
    match descriptor {
        Ok(descriptor) => descriptor.clone(),
        Err(error) => format!("<{error}>"),
    }
}

/// The activity class declared in the Android manifest.
fn manifest_activity(manifest: &Path) -> Result<String> {
    let text = fs::read_to_string(manifest)
        .map_err(|error| XtaskError::new(format!("{}: {error}", manifest.display())))?;
    let activity = text
        .find("<activity")
        .ok_or_else(|| XtaskError::new("AndroidManifest.xml declares no activity"))?;
    let element_end = text[activity..]
        .find('>')
        .map_or(text.len(), |end| activity + end);
    let element = &text[activity..element_end];
    let name_start = element
        .find("android:name=\"")
        .ok_or_else(|| XtaskError::new("the manifest activity has no android:name"))?
        + "android:name=\"".len();
    let name_end = element[name_start..]
        .find('"')
        .map_or(element.len(), |end| name_start + end);
    let name = &element[name_start..name_end];
    Ok(match name.strip_prefix('.') {
        Some(simple) => format!("{PACKAGE}.{simple}"),
        None if name.contains('.') => name.to_owned(),
        None => format!("{PACKAGE}.{name}"),
    })
}

// ---------------------------------------------------------------------------
// Rust side

struct RustExport {
    class: String,
    method: String,
    is_static: bool,
    descriptor: std::result::Result<String, String>,
}

/// `Java_<package>_<Class>_<method>` exports and their JNI descriptors.
fn rust_exports(source: &RustSource) -> Vec<RustExport> {
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
fn decode_export_name(mangled: &str) -> Option<(String, String)> {
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
enum CallKind {
    Method,
    StaticMethod,
    Field,
    StaticField,
}

impl CallKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Method => "call_method",
            Self::StaticMethod => "call_static_method",
            Self::Field => "get_field",
            Self::StaticField => "get_static_field",
        }
    }

    const fn member(self) -> &'static str {
        match self {
            Self::Method => "instance method",
            Self::StaticMethod => "static method",
            Self::Field => "instance field",
            Self::StaticField => "static field",
        }
    }
}

struct RustCall {
    kind: CallKind,
    name: String,
    line: usize,
    receiver_class: Option<String>,
    descriptor: std::result::Result<String, String>,
}

/// Helper function name to the class of the object it passes to its closure.
fn helper_classes(activity: &str, rust: &[(String, RustSource)]) -> BTreeMap<String, String> {
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
fn rust_calls(source: &RustSource, helpers: &BTreeMap<String, String>) -> Vec<RustCall> {
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
fn receiver_class(
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
fn jni_sig_method_descriptor(signature: &str) -> std::result::Result<String, String> {
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

fn matching_paren(text: &str, open: usize) -> Option<usize> {
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
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
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

struct JavaModel {
    /// Fully qualified binary-ish class name (`a.b.Outer.Inner`) to its members.
    classes: BTreeMap<String, JavaClass>,
}

struct JavaClass {
    file: String,
    methods: Vec<JavaMethod>,
    fields: Vec<JavaField>,
}

struct JavaMethod {
    name: String,
    is_static: bool,
    native: bool,
    descriptor: std::result::Result<String, String>,
}

struct JavaField {
    name: String,
    is_static: bool,
    descriptor: std::result::Result<String, String>,
}

impl JavaModel {
    fn load(directory: &Path) -> Result<Self> {
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

    fn parse(files: &[(String, String)]) -> Self {
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

    fn member_matches(&self, class: &str, call: &RustCall, descriptor: &str) -> bool {
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

    fn declared(&self, class: &str, name: &str) -> String {
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
    declared: &'a BTreeSet<String>,
}

impl TypeResolver<'_> {
    /// JNI descriptor of a Java source type, with generics erased.
    fn descriptor(&self, ty: &str, scope: &str) -> std::result::Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const JAVA: &str = r#"
package de.duecki.calibraw;

import android.net.Uri;
import java.util.ArrayList;

public final class Host extends Base {
    private Store store;
    @Override
    protected void onCreate(Bundle state) {}
    private static native boolean nativeBack();
    private static native void nativePicked(String path, int fd);
    public String path(String uri, long size) { return ""; }
    ArrayList<Uri> uris() { return null; }
    Store.Entry entry(byte[] data) { return null; }

    static final class Store {
        static final class Entry {}
        void put(String key, Uri value) {}
    }
}
"#;

    const RUST: &str = r#"
#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_Host_nativeBack<'local>(
    _env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jni::sys::jboolean { true }

#[unsafe(no_mangle)]
pub extern "system" fn Java_de_duecki_calibraw_Host_nativePicked<'local>(
    _env: EnvUnowned<'local>,
    _class: JClass<'local>,
    path: JString<'local>,
    fd: jni::sys::jint,
) {}

fn with_store<T>(app: &App, operation: impl FnOnce(&mut Env, &JObject) -> T) -> T {
    with_activity(app, |env, activity| {
        let store = env.get_field(activity, jni::jni_str!("store"), jni::jni_sig!(de.duecki.calibraw.Host::Store))?;
        operation(env, &store)
    })
}

fn calls(app: &App) {
    with_activity(app, |env, activity| {
        env.call_method(activity, jni::jni_str!("path"), jni::jni_sig!((JString, i64) -> JString), &[]);
    });
    with_store(app, |env, store| {
        env.call_method(store, jni::jni_str!("put"), jni::jni_sig!((JString, android.net.Uri) -> void), &[]);
    });
    with_activity(app, |env, _| {
        env.call_method(&self.other, jni::jni_str!("entry"), jni::jni_sig!((byte[]) -> de.duecki.calibraw.Host::Store::Entry), &[]);
    });
}
"#;

    fn run_fixture(java: &str, rust: &str) -> Report {
        let model = JavaModel::parse(&[("Host.java".to_owned(), java.to_owned())]);
        check(
            &model,
            "de.duecki.calibraw.Host",
            &[("lib.rs".to_owned(), RustSource::new(rust))],
        )
    }

    #[test]
    fn matching_exports_calls_and_fields_pass() {
        let report = run_fixture(JAVA, RUST);
        assert!(report.failures.is_empty(), "{:#?}", report.failures);
        // Two exports, the helper's get_field and three calls.
        assert_eq!(report.passed.len(), 6, "{:#?}", report.passed);
        assert!(report
            .passed
            .iter()
            .any(|line| line.contains("put") && line.contains("Host.Store")));
    }

    #[test]
    fn signature_static_and_missing_mismatches_fail() {
        let java = JAVA
            .replace(
                "private static native void nativePicked(String path, int fd);",
                "private native void nativePicked(String path, long fd);",
            )
            .replace("String path(String uri, long size)", "String path(String uri, int size)")
            .replace("private static native boolean nativeBack();", "private static native boolean nativeBack();\n    private static native void nativeOrphan();");
        let report = run_fixture(&java, RUST);
        let failures = report.failures.join("\n");
        assert!(
            failures.contains("nativePicked: Rust takes a jclass"),
            "{failures}"
        );
        assert!(
            failures.contains("nativeOrphan has no Rust export"),
            "{failures}"
        );
        assert!(failures.contains("call_method path"), "{failures}");
    }

    #[test]
    fn forwarded_member_names_are_resolved_through_their_callers() {
        let rust = format!(
            "{RUST}\nfn lookup(app: &App, method: impl AsRef<JNIStr>) {{\n    with_activity(app, |env, activity| {{\n        env.call_method(activity, method, jni::jni_sig!((JString, i64) -> JString), &[]);\n    }});\n}}\nfn uses(app: &App) {{ lookup(app, jni::jni_str!(\"path\")); lookup(app, jni::jni_str!(\"missing\")); }}\n"
        );
        let report = run_fixture(JAVA, &rust);
        assert!(
            report
                .passed
                .iter()
                .filter(|line| line.contains(" path "))
                .count()
                >= 2
        );
        assert_eq!(report.failures.len(), 1, "{:#?}", report.failures);
        assert!(report.failures[0].contains("missing"));

        let unresolved = format!(
            "{RUST}\nfn dynamic(app: &App, name: &str) {{\n    with_activity(app, |env, activity| {{\n        env.call_method(activity, make(name), jni::jni_sig!(() -> void), &[]);\n    }});\n}}\n"
        );
        let report = run_fixture(JAVA, &unresolved);
        assert!(
            report
                .failures
                .iter()
                .any(|line| line.contains("neither a jni_str! literal")),
            "{:#?}",
            report.failures
        );
    }

    #[test]
    fn jni_sig_types_map_to_descriptors() {
        assert_eq!(
            jni_sig_method_descriptor(
                "(JString, i32, i32, i32) -> de.duecki.calibraw.ReplayVideoEncoder"
            )
            .unwrap(),
            "(Ljava/lang/String;III)Lde/duecki/calibraw/ReplayVideoEncoder;"
        );
        assert_eq!(
            jni_sig_method_descriptor("(byte[]) -> void").unwrap(),
            "([B)V"
        );
        assert_eq!(jni_sig_method_descriptor("() -> i64").unwrap(), "()J");
        assert_eq!(
            jni_sig_method_descriptor("(arg: [jint], name: \"a.b.Outer$Inner\")").unwrap(),
            "([ILa/b/Outer$Inner;)V"
        );
        assert_eq!(decode_export_name("My_1Class_run").unwrap().0, "My_Class");
    }
}
