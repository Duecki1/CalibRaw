//! The automatic runtime: the library a packager bundled, or else the pinned
//! ONNX Runtime package for this platform, downloaded into the model cache.
//! Where Microsoft builds one, the package carries the WebGPU plugin
//! execution provider beside the core library
//! (`packaging/onnxruntime/package_webgpu.py`).

use crate::config::Sha256;
use crate::registry::artifact::{ensure_artifact, sha256_file_hex, DownloadOptions, ModelArtifact};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
    time::Duration,
};

const INSTALL_MANIFEST: &str = "calibraw-runtime.txt";
/// The folder below the model folder that automatic runtimes are kept in.
const RUNTIME_CACHE_FOLDER: &str = "onnxruntime";

#[derive(Clone, Copy, Debug)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Clone, Copy, Debug)]
struct RuntimePackage {
    /// The folder it is installed in below the cache's `onnxruntime`, and
    /// the platform named in consent text.
    platform: &'static str,
    version: &'static str,
    /// Version of the WebGPU plugin beside the core library; `None` where
    /// Microsoft builds none, so AI runs on the CPU.
    webgpu: Option<&'static str>,
    archive_name: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
    format: ArchiveFormat,
}

/// The automatic runtime of each platform, by `std::env::consts` OS and
/// architecture. The WebGPU packages hold unmodified Microsoft libraries:
/// ONNX Runtime 1.30.0 and `onnxruntime-ep-webgpu` 0.4.0, which needs 1.24.4
/// or newer. Their install folders differ from the CPU, CUDA and DirectML
/// packages they replaced, which `RETIRED_RUNTIME_FILES` lists.
const PACKAGES: [(&str, &str, RuntimePackage); 6] = [
    (
        "linux",
        "x86_64",
        RuntimePackage {
            platform: "linux-x86_64-webgpu",
            version: "1.30.0",
            webgpu: Some("0.4.0"),
            archive_name: "onnxruntime-linux-x64-1.30.0-webgpu-0.4.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/30b40682c71039d9c063f0a54283f3182e027130/onnxruntime/linux-x86_64/onnxruntime-linux-x64-1.30.0-webgpu-0.4.0.tgz",
            bytes: 17_876_357,
            sha256: "4c0c8d379e04e5794db830130cb5c3c5e2ee01a6cb2136a9451e2a82f9073796",
            format: ArchiveFormat::TarGz,
        },
    ),
    (
        "linux",
        "aarch64",
        RuntimePackage {
            platform: "linux-arm64-webgpu",
            version: "1.30.0",
            webgpu: Some("0.4.0"),
            archive_name: "onnxruntime-linux-aarch64-1.30.0-webgpu-0.4.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/30b40682c71039d9c063f0a54283f3182e027130/onnxruntime/linux-arm64/onnxruntime-linux-aarch64-1.30.0-webgpu-0.4.0.tgz",
            bytes: 16_058_337,
            sha256: "8b04023808adac3123fd70722b4021b6446ae6eeeb36b34c5a263d6eb6da9855",
            format: ArchiveFormat::TarGz,
        },
    ),
    (
        "macos",
        "aarch64",
        RuntimePackage {
            platform: "macos-arm64-webgpu",
            version: "1.30.0",
            webgpu: Some("0.4.0"),
            archive_name: "onnxruntime-osx-arm64-1.30.0-webgpu-0.4.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/30b40682c71039d9c063f0a54283f3182e027130/onnxruntime/macos-arm64/onnxruntime-osx-arm64-1.30.0-webgpu-0.4.0.tgz",
            bytes: 17_645_295,
            sha256: "7f933179aa2e7bcd233a57f26cd543c83268764900d243ff320cd598188928ff",
            format: ArchiveFormat::TarGz,
        },
    ),
    // Microsoft's last macOS x86_64 build is 1.23.2, older than the WebGPU
    // plugin supports, and the plugin has no x86_64 build.
    (
        "macos",
        "x86_64",
        RuntimePackage {
            platform: "macos-x86_64",
            version: "1.23.2",
            webgpu: None,
            archive_name: "onnxruntime-osx-x86_64-1.23.2.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/7a00e95bdf48c64d354b99b1385cfe4ccf4cecbb/onnxruntime/macos-x86_64/onnxruntime-osx-x86_64-1.23.2.tgz",
            bytes: 11_676_322,
            sha256: "d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6",
            format: ArchiveFormat::TarGz,
        },
    ),
    // dxcompiler.dll and dxil.dll ship beside the plugin, where Dawn's
    // Direct3D 12 backend loads them from.
    (
        "windows",
        "x86_64",
        RuntimePackage {
            platform: "windows-x86_64-webgpu",
            version: "1.30.0",
            webgpu: Some("0.4.0"),
            archive_name: "onnxruntime-win-x64-1.30.0-webgpu-0.4.0.zip",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/30b40682c71039d9c063f0a54283f3182e027130/onnxruntime/windows-x86_64/onnxruntime-win-x64-1.30.0-webgpu-0.4.0.zip",
            bytes: 19_786_869,
            sha256: "3775b32da38429479e5f52a27ae56f45f33ab28889209cf233ec6f9db7495c2f",
            format: ArchiveFormat::Zip,
        },
    ),
    (
        "windows",
        "aarch64",
        RuntimePackage {
            platform: "windows-arm64-webgpu",
            version: "1.30.0",
            webgpu: Some("0.4.0"),
            archive_name: "onnxruntime-win-arm64-1.30.0-webgpu-0.4.0.zip",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/30b40682c71039d9c063f0a54283f3182e027130/onnxruntime/windows-arm64/onnxruntime-win-arm64-1.30.0-webgpu-0.4.0.zip",
            bytes: 20_912_627,
            sha256: "cf3d088a0cf97c56432deaaeaf37316cb877e502505bbeede6d3e6bc4dd14538",
            format: ArchiveFormat::Zip,
        },
    ),
];

