//! `cargo xtask`: CalibRaw project diagnostics, verification and build helpers.
//!
//! Each command lives in its own module; this file only dispatches. The
//! commands and the checks they enforce are documented in
//! `docs/DEVELOPMENT.md`.

mod android;
mod arch;
mod baseline;
mod colorchecker;
mod error;
mod icons;
mod jni_contract;
mod loc;
mod process;
mod rust_source;

pub(crate) use error::{Result, XtaskError};

use process::ensure_no_extra_args;
use std::env;
use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if !error.message.is_empty() {
                eprintln!("error: {error}");
            }
            ExitCode::from(error.code.clamp(1, 255) as u8)
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os();
    let _program = args.next();
    let Some(command) = args.next() else {
        print_help();
        return Err(XtaskError::usage("missing command"));
    };
    let rest: Vec<OsString> = args.collect();

    match command.to_string_lossy().as_ref() {
        "arch-check" => {
            ensure_no_extra_args(&rest, "arch-check")?;
            arch::run()
        }
        "baseline-run" => baseline::run(rest),
        "baseline-compare" => baseline::compare(rest),
        "colorchecker-wb-validate" => colorchecker::run(rest),
        "icons" => {
            ensure_no_extra_args(&rest, "icons")?;
            icons::command_icons()
        }
        "jni-contract" => {
            ensure_no_extra_args(&rest, "jni-contract")?;
            jni_contract::run()
        }
        "loc" => loc::run(rest),
        "build-android" => android::build_android(rest),
        "build-android-libraw" => android::build_android_libraw(rest),
        "build-android-lensfun" => android::build_android_lensfun(rest),
        "verify-android-16kb" => android::verify_android_16kb(rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            print_help();
            Err(XtaskError::usage(format!("unknown command: {other}")))
        }
    }
}

pub(crate) fn print_help() {
    println!(
        "CalibRaw project-specific diagnostics and build helpers.\n\n\
         Usage: cargo xtask <command> [options]\n\n\
         Verification:\n\
           arch-check                       check crate dependency boundaries\n\
           jni-contract                     check Rust JNI exports and calls against Java\n\
           loc [--json PATH] [REPO...]      count production, test and build lines\n\
           baseline-run LABEL [--filter F]  capture UI review screenshots into\n\
                                            $CALIBRAW_BASELINE_DIR/<date>-<rev>-LABEL\n\
           baseline-compare BEFORE AFTER    compare two capture runs pixel by pixel\n\
           colorchecker-wb-validate CSV [--json PATH]\n\n\
         Build and packaging:\n\
           icons\n\
           build-android [ABI] [PROFILE]\n\
           build-android-libraw [ABI]\n\
           build-android-lensfun [ABI]\n\
           verify-android-16kb [APK] [--objdump PATH] [--zipalign PATH]"
    );
}
