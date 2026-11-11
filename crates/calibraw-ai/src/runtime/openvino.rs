//! OpenVINO on Intel CPUs. Where the CPU is Intel's and Settings allow it,
//! Automatic mode downloads Intel's OpenVINO plugin execution provider, cut
//! down to its CPU device (`packaging/onnxruntime/package_openvino.py`), as a
//! package of its own beside the automatic runtime, and `load` registers it.
//! Sessions that would run on ONNX Runtime's CPU provider then try OpenVINO
//! first (`placement`); GPU providers still come before both. Any failure,
//! from the download to a run, leaves them on ONNX Runtime's CPU provider.

use super::package::{
    files_below, install_archive, install_lock, read_verified_manifest, runtime_cache_dir,
    runtime_download_options, ArchiveFormat, PluginLibrary,
};
use super::OpenVinoPackageInfo;
use crate::config::RuntimeSource;
use crate::registry::artifact::{ensure_artifact, ModelArtifact};
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Copy, Debug)]
struct OpenVinoPackage {
    /// The folder it is installed in below the cache's `onnxruntime`.
    install_folder: &'static str,
    plugin_version: &'static str,
    openvino_version: &'static str,
    archive_name: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
    format: ArchiveFormat,
}

/// The package of each platform Intel builds the plugin for, by
/// `std::env::consts` OS; x86_64 only. Plugin 1.8.1 needs ONNX Runtime 1.23
/// or newer, and the automatic runtime of both is 1.30.0.
const PACKAGES: [(&str, OpenVinoPackage); 2] = [
    (
        "linux",
        OpenVinoPackage {
            install_folder: "openvino-ep-1.8.1-cpu-linux-x86_64",
            plugin_version: "1.8.1",
            openvino_version: "2026.4.1",
            archive_name: "openvino-ep-1.8.1-cpu-linux-x86_64.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/onnxruntime/openvino/linux-x86_64/openvino-ep-1.8.1-cpu-linux-x86_64.tgz",
            bytes: 28_675_357,
            sha256: "a9939b31b72828e85038b63093382eef3c5f77ac028dca37ca8d1219ee0fa9c8",
            format: ArchiveFormat::TarGz,
        },
    ),
    (
        "windows",
        OpenVinoPackage {
            install_folder: "openvino-ep-1.8.1-cpu-windows-x86_64",
            plugin_version: "1.8.1",
            openvino_version: "2026.4.1",
            archive_name: "openvino-ep-1.8.1-cpu-windows-x86_64.zip",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/main/onnxruntime/openvino/windows-x86_64/openvino-ep-1.8.1-cpu-windows-x86_64.zip",
            bytes: 22_437_176,
            sha256: "9acd9623e66481a6ee6d8c63ad4bee6b9427accf7df18a94d9beb83c9db5356e",
            format: ArchiveFormat::Zip,
        },
    ),
];