/// Automatic runtimes CalibRaw downloaded before the WebGPU packages, below
/// the cache's `onnxruntime` folder: the CPU packages those replaced, the
/// Linux CUDA 12 and 13 packages (up to 440 MB each) and the Windows
/// DirectML ones, as install folders and their archives. Nothing loads them
/// any more.
///
/// migration: remove in v1.5.0, with `retired_runtime_paths`. Listed in
/// `calibraw_core`'s `migrations`.
const RETIRED_RUNTIME_FILES: &[&str] = &[
    "linux-x86_64",
    "onnxruntime-linux-x64-1.30.0.tgz",
    "linux-x86_64-cuda12",
    "onnxruntime-linux-x64-gpu_cuda12-1.30.0.tgz",
    "linux-x86_64-cuda13",
    "onnxruntime-linux-x64-gpu_cuda13-1.30.0.tgz",
    "linux-arm64",
    "onnxruntime-linux-aarch64-1.30.0.tgz",
    "macos-arm64",
    "onnxruntime-osx-arm64-1.30.0.tgz",
    "windows-x86_64",
    "onnxruntime-win-x64-directml-1.24.4.zip",
    "windows-arm64",
    "onnxruntime-win-arm64-directml-1.24.4.zip",
];

/// The install folders and archives of retired automatic runtimes
/// (`RETIRED_RUNTIME_FILES`) in the model folder `model_root`, for start-up
/// to delete.
///
/// migration: remove in v1.5.0. Listed in `calibraw_core`'s `migrations`.
pub fn retired_runtime_paths(model_root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    RETIRED_RUNTIME_FILES
        .iter()
        .map(move |name| model_root.join(RUNTIME_CACHE_FOLDER).join(name))
}

/// The folder automatic runtimes are installed in.
fn runtime_cache_dir() -> PathBuf {
    crate::config::model_root().join(RUNTIME_CACHE_FOLDER)
}

fn runtime_package() -> Result<RuntimePackage> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    PACKAGES
        .iter()
        .find(|(package_os, package_arch, _)| *package_os == os && *package_arch == arch)
        .map(|&(_, _, package)| package)
        .with_context(|| {
            format!(
                "automatic ONNX Runtime is unavailable for {os}/{arch}; select a compatible runtime manually in Settings"
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomaticOnnxRuntimeInfo {
    pub platform: &'static str,
    pub version: &'static str,
    pub download_bytes: u64,
    /// `None` for a bundled runtime.
    pub download_url: Option<&'static str>,
}

/// The runtime included by the packager, fixed when CalibRaw is compiled.
/// This remains `Some` if the file is missing so automatic mode cannot silently
/// replace a broken bundled installation with a download.
pub fn bundled_onnx_runtime_path() -> Option<&'static Path> {
    option_env!("CALIBRAW_BUNDLED_ONNX_RUNTIME").map(Path::new)
}

enum AutomaticRuntime<'a> {
    Bundled {
        path: &'a Path,
        version: &'static str,
    },
    Download(RuntimePackage),
}

fn automatic_runtime<'a>(
    bundled_path: Option<&'a Path>,
    bundled_version: Option<&'static str>,
) -> Result<AutomaticRuntime<'a>> {
    if let Some(path) = bundled_path {
        return Ok(AutomaticRuntime::Bundled {
            path,
            version: bundled_version.unwrap_or("unknown"),
        });
    }
    Ok(AutomaticRuntime::Download(runtime_package()?))
}

