//! Android native build orchestration and APK verification.
//!
//! The pinned NDK, build-tools and SDK levels come from
//! `[workspace.metadata]` in the root `Cargo.toml` (the build contract).

use crate::process::{
    directory_has_extension, executable_name, next_value, remove_path, require_executable,
    require_file, rooted, run_checked, run_command_bytes, run_command_output, temporary_directory,
    workspace_root,
};
use crate::{print_help, Result, XtaskError};
use serde_json::Value;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use zip::ZipArchive;

const ANDROID_64_BIT_ABIS: [&str; 1] = ["arm64-v8a"];
const CARGO_NDK_VERSION: &str = "4.1.2";

/// `cargo xtask build-android [ABI] [PROFILE]`
pub(crate) fn build_android(args: Vec<OsString>) -> Result<()> {
    command_build_android(parse_build_android_args(args)?)
}

/// `cargo xtask build-android-libraw [ABI]`
pub(crate) fn build_android_libraw(args: Vec<OsString>) -> Result<()> {
    command_build_android_dependency(
        parse_build_dependency_args(args, "build-android-libraw")?,
        AndroidDependency::LibRaw,
    )
}

/// `cargo xtask build-android-lensfun [ABI]`
pub(crate) fn build_android_lensfun(args: Vec<OsString>) -> Result<()> {
    command_build_android_dependency(
        parse_build_dependency_args(args, "build-android-lensfun")?,
        AndroidDependency::Lensfun,
    )
}

/// `cargo xtask verify-android-16kb [APK] [--objdump PATH] [--zipalign PATH]`
pub(crate) fn verify_android_16kb(args: Vec<OsString>) -> Result<()> {
    command_verify_android_16kb(parse_android_args(args)?)
}

#[derive(Debug)]
struct BuildAndroidArgs {
    abi: String,
    profile: String,
}

fn parse_build_android_args(args: Vec<OsString>) -> Result<BuildAndroidArgs> {
    let mut positionals = Vec::new();
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown build-android option: {value}"
                )));
            }
            _ => positionals.push(argument.to_string_lossy().into_owned()),
        }
    }
    if positionals.len() > 2 {
        return Err(XtaskError::usage(
            "build-android accepts at most ABI and PROFILE positional arguments",
        ));
    }
    Ok(BuildAndroidArgs {
        abi: positionals
            .first()
            .cloned()
            .unwrap_or_else(|| "arm64-v8a".to_owned()),
        profile: positionals
            .get(1)
            .cloned()
            .unwrap_or_else(|| "release".to_owned()),
    })
}

#[derive(Debug)]
struct BuildDependencyArgs {
    abi: String,
}

fn parse_build_dependency_args(args: Vec<OsString>, command: &str) -> Result<BuildDependencyArgs> {
    let mut abi = None;
    for argument in args {
        match argument.to_string_lossy().as_ref() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown {command} option: {value}"
                )));
            }
            _ if abi.is_none() => abi = Some(argument.to_string_lossy().into_owned()),
            _ => {
                return Err(XtaskError::usage(format!(
                    "{command} accepts at most one ABI positional argument"
                )));
            }
        }
    }
    Ok(BuildDependencyArgs {
        abi: abi.unwrap_or_else(|| "arm64-v8a".to_owned()),
    })
}

#[derive(Debug)]
struct AndroidArgs {
    apk: PathBuf,
    objdump: Option<PathBuf>,
    zipalign: Option<PathBuf>,
}

fn parse_android_args(args: Vec<OsString>) -> Result<AndroidArgs> {
    let mut apk = None;
    let mut objdump = None;
    let mut zipalign = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--objdump" => {
                objdump = Some(PathBuf::from(next_value(&args, &mut index, "--objdump")?));
            }
            "--zipalign" => {
                zipalign = Some(PathBuf::from(next_value(&args, &mut index, "--zipalign")?));
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(XtaskError::usage(format!(
                    "unknown verify-android-16kb option: {value}"
                )));
            }
            _ => {
                if apk.is_some() {
                    return Err(XtaskError::usage(
                        "verify-android-16kb accepts exactly one APK path",
                    ));
                }
                apk = Some(PathBuf::from(args[index].clone()));
            }
        }
        index += 1;
    }

    Ok(AndroidArgs {
        apk: apk
            .unwrap_or_else(|| PathBuf::from("android/app/build/outputs/apk/debug/app-debug.apk")),
        objdump,
        zipalign,
    })
}

