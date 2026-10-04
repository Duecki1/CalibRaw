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

mod java_side;
mod rust_side;
use java_side::*;
use rust_side::*;

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