fn configured_automatic_runtime() -> Result<AutomaticRuntime<'static>> {
    automatic_runtime(
        bundled_onnx_runtime_path(),
        option_env!("CALIBRAW_BUNDLED_ONNX_RUNTIME_VERSION"),
    )
}

impl AutomaticRuntime<'_> {
    fn info(&self) -> AutomaticOnnxRuntimeInfo {
        match self {
            Self::Bundled { version, .. } => AutomaticOnnxRuntimeInfo {
                platform: std::env::consts::OS,
                version,
                download_bytes: 0,
                download_url: None,
            },
            Self::Download(package) => AutomaticOnnxRuntimeInfo {
                platform: package.platform,
                version: package.version,
                download_bytes: package.bytes,
                download_url: Some(package.url),
            },
        }
    }

    fn is_installed(&self) -> bool {
        let package = match self {
            Self::Bundled { path, .. } => return path.is_file(),
            Self::Download(package) => package,
        };
        let install_dir = runtime_cache_dir().join(package.platform);
        matches!(load_verified_install(&install_dir, package), Ok(Some(_)))
    }
}

pub fn automatic_onnx_runtime_info() -> Option<AutomaticOnnxRuntimeInfo> {
    Some(configured_automatic_runtime().ok()?.info())
}

pub fn automatic_onnx_runtime_is_installed() -> bool {
    configured_automatic_runtime().is_ok_and(|runtime| runtime.is_installed())
}

/// Whether Automatic mode's runtime carries the WebGPU provider. Packagers
/// bundle CPU builds.
fn automatic_onnx_runtime_has_gpu_provider() -> bool {
    matches!(
        configured_automatic_runtime(),
        Ok(AutomaticRuntime::Download(RuntimePackage {
            webgpu: Some(_),
            ..
        }))
    )
}

/// Whether Automatic mode downloads a runtime without a GPU build for this
/// platform (macOS x86_64), so sessions run on the CPU for that reason.
pub(crate) fn automatic_runtime_lacks_gpu_build() -> bool {
    matches!(
        configured_automatic_runtime(),
        Ok(AutomaticRuntime::Download(RuntimePackage {
            webgpu: None,
            ..
        }))
    )
}

/// Whether new AI sessions may run on a GPU: GPU AI is enabled and the runtime
/// may include a GPU execution provider. A manually selected runtime is not
/// known before it loads, so it may.
pub fn ai_sessions_may_use_gpu(manual_runtime: bool) -> bool {
    crate::placement::ai_acceleration_enabled()
        && (manual_runtime || automatic_onnx_runtime_has_gpu_provider())
}

fn install_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn runtime_download_options() -> DownloadOptions {
    DownloadOptions {
        connect_timeout: Duration::from_secs(45),
        response_timeout: Duration::from_secs(60),
        body_timeout: Duration::from_secs(30 * 60),
        attempts: 5,
        resume: true,
    }
}

/// The libraries of an installed automatic runtime, each with the SHA-256 it
/// had when it was installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RuntimeLibraries {
    pub(crate) core: (PathBuf, Sha256),
    /// The WebGPU plugin execution provider and its version, where the
    /// package has one.
    pub(crate) webgpu: Option<WebGpuPlugin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WebGpuPlugin {
    pub(crate) path: PathBuf,
    pub(crate) sha256: Sha256,
    pub(crate) version: &'static str,
}

pub(crate) fn ensure_automatic_onnx_runtime() -> Result<RuntimeLibraries> {
    ensure_automatic_runtime(configured_automatic_runtime()?)
}