#[derive(Debug)]
struct BuildContract {
    ndk_version: String,
    build_tools_version: String,
    min_sdk: u64,
}

fn load_build_contract() -> Result<BuildContract> {
    let root = workspace_root();
    let output = run_command_bytes(
        Command::new("cargo")
            .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
            .current_dir(&root),
        "cargo metadata",
    )?;

    let document: Value = serde_json::from_slice(&output)?;
    let metadata = document
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| XtaskError::new("Cargo.toml is missing [workspace.metadata]"))?;
    let string = |key: &str| -> Result<String> {
        metadata
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| XtaskError::new(format!("workspace.metadata.{key} must be a string")))
    };
    let integer = |key: &str| -> Result<u64> {
        metadata
            .get(key)
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                XtaskError::new(format!(
                    "workspace.metadata.{key} must be a positive integer"
                ))
            })
    };

    Ok(BuildContract {
        ndk_version: string("android_ndk_version")?,
        build_tools_version: string("android_build_tools_version")?,
        min_sdk: integer("android_min_sdk")?,
    })
}

fn parse_properties_file(path: &Path) -> Result<Vec<(String, String)>> {
    let source = fs::read_to_string(path)
        .map_err(|error| XtaskError::new(format!("cannot read {}: {error}", path.display())))?;
    let mut values = Vec::new();
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    Ok(values)
}

fn property_value<'a>(properties: &'a [(String, String)], key: &str) -> Option<&'a str> {
    properties
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn android_abi_config(abi: &str, api: u64) -> Result<(String, &'static str)> {
    match abi {
        "arm64-v8a" => Ok((
            format!("aarch64-linux-android{api}"),
            "aarch64-linux-android",
        )),
        _ => Err(XtaskError::usage(format!(
            "Unsupported ABI '{abi}' (use arm64-v8a)"
        ))),
    }
}

fn android_sdk_root_with_local_properties(root: &Path) -> Result<Option<PathBuf>> {
    if let Some(configured) =
        env::var_os("ANDROID_SDK_ROOT").or_else(|| env::var_os("ANDROID_HOME"))
    {
        return Ok(Some(rooted(root, configured)));
    }
    let local_properties = root.join("android/local.properties");
    if !local_properties.is_file() {
        return Ok(None);
    }
    let properties = parse_properties_file(&local_properties)?;
    Ok(property_value(&properties, "sdk.dir").map(|value| rooted(root, value)))
}

fn android_ndk_root(
    root: &Path,
    expected_version: &str,
    require_toolchain: bool,
) -> Result<PathBuf> {
    let sdk = android_sdk_root_with_local_properties(root)?;
    let configured = env::var_os("ANDROID_NDK_HOME").or_else(|| env::var_os("ANDROID_NDK_ROOT"));
    let ndk = configured
        .map(|path| rooted(root, path))
        .or_else(|| sdk.map(|sdk| sdk.join("ndk").join(expected_version)))
        .ok_or_else(|| {
            XtaskError::new("Android NDK not found. Set ANDROID_NDK_HOME (or ANDROID_SDK_ROOT).")
        })?;
    if require_toolchain && !ndk.join("build/cmake/android.toolchain.cmake").is_file() {
        return Err(XtaskError::new(format!(
            "Android NDK toolchain not found at {}",
            ndk.display()
        )));
    }
    let source_properties = ndk.join("source.properties");
    if !source_properties.is_file() {
        return Err(XtaskError::new(format!(
            "Android NDK not found at {}",
            ndk.display()
        )));
    }
    let properties = parse_properties_file(&source_properties)?;
    let revision = property_value(&properties, "Pkg.Revision").unwrap_or("");
    if revision != expected_version {
        return Err(XtaskError::new(format!(
            "Android NDK {expected_version} is required, found {} at {}",
            if revision.is_empty() {
                "unknown"
            } else {
                revision
            },
            ndk.display()
        )));
    }
    Ok(ndk)
}