/// This machine's package where the CPU is Intel's, fixed for the process.
/// The plugin offers devices for Intel hardware only.
fn machine_package() -> Option<&'static OpenVinoPackage> {
    static PACKAGE: OnceLock<Option<&'static OpenVinoPackage>> = OnceLock::new();
    *PACKAGE.get_or_init(|| {
        let package = PACKAGES
            .iter()
            .find(|(os, _)| *os == std::env::consts::OS && std::env::consts::ARCH == "x86_64")
            .map(|(_, package)| package);
        let intel = intel_cpu();
        let message = match (package, intel) {
            (Some(package), true) => {
                format!("OpenVINO {} supported: Intel CPU", package.openvino_version)
            }
            (Some(_), false) => "OpenVINO not supported: the CPU is not Intel's".to_owned(),
            (None, _) => format!(
                "OpenVINO not supported: no OpenVINO package for {}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        };
        calibraw_core::diagnostics::record(message);
        package.filter(|_| intel)
    })
}

/// Whether CPUID names Intel as the CPU's vendor.
#[cfg(target_arch = "x86_64")]
fn intel_cpu() -> bool {
    // Leaf 0 returns the vendor string in EBX, EDX and ECX.
    // SAFETY: every x86_64 CPU has CPUID, and leaf 0 exists on all of them.
    let leaf = unsafe { std::arch::x86_64::__cpuid(0) };
    let mut vendor = [0; 12];
    for (chunk, register) in vendor
        .chunks_exact_mut(4)
        .zip([leaf.ebx, leaf.edx, leaf.ecx])
    {
        chunk.copy_from_slice(&register.to_le_bytes());
    }
    &vendor == b"GenuineIntel"
}

#[cfg(not(target_arch = "x86_64"))]
fn intel_cpu() -> bool {
    false
}

/// Whether this machine can use OpenVINO: an Intel CPU on a platform Intel
/// builds the plugin for. Settings offer the choice only here.
pub fn openvino_supported() -> bool {
    machine_package().is_some()
}

/// The package when OpenVINO applies now: the machine supports it, Settings
/// allow it, and AI loads the downloaded automatic runtime, whose version
/// the plugin was checked with. A bundled or manually selected runtime runs
/// without it.
fn applicable_package() -> Option<&'static OpenVinoPackage> {
    let package = machine_package()?;
    (crate::placement::openvino_enabled()
        && crate::config::runtime_source() == RuntimeSource::Automatic
        && super::package::bundled_onnx_runtime_path().is_none())
    .then_some(package)
}

/// The OpenVINO download, when OpenVINO applies.
pub fn openvino_package_info() -> Option<OpenVinoPackageInfo> {
    applicable_package().map(|package| OpenVinoPackageInfo {
        plugin_version: package.plugin_version,
        openvino_version: package.openvino_version,
        download_bytes: package.bytes,
        download_url: package.url,
    })
}

fn install_dir(package: &OpenVinoPackage) -> PathBuf {
    runtime_cache_dir().join(package.install_folder)
}

fn verified_install(package: &OpenVinoPackage) -> Result<Option<PluginLibrary>> {
    let libraries = read_verified_manifest(&install_dir(package), package.sha256, &[""])?;
    Ok(libraries
        .and_then(|libraries| libraries.into_iter().next())
        .map(|(path, sha256)| PluginLibrary {
            path,
            sha256,
            version: package.plugin_version,
        }))
}

/// Whether the OpenVINO package is installed, or OpenVINO does not apply.
pub fn openvino_package_is_installed() -> bool {
    applicable_package().is_none_or(|package| matches!(verified_install(package), Ok(Some(_))))
}

/// The installed plugin when OpenVINO applies, downloading and installing
/// its package first if needed; `None` when it does not apply.
pub(crate) fn ensure_openvino_plugin() -> Result<Option<PluginLibrary>> {
    let Some(package) = applicable_package() else {
        return Ok(None);
    };
    let _guard = install_lock();
    if let Some(plugin) = verified_install(package)? {
        return Ok(Some(plugin));
    }
    let root = runtime_cache_dir();
    std::fs::create_dir_all(&root)
        .with_context(|| format!("create ONNX Runtime cache {}", root.display()))?;
    let archive_path = root.join(package.archive_name);
    let artifact = ModelArtifact {
        name: "CalibRaw OpenVINO execution provider",
        url: Some(package.url),
        sha256: package.sha256,
        bytes: package.bytes,
        cache_filename: package.archive_name,
    };
    ensure_artifact(
        &archive_path,
        artifact,
        runtime_download_options(),
        |_, _| {},
        || Ok(()),
    )
    .context("download the OpenVINO execution provider")?;
    install_archive(
        &archive_path,
        package.sha256,
        package.format,
        &install_dir(package),
        &[("", find_openvino_plugin)],
    )?;
    verified_install(package)?
        .context("the OpenVINO execution provider install is invalid")
        .map(Some)
}

fn find_openvino_plugin(root: &Path) -> Result<PathBuf> {
    files_below(root)?
        .into_iter()
        .find(|(name, _)| {
            matches!(
                name.as_str(),
                "onnxruntime_providers_openvino_plugin.dll"
                    | "libonnxruntime_providers_openvino_plugin.so"
            )
        })
        .map(|(_, path)| path)
        .context("downloaded archive contains no OpenVINO execution provider")
}