fn ensure_automatic_runtime(runtime: AutomaticRuntime<'_>) -> Result<RuntimeLibraries> {
    let _guard = install_lock();
    let package = match runtime {
        AutomaticRuntime::Bundled { path, .. } => {
            anyhow::ensure!(
                path.is_file(),
                "bundled ONNX Runtime library is missing or is not a regular file: {}; repair the CalibRaw installation or select a trusted runtime manually in Settings",
                path.display()
            );
            // Flatpak strips libraries after Cargo builds the application. Hash
            // the installed bytes here and retain the normal pre-load checks.
            let sha256 = Sha256::of_file(path)
                .with_context(|| format!("hash bundled ONNX Runtime {}", path.display()))?;
            return Ok(RuntimeLibraries {
                core: (path.to_path_buf(), sha256),
                webgpu: None,
            });
        }
        AutomaticRuntime::Download(package) => package,
    };
    let root = runtime_cache_dir();
    let install_dir = root.join(package.platform);
    if let Some(runtime) = load_verified_install(&install_dir, &package)? {
        return Ok(runtime);
    }

    fs::create_dir_all(&root)
        .with_context(|| format!("create ONNX Runtime cache {}", root.display()))?;
    let archive_path = root.join(package.archive_name);
    let artifact = ModelArtifact {
        name: "CalibRaw automatic ONNX Runtime",
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
    .with_context(|| format!("download automatic ONNX Runtime for {}", package.platform))?;

    install_archive(&archive_path, &package, &install_dir)?;
    load_verified_install(&install_dir, &package)?
        .context("automatic ONNX Runtime install is invalid")
}

/// The manifest written beside an install: the archive it came from, then
/// each library's SHA-256 and path relative to the install folder, the WebGPU
/// plugin's only where the package has one.
fn load_verified_install(
    install_dir: &Path,
    package: &RuntimePackage,
) -> Result<Option<RuntimeLibraries>> {
    let manifest = match fs::read_to_string(install_dir.join(INSTALL_MANIFEST)) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read automatic ONNX Runtime manifest"),
    };
    let mut lines = manifest.lines();
    if lines
        .next()
        .and_then(|line| line.strip_prefix("archive_sha256="))
        != Some(package.sha256)
    {
        return Ok(None);
    }
    let Some(core) = verified_library(install_dir, &mut lines, "")? else {
        return Ok(None);
    };
    let webgpu = match package.webgpu {
        Some(version) => match verified_library(install_dir, &mut lines, "webgpu_")? {
            Some((path, sha256)) => Some(WebGpuPlugin {
                path,
                sha256,
                version,
            }),
            None => return Ok(None),
        },
        None => None,
    };
    if lines.next().is_some() {
        return Ok(None);
    }
    Ok(Some(RuntimeLibraries { core, webgpu }))
}

/// Reads a `{prefix}sha256=` and `{prefix}path=` line pair and checks the
/// library still has that hash.
fn verified_library<'a>(
    install_dir: &Path,
    lines: &mut impl Iterator<Item = &'a str>,
    prefix: &str,
) -> Result<Option<(PathBuf, Sha256)>> {
    let mut field = |name: &str| {
        lines
            .next()
            .and_then(|line| line.strip_prefix(prefix))
            .and_then(|line| line.strip_prefix(name))
    };
    let (Some(sha256), Some(relative)) = (field("sha256="), field("path=")) else {
        return Ok(None);
    };
    let Ok(sha256) = Sha256::parse(sha256) else {
        return Ok(None);
    };
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Ok(None);
    }
    let path = install_dir.join(relative);
    if !path.is_file() || Sha256::of_file(&path)? != sha256 {
        return Ok(None);
    }
    Ok(Some((path, sha256)))
}