fn ndk_host_root(ndk: &Path) -> Result<PathBuf> {
    let prebuilt = ndk.join("toolchains/llvm/prebuilt");
    let mut candidates = if prebuilt.is_dir() {
        fs::read_dir(&prebuilt)?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        XtaskError::new(format!(
            "The selected NDK has no LLVM toolchain: {}",
            ndk.display()
        ))
    })
}

fn find_named_file_recursive(root: &Path, prefix: &str) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(directory).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with(prefix))
            {
                return Some(path);
            }
        }
    }
    None
}

fn find_host_libclang(ndk_host: &Path) -> Option<PathBuf> {
    for candidate in [ndk_host.join("lib64"), ndk_host.join("lib")] {
        if find_named_file_recursive(&candidate, "libclang.so").is_some() {
            return Some(candidate);
        }
    }
    find_named_file_recursive(Path::new("/usr/lib"), "libclang.so")
        .and_then(|library| library.parent().map(Path::to_path_buf))
}

fn validate_android_profile(profile: &str) -> Result<()> {
    if matches!(profile, "debug" | "release") {
        Ok(())
    } else {
        Err(XtaskError::usage(format!(
            "Unknown profile '{profile}' (use release or debug)"
        )))
    }
}

fn require_lensfun_assets(staged: &Path) -> Result<()> {
    let assets = staged.join("apk-assets/lensfun");
    if directory_has_extension(&assets, "xml")? {
        Ok(())
    } else {
        Err(XtaskError::new(format!(
            "Lensfun XML database is missing from {}",
            assets.display()
        )))
    }
}

fn run_gradle_android_native_dependencies(
    root: &Path,
    abi: &str,
    profile: &str,
    min_sdk: u64,
) -> Result<()> {
    android_abi_config(abi, min_sdk)?;
    validate_android_profile(profile)?;
    let gradlew = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    require_file(&gradlew)?;
    let mut title = profile.to_owned();
    if let Some(first) = title.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    let task = format!(":app:externalNativeBuild{title}");
    run_checked(
        Command::new(&gradlew)
            .current_dir(root)
            .arg(task)
            .arg(format!("-PcalibrawAbis={abi}"))
            .arg("-PcalibrawBuildRust=false"),
        &gradlew.display().to_string(),
    )
}

#[derive(Debug, Clone, Copy)]
enum AndroidDependency {
    LibRaw,
    Lensfun,
}

fn command_build_android_dependency(
    args: BuildDependencyArgs,
    dependency: AndroidDependency,
) -> Result<()> {
    let contract = load_build_contract()?;
    let root = workspace_root();
    run_gradle_android_native_dependencies(&root, &args.abi, "release", contract.min_sdk)?;
    match dependency {
        AndroidDependency::LibRaw => {
            let staged = root.join("android/native/libraw").join(&args.abi);
            require_file(&staged.join("include/libraw/libraw.h"))?;
            require_file(&staged.join("lib/libraw.a"))?;
            println!(
                "AGP/CMake staged LibRaw for {} in {}",
                args.abi,
                staged.display()
            );
        }
        AndroidDependency::Lensfun => {
            let staged = root.join("android/native/lensfun").join(&args.abi);
            require_file(&staged.join("include/lensfun/lensfun.h"))?;
            require_file(&staged.join("lib/liblensfun.a"))?;
            require_file(&staged.join("lib/libglib-2.0.a"))?;
            require_lensfun_assets(&staged)?;
            println!(
                "AGP/CMake staged Lensfun for {} in {}",
                args.abi,
                staged.display()
            );
        }
    }
    Ok(())
}

fn command_build_android(args: BuildAndroidArgs) -> Result<()> {
    let contract = load_build_contract()?;
    let root = workspace_root();
    let (clang_target, cxx_triple) = android_abi_config(&args.abi, contract.min_sdk)?;
    validate_android_profile(&args.profile)?;

    let ndk = android_ndk_root(&root, &contract.ndk_version, true)?;
    let ndk_host = ndk_host_root(&ndk)?;
    let sysroot = ndk_host.join("sysroot");
    if !sysroot.is_dir() {
        return Err(XtaskError::new(format!(
            "The selected NDK has no LLVM sysroot: {}",
            ndk.display()
        )));
    }

    require_executable(
        "cargo-ndk",
        "cargo-ndk 4.1.2 is required. Install it with: cargo install cargo-ndk --version 4.1.2 --locked",
    )?;
    let cargo = require_executable("cargo", "cargo is required")?;
    let version_output = run_command_output(
        Command::new(&cargo).args(["ndk", "--version"]),
        "cargo ndk --version",
    )?;
    let cargo_ndk_version = version_output
        .trim()
        .strip_prefix("cargo-ndk ")
        .unwrap_or(version_output.trim());
    if cargo_ndk_version != CARGO_NDK_VERSION {
        return Err(XtaskError::new(format!(
            "cargo-ndk {CARGO_NDK_VERSION} is required, found {}",
            if cargo_ndk_version.is_empty() {
                "unknown"
            } else {
                cargo_ndk_version
            }
        )));
    }

    if env::var_os("CALIBRAW_NATIVE_DEPS_READY").as_deref() != Some(OsStr::new("1")) {
        run_gradle_android_native_dependencies(&root, &args.abi, &args.profile, contract.min_sdk)?;
    }

    let libraw_root = root.join("android/native/libraw").join(&args.abi);
    let lensfun_root = root.join("android/native/lensfun").join(&args.abi);
    let jni_root = root.join("android/app/src/main/jniLibs");
    let abi_jni = jni_root.join(&args.abi);
    remove_path(&abi_jni)?;

    let mut command = Command::new(&cargo);
    command
        .current_dir(&root)
        .env("ANDROID_NDK_HOME", &ndk)
        .env(
            "BINDGEN_EXTRA_CLANG_ARGS",
            format!("--target={clang_target} --sysroot={}", sysroot.display()),
        )
        .env("CALIBRAW_LIBRAW_ROOT", &libraw_root)
        .env("CALIBRAW_LENSFUN_ROOT", &lensfun_root)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", root.join("target"))
        .args(["ndk", "-t"])
        .arg(&args.abi)
        .arg("-o")
        .arg(&jni_root)
        .args(["build", "--locked"]);

    if env::var_os("LIBCLANG_PATH").is_none() {
        if let Some(libclang) = find_host_libclang(&ndk_host) {
            command.env("LIBCLANG_PATH", libclang);
        }
    }
    for key in [
        "CARGO_BUILD_TARGET",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
    ] {
        command.env_remove(key);
    }
    if args.profile == "release" {
        let source_date_epoch = run_command_output(
            Command::new("git")
                .current_dir(&root)
                .args(["show", "-s", "--format=%ct", "HEAD"]),
            "git show source date",
        )?;
        command
            .arg("--release")
            .env("CALIBRAW_REQUIRE_COMMITTED_SOURCE", "1")
            .env("SOURCE_DATE_EPOCH", source_date_epoch.trim());
    }
    command
        .args(["--package", "calibraw-ui", "--lib", "--manifest-path"])
        .arg(root.join("Cargo.toml"));
    run_checked(&mut command, &cargo.display().to_string())?;

    let cxx_runtime = ndk_host
        .join("sysroot/usr/lib")
        .join(cxx_triple)
        .join("libc++_shared.so");
    require_file(&cxx_runtime)?;
    fs::create_dir_all(&abi_jni)?;
    fs::copy(&cxx_runtime, abi_jni.join("libc++_shared.so"))?;
    require_file(&abi_jni.join("libcalibraw.so"))?;
    require_file(&abi_jni.join("libc++_shared.so"))?;

    require_lensfun_assets(&lensfun_root)?;
    println!(
        "Rust, LibRaw, and Lensfun Android libraries are ready for Gradle ({}, {}).",
        args.abi, args.profile
    );
    Ok(())
}