fn install_archive(
    archive_path: &Path,
    package: &RuntimePackage,
    install_dir: &Path,
) -> Result<()> {
    let temporary = install_dir.with_extension(format!("installing.{}", std::process::id()));
    if temporary.exists() {
        fs::remove_dir_all(&temporary)
            .with_context(|| format!("remove stale runtime install {}", temporary.display()))?;
    }
    fs::create_dir_all(&temporary)
        .with_context(|| format!("create runtime install {}", temporary.display()))?;
    let result = (|| {
        match package.format {
            ArchiveFormat::TarGz => {
                let file = File::open(archive_path)
                    .with_context(|| format!("open {}", archive_path.display()))?;
                let mut archive = tar::Archive::new(GzDecoder::new(file));
                archive
                    .unpack(&temporary)
                    .with_context(|| format!("extract {}", archive_path.display()))?;
            }
            ArchiveFormat::Zip => {
                let file = File::open(archive_path)
                    .with_context(|| format!("open {}", archive_path.display()))?;
                let mut archive = zip::ZipArchive::new(file)
                    .with_context(|| format!("read {}", archive_path.display()))?;
                archive
                    .extract(&temporary)
                    .with_context(|| format!("extract {}", archive_path.display()))?;
            }
        }
        let mut manifest = format!("archive_sha256={}\n", package.sha256);
        manifest_entry(
            &temporary,
            &find_runtime_library(&temporary)?,
            "",
            &mut manifest,
        )?;
        if package.webgpu.is_some() {
            let plugin = find_webgpu_plugin(&temporary)?;
            manifest_entry(&temporary, &plugin, "webgpu_", &mut manifest)?;
        }
        fs::write(temporary.join(INSTALL_MANIFEST), manifest)
            .context("write automatic ONNX Runtime manifest")?;
        if install_dir.exists() {
            fs::remove_dir_all(install_dir).with_context(|| {
                format!("replace invalid runtime install {}", install_dir.display())
            })?;
        }
        fs::rename(&temporary, install_dir).with_context(|| {
            format!(
                "publish automatic runtime install {}",
                install_dir.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

/// Appends the manifest lines for `library`, below `root`.
fn manifest_entry(root: &Path, library: &Path, prefix: &str, manifest: &mut String) -> Result<()> {
    let sha256 = sha256_file_hex(library)?;
    let relative = library
        .strip_prefix(root)
        .context("automatic runtime escaped its install directory")?;
    let relative = relative
        .to_str()
        .context("automatic runtime path is not valid UTF-8")?;
    anyhow::ensure!(
        !relative.contains(['\n', '\r']),
        "automatic runtime path contains a line break"
    );
    manifest.push_str(&format!(
        "{prefix}sha256={sha256}\n{prefix}path={relative}\n"
    ));
    Ok(())
}

/// Every regular file below `root`, by lowercase file name.
fn files_below(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("inspect runtime directory {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                directories.push(path);
            } else if path.is_file() {
                files.push((
                    entry.file_name().to_string_lossy().to_ascii_lowercase(),
                    path,
                ));
            }
        }
    }
    Ok(files)
}

fn find_runtime_library(root: &Path) -> Result<PathBuf> {
    let mut candidates: Vec<(bool, PathBuf)> = files_below(root)?
        .into_iter()
        .filter_map(|(name, path)| {
            let exact = matches!(
                name.as_str(),
                "onnxruntime.dll" | "libonnxruntime.so" | "libonnxruntime.dylib"
            );
            let versioned = name.starts_with("libonnxruntime.so.")
                || (name.starts_with("libonnxruntime.") && name.ends_with(".dylib"));
            (exact || versioned).then_some((!exact, path))
        })
        .collect();
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .context("downloaded archive contains no ONNX Runtime shared library")
}

fn find_webgpu_plugin(root: &Path) -> Result<PathBuf> {
    files_below(root)?
        .into_iter()
        .find(|(name, _)| {
            matches!(
                name.as_str(),
                "onnxruntime_providers_webgpu.dll"
                    | "libonnxruntime_providers_webgpu.so"
                    | "libonnxruntime_providers_webgpu.dylib"
            )
        })
        .map(|(_, path)| path)
        .context("downloaded archive contains no WebGPU execution provider")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_runtime_metadata_matches_build_configuration() {
        let info = automatic_onnx_runtime_info().unwrap();
        if let Some(path) = option_env!("CALIBRAW_BUNDLED_ONNX_RUNTIME") {
            assert_eq!(bundled_onnx_runtime_path(), Some(Path::new(path)));
            assert_eq!(
                info.version,
                option_env!("CALIBRAW_BUNDLED_ONNX_RUNTIME_VERSION").unwrap_or("unknown")
            );
            assert_eq!(info.download_bytes, 0);
            assert_eq!(
                automatic_onnx_runtime_is_installed(),
                Path::new(path).is_file()
            );
            if !Path::new(path).is_file() {
                assert!(ensure_automatic_onnx_runtime()
                    .unwrap_err()
                    .to_string()
                    .contains("bundled ONNX Runtime library is missing"));
            }
        } else {
            assert!(bundled_onnx_runtime_path().is_none());
            let package = runtime_package().unwrap();
            assert_eq!(info.version, package.version);
            assert_eq!(info.download_bytes, package.bytes);
        }
    }

    #[test]
    fn bundled_runtime_hashes_the_installed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("libonnxruntime.so");
        fs::write(&library, b"runtime before packaging").unwrap();
        let resolve = || automatic_runtime(Some(&library), Some("1.30.0")).unwrap();
        assert!(resolve().is_installed());
        let installed = ensure_automatic_runtime(resolve()).unwrap();
        let (path, original_hash) = installed.core;
        assert_eq!(path, library);
        assert_eq!(original_hash.as_str(), sha256_file_hex(&library).unwrap());
        assert!(installed.webgpu.is_none());

        // Model flatpak-builder changing the library after Cargo has built it.
        fs::write(&library, b"stripped runtime").unwrap();
        let (path, installed_hash) = ensure_automatic_runtime(resolve()).unwrap().core;
        assert_eq!(path, library);
        assert_eq!(installed_hash.as_str(), sha256_file_hex(&library).unwrap());
        assert_ne!(installed_hash, original_hash);
    }

    #[test]
    fn missing_bundled_runtime_fails_without_falling_back_to_download() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("missing-libonnxruntime.so");
        let runtime = automatic_runtime(Some(&library), Some("bundled-test-version")).unwrap();
        assert_eq!(runtime.info().version, "bundled-test-version");
        assert_eq!(runtime.info().download_bytes, 0);
        assert!(!runtime.is_installed());
        let error = ensure_automatic_runtime(runtime).unwrap_err().to_string();
        assert!(error.contains("bundled ONNX Runtime library is missing"));
        assert!(error.contains(library.to_str().unwrap()));
        assert!(error.contains("repair the CalibRaw installation"));
    }

    #[test]
    fn bundled_runtime_rejects_a_directory() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = automatic_runtime(Some(directory.path()), Some("1.30.0")).unwrap();
        assert!(!runtime.is_installed());
        assert!(ensure_automatic_runtime(runtime)
            .unwrap_err()
            .to_string()
            .contains("not a regular file"));
    }

    #[test]
    fn desktop_without_a_bundle_keeps_the_pinned_download_package() {
        let runtime = automatic_runtime(None, Some("unused-bundled-version")).unwrap();
        let package = runtime_package().unwrap();
        assert!(matches!(runtime, AutomaticRuntime::Download(_)));
        assert_eq!(
            runtime.info(),
            AutomaticOnnxRuntimeInfo {
                platform: package.platform,
                version: package.version,
                download_bytes: package.bytes,
                download_url: Some(package.url),
            }
        );
        assert!(runtime.info().download_bytes > 0);
    }

    #[test]
    fn every_package_downloads_from_a_pinned_commit() {
        for (os, arch, package) in PACKAGES {
            let revision = package
                .url
                .split("/resolve/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap();
            assert!(
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{os}/{arch} downloads from mutable revision {revision}"
            );
            assert!(package.url.ends_with(package.archive_name), "{os}/{arch}");
        }
    }

    /// The packages' install folders and archives are not among the retired
    /// files, which start-up deletes, and each package has its own.
    #[test]
    fn current_packages_are_not_retired() {
        for (os, arch, package) in PACKAGES {
            for name in [package.platform, package.archive_name] {
                assert!(
                    !RETIRED_RUNTIME_FILES.contains(&name),
                    "{os}/{arch}: {name}"
                );
            }
            for (_, _, other) in PACKAGES {
                if other.sha256 != package.sha256 {
                    assert_ne!(other.platform, package.platform);
                    assert_ne!(other.archive_name, package.archive_name);
                }
            }
        }
        for name in RETIRED_RUNTIME_FILES {
            assert!(!name.is_empty() && !name.contains(['/', '\\']), "{name}");
        }
    }

    /// Only the macOS x86_64 package lacks the WebGPU provider; every other
    /// one has a core new enough for the plugin.
    #[test]
    fn webgpu_packages_carry_a_core_the_plugin_supports() {
        for (os, arch, package) in PACKAGES {
            assert_eq!(
                package.webgpu.is_none(),
                (os, arch) == ("macos", "x86_64"),
                "{os}/{arch}"
            );
            if package.webgpu.is_some() {
                assert_eq!(package.version, "1.30.0", "{os}/{arch}");
                assert!(package.platform.ends_with("-webgpu"), "{os}/{arch}");
            }
        }
    }

    fn webgpu_package() -> RuntimePackage {
        RuntimePackage {
            webgpu: Some("0.4.0"),
            ..PACKAGES[0].2
        }
    }

    fn write_install(directory: &Path, archive_sha256: &str, plugin: bool) {
        fs::create_dir_all(directory.join("lib")).unwrap();
        fs::write(directory.join("lib/libonnxruntime.so.1.30.0"), b"core").unwrap();
        let mut manifest = format!("archive_sha256={archive_sha256}\n");
        manifest_entry(
            directory,
            &directory.join("lib/libonnxruntime.so.1.30.0"),
            "",
            &mut manifest,
        )
        .unwrap();
        if plugin {
            let path = directory.join("lib/libonnxruntime_providers_webgpu.so");
            fs::write(&path, b"plugin").unwrap();
            manifest_entry(directory, &path, "webgpu_", &mut manifest).unwrap();
        }
        fs::write(directory.join(INSTALL_MANIFEST), manifest).unwrap();
    }

    #[test]
    fn runtime_upgrade_invalidates_the_previous_install() {
        let directory = tempfile::tempdir().unwrap();
        let package = webgpu_package();
        write_install(directory.path(), "old-release", true);
        assert!(load_verified_install(
            directory.path(),
            &RuntimePackage {
                sha256: "old-release",
                ..package
            }
        )
        .unwrap()
        .is_some());
        assert!(load_verified_install(directory.path(), &package)
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_webgpu_install_verifies_both_libraries() {
        let directory = tempfile::tempdir().unwrap();
        let package = webgpu_package();
        write_install(directory.path(), package.sha256, true);
        let installed = load_verified_install(directory.path(), &package)
            .unwrap()
            .unwrap();
        assert_eq!(
            installed.core.0,
            directory.path().join("lib/libonnxruntime.so.1.30.0")
        );
        let plugin = installed.webgpu.unwrap();
        assert_eq!(
            plugin.path,
            directory
                .path()
                .join("lib/libonnxruntime_providers_webgpu.so")
        );
        assert_eq!(plugin.version, "0.4.0");

        // A changed plugin invalidates the install, so it is extracted again.
        fs::write(&plugin.path, b"changed plugin").unwrap();
        assert!(load_verified_install(directory.path(), &package)
            .unwrap()
            .is_none());

        // So does a manifest without the plugin for a package that has one.
        write_install(directory.path(), package.sha256, false);
        assert!(load_verified_install(directory.path(), &package)
            .unwrap()
            .is_none());
        let cpu_package = RuntimePackage {
            webgpu: None,
            ..package
        };
        let installed = load_verified_install(directory.path(), &cpu_package)
            .unwrap()
            .unwrap();
        assert!(installed.webgpu.is_none());
    }

    #[test]
    fn archives_install_the_core_and_the_plugin() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("runtime.tgz");
        let root = "onnxruntime-linux-x64-1.30.0-webgpu-0.4.0";
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gz);
            for (name, data) in [
                ("lib/libonnxruntime.so.1.30.0", &b"core"[..]),
                ("lib/libonnxruntime_providers_webgpu.so", &b"plugin"[..]),
                ("LICENSE", &b"license"[..]),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, format!("{root}/{name}"), data)
                    .unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        let package = RuntimePackage {
            sha256: "test-archive",
            format: ArchiveFormat::TarGz,
            ..webgpu_package()
        };
        let install_dir = directory.path().join(package.platform);
        install_archive(&archive, &package, &install_dir).unwrap();
        let installed = load_verified_install(&install_dir, &package)
            .unwrap()
            .unwrap();
        assert_eq!(
            installed.core.0,
            install_dir.join(root).join("lib/libonnxruntime.so.1.30.0")
        );
        assert_eq!(
            installed.webgpu.unwrap().path,
            install_dir
                .join(root)
                .join("lib/libonnxruntime_providers_webgpu.so")
        );
    }

    #[test]
    #[ignore = "requires a network connection and writes the desktop runtime cache"]
    fn automatic_runtime_downloads_extracts_and_initializes() {
        crate::runtime::initialize_runtime().unwrap();
    }
}