fn command_verify_android_16kb(args: AndroidArgs) -> Result<()> {
    let contract = load_build_contract()?;
    let root = workspace_root();
    let apk = rooted(&root, args.apk);
    if !apk.is_file() {
        return Err(XtaskError::new(format!("APK not found: {}", apk.display())));
    }

    let sdk = android_sdk_root_with_local_properties(&root)?.ok_or_else(|| {
        XtaskError::new("Android SDK not found. Set ANDROID_SDK_ROOT (or ANDROID_HOME).")
    })?;
    let ndk = android_ndk_root(&root, &contract.ndk_version, false)?;
    let ndk_host = ndk_host_root(&ndk)?;
    let objdump = args
        .objdump
        .or_else(|| env::var_os("LLVM_OBJDUMP").map(PathBuf::from))
        .unwrap_or_else(|| ndk_host.join("bin").join(executable_name("llvm-objdump")));
    let zipalign = args
        .zipalign
        .or_else(|| env::var_os("ZIPALIGN").map(PathBuf::from))
        .unwrap_or_else(|| {
            sdk.join("build-tools")
                .join(&contract.build_tools_version)
                .join(executable_name("zipalign"))
        });
    if !objdump.is_file() {
        return Err(XtaskError::new(format!(
            "llvm-objdump not found: {}",
            objdump.display()
        )));
    }
    if !zipalign.is_file() {
        return Err(XtaskError::new(format!(
            "zipalign {} not found: {}",
            contract.build_tools_version,
            zipalign.display()
        )));
    }

    let temporary = temporary_directory("calibraw-16kb")?;
    let libraries = extract_64_bit_libraries(&apk, temporary.path())?;
    if libraries.is_empty() {
        println!("No 64-bit native libraries found; ELF 16 KB check not applicable.");
    }
    for (archive_path, library) in &libraries {
        verify_elf_alignment(&objdump, library)?;
        println!("16 KB ELF aligned: {archive_path}");
    }

    run_checked(
        Command::new(&zipalign)
            .args(["-c", "-P", "16", "-v", "4"])
            .arg(&apk),
        &zipalign.display().to_string(),
    )?;
    println!("Android 16 KB page-size checks passed: {}", apk.display());
    Ok(())
}

fn extract_64_bit_libraries(apk: &Path, destination: &Path) -> Result<Vec<(String, PathBuf)>> {
    let file = File::open(apk)
        .map_err(|error| XtaskError::new(format!("cannot open APK {}: {error}", apk.display())))?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| XtaskError::new(format!("invalid APK {}: {error}", apk.display())))?;
    let mut libraries = Vec::new();

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        let archive_path = entry.name().replace('\\', "/");
        let Some((abi, file_name)) = native_library_path(&archive_path) else {
            continue;
        };
        if !ANDROID_64_BIT_ABIS.contains(&abi) {
            continue;
        }

        let abi_directory = destination.join(abi);
        fs::create_dir_all(&abi_directory)?;
        let output_path = abi_directory.join(file_name);
        let mut output = File::create(&output_path)?;
        io::copy(&mut entry, &mut output)?;
        libraries.push((archive_path, output_path));
    }

    libraries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(libraries)
}

fn native_library_path(path: &str) -> Option<(&str, &str)> {
    let mut parts = path.split('/');
    let root = parts.next()?;
    let abi = parts.next()?;
    let file = parts.next()?;
    if root != "lib"
        || parts.next().is_some()
        || !file.ends_with(".so")
        || !is_simple_file_name(abi)
        || !is_simple_file_name(file)
    {
        return None;
    }
    Some((abi, file))
}

fn is_simple_file_name(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
}

fn verify_elf_alignment(objdump: &Path, library: &Path) -> Result<()> {
    let output = Command::new(objdump)
        .arg("-p")
        .arg(library)
        .output()
        .map_err(|error| {
            XtaskError::new(format!("could not execute {}: {error}", objdump.display()))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let load_lines: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains("LOAD"))
        .collect();
    let alignments: Vec<u32> = load_lines
        .iter()
        .filter_map(|line| parse_alignment_power(line))
        .collect();
    if !output.status.success() || alignments.is_empty() {
        return Err(XtaskError::new(format!(
            "Could not read ELF LOAD alignment from {}",
            library.display()
        )));
    }
    if alignments.iter().any(|alignment| *alignment < 14) {
        eprintln!("16 KB ELF alignment check failed: {}", library.display());
        for line in load_lines {
            eprintln!("{line}");
        }
        return Err(XtaskError::silent(1));
    }
    Ok(())
}

fn parse_alignment_power(line: &str) -> Option<u32> {
    let marker = "align 2**";
    let start = line.find(marker)? + marker.len();
    line[start..]
        .split(|character: char| !character.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}
