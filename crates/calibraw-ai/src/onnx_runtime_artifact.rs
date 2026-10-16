use crate::model_artifact::{ensure_artifact, sha256_file_hex, DownloadOptions, ModelArtifact};
use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard, OnceLock},
    time::Duration,
};

const INSTALL_MANIFEST: &str = "calibraw-runtime.txt";

#[derive(Clone, Copy)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Clone, Copy)]
struct RuntimePackage {
    platform: &'static str,
    version: &'static str,
    archive_name: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
    format: ArchiveFormat,
    /// Built with a GPU execution provider (DirectML, CUDA or CoreML).
    gpu_provider: bool,
}

/// Which automatic runtime to download. Only Linux x86_64 has a choice: its
/// CPU package has no GPU provider, and Microsoft's CUDA builds need the
/// matching CUDA toolkit and cuDNN 9 installed on the system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomaticOnnxRuntimeKind {
    #[default]
    Cpu,
    Cuda12,
    Cuda13,
}

impl AutomaticOnnxRuntimeKind {
    pub const ALL: [Self; 3] = [Self::Cpu, Self::Cuda12, Self::Cuda13];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Cuda12 => "NVIDIA CUDA 12",
            Self::Cuda13 => "NVIDIA CUDA 13",
        }
    }

    /// Archive size of this kind's download, or `None` where this platform
    /// has no automatic runtime.
    pub fn download_bytes(self) -> Option<u64> {
        runtime_package(self).ok().map(|package| package.bytes)
    }
}

static AUTOMATIC_RUNTIME_KIND: Mutex<AutomaticOnnxRuntimeKind> =
    Mutex::new(AutomaticOnnxRuntimeKind::Cpu);

pub fn set_automatic_onnx_runtime_kind(kind: AutomaticOnnxRuntimeKind) {
    *AUTOMATIC_RUNTIME_KIND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = kind;
}

fn automatic_runtime_kind() -> AutomaticOnnxRuntimeKind {
    *AUTOMATIC_RUNTIME_KIND
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether Automatic mode offers the CUDA runtimes. Bundled runtimes, such as
/// the Flatpak's, are never replaced by a download.
pub fn automatic_onnx_runtime_kind_selectable() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64")) && bundled_onnx_runtime_path().is_none()
}

fn runtime_package(kind: AutomaticOnnxRuntimeKind) -> Result<RuntimePackage> {
    let package = match (std::env::consts::OS, std::env::consts::ARCH, kind) {
        // Unmodified copies of Microsoft's release assets.
        ("linux", "x86_64", AutomaticOnnxRuntimeKind::Cuda12) => RuntimePackage {
            platform: "linux-x86_64-cuda12",
            version: "1.30.0",
            archive_name: "onnxruntime-linux-x64-gpu_cuda12-1.30.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/4932f77083731a27014f944c85593bc30d3387e0/onnxruntime/linux-x86_64/onnxruntime-linux-x64-gpu_cuda12-1.30.0.tgz",
            bytes: 439_354_926,
            sha256: "f9886932ee7bb0b4d3fcab736a392d4ff5efaa0672b47f19f0cec03437cf64f1",
            format: ArchiveFormat::TarGz,
            gpu_provider: true,
        },
        ("linux", "x86_64", AutomaticOnnxRuntimeKind::Cuda13) => RuntimePackage {
            platform: "linux-x86_64-cuda13",
            version: "1.30.0",
            archive_name: "onnxruntime-linux-x64-gpu_cuda13-1.30.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/4932f77083731a27014f944c85593bc30d3387e0/onnxruntime/linux-x86_64/onnxruntime-linux-x64-gpu_cuda13-1.30.0.tgz",
            bytes: 235_697_517,
            sha256: "382d79133112388cf94ce5855789b7c9bef12bef76a08b6b277e5a317213adcd",
            format: ArchiveFormat::TarGz,
            gpu_provider: true,
        },
        ("linux", "x86_64", AutomaticOnnxRuntimeKind::Cpu) => RuntimePackage {
            platform: "linux-x86_64",
            version: "1.30.0",
            archive_name: "onnxruntime-linux-x64-1.30.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/7a00e95bdf48c64d354b99b1385cfe4ccf4cecbb/onnxruntime/linux-x86_64/onnxruntime-linux-x64-1.30.0.tgz",
            bytes: 11_306_877,
            sha256: "a5ed5a3cac51fbb2e90da632ae43d19212faaa20e76484e62bcb7c23ddb3b3fd",
            format: ArchiveFormat::TarGz,
            gpu_provider: false,
        },
        ("linux", "aarch64", _) => RuntimePackage {
            platform: "linux-arm64",
            version: "1.30.0",
            archive_name: "onnxruntime-linux-aarch64-1.30.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/7a00e95bdf48c64d354b99b1385cfe4ccf4cecbb/onnxruntime/linux-arm64/onnxruntime-linux-aarch64-1.30.0.tgz",
            bytes: 10_269_495,
            sha256: "e16a27a8ed330bbc698df7330b0cf56e722f354e3bcc92118682c74ef3c3e3da",
            format: ArchiveFormat::TarGz,
            gpu_provider: false,
        },
        ("macos", "aarch64", _) => RuntimePackage {
            platform: "macos-arm64",
            version: "1.30.0",
            archive_name: "onnxruntime-osx-arm64-1.30.0.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/7a00e95bdf48c64d354b99b1385cfe4ccf4cecbb/onnxruntime/macos-arm64/onnxruntime-osx-arm64-1.30.0.tgz",
            bytes: 42_373_116,
            sha256: "6ebb5062a934537c352937821f9fe9718e7de1a2db1122a93dd363ffd53a7012",
            format: ArchiveFormat::TarGz,
            gpu_provider: true,
        },
        ("macos", "x86_64", _) => RuntimePackage {
            platform: "macos-x86_64",
            version: "1.23.2",
            archive_name: "onnxruntime-osx-x86_64-1.23.2.tgz",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/7a00e95bdf48c64d354b99b1385cfe4ccf4cecbb/onnxruntime/macos-x86_64/onnxruntime-osx-x86_64-1.23.2.tgz",
            bytes: 11_676_322,
            sha256: "d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6",
            format: ArchiveFormat::TarGz,
            gpu_provider: true,
        },
        // 1.24.4 is Microsoft's final DirectML build. DirectML.dll ships beside
        // onnxruntime.dll, where its delay-load hook looks before System32.
        ("windows", "x86_64", _) => RuntimePackage {
            platform: "windows-x86_64",
            version: "1.24.4",
            archive_name: "onnxruntime-win-x64-directml-1.24.4.zip",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/4932f77083731a27014f944c85593bc30d3387e0/onnxruntime/windows-x86_64/onnxruntime-win-x64-directml-1.24.4.zip",
            bytes: 15_806_028,
            sha256: "23131eec611e9ccd5cc563679aaaf55f221d51615ff6bf5e245970a5f5218c38",
            format: ArchiveFormat::Zip,
            gpu_provider: true,
        },
        ("windows", "aarch64", _) => RuntimePackage {
            platform: "windows-arm64",
            version: "1.24.4",
            archive_name: "onnxruntime-win-arm64-directml-1.24.4.zip",
            url: "https://huggingface.co/Duecki/CalibRaw-Artifacts/resolve/4932f77083731a27014f944c85593bc30d3387e0/onnxruntime/windows-arm64/onnxruntime-win-arm64-directml-1.24.4.zip",
            bytes: 15_562_020,
            sha256: "27385338c565cf41c2c46e613ef603e7063a358a821176b1f916902d5afa93e3",
            format: ArchiveFormat::Zip,
            gpu_provider: true,
        },
        (os, arch, _) => anyhow::bail!(
            "automatic ONNX Runtime is unavailable for {os}/{arch}; select a compatible runtime manually in Settings"
        ),
    };
    Ok(package)
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
    kind: AutomaticOnnxRuntimeKind,
) -> Result<AutomaticRuntime<'a>> {
    if let Some(path) = bundled_path {
        return Ok(AutomaticRuntime::Bundled {
            path,
            version: bundled_version.unwrap_or("unknown"),
        });
    }
    Ok(AutomaticRuntime::Download(runtime_package(kind)?))
}

fn configured_automatic_runtime() -> Result<AutomaticRuntime<'static>> {
    automatic_runtime(
        bundled_onnx_runtime_path(),
        option_env!("CALIBRAW_BUNDLED_ONNX_RUNTIME_VERSION"),
        automatic_runtime_kind(),
    )
}

impl AutomaticRuntime<'_> {
    fn info(&self) -> AutomaticOnnxRuntimeInfo {
        match self {
            // Packagers bundle a CPU build; the selected kind only picks downloads.
            Self::Bundled { version, .. } => AutomaticOnnxRuntimeInfo {
                platform: runtime_package(AutomaticOnnxRuntimeKind::Cpu)
                    .map(|package| package.platform)
                    .unwrap_or(std::env::consts::OS),
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
        let install_dir = crate::desktop_model_cache_root()
            .join("onnxruntime")
            .join(package.platform);
        matches!(
            load_verified_install(&install_dir, package.sha256),
            Ok(Some(_))
        )
    }
}

pub fn automatic_onnx_runtime_info() -> Option<AutomaticOnnxRuntimeInfo> {
    Some(configured_automatic_runtime().ok()?.info())
}

pub fn automatic_onnx_runtime_is_installed() -> bool {
    configured_automatic_runtime().is_ok_and(|runtime| runtime.is_installed())
}

/// Whether Automatic mode's runtime is built with a GPU execution provider.
/// Packagers bundle CPU builds.
fn automatic_onnx_runtime_has_gpu_provider() -> bool {
    matches!(
        configured_automatic_runtime(),
        Ok(AutomaticRuntime::Download(RuntimePackage {
            gpu_provider: true,
            ..
        }))
    )
}

/// Whether new AI sessions may run on a GPU: GPU AI is enabled and the runtime
/// may include a GPU execution provider. A manually selected runtime is not
/// known before it loads, so it may.
pub fn ai_sessions_may_use_gpu(manual_runtime: bool) -> bool {
    crate::execution_provider::ai_acceleration_enabled()
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

pub fn ensure_automatic_onnx_runtime() -> Result<(PathBuf, String)> {
    ensure_automatic_runtime(configured_automatic_runtime()?)
}

fn ensure_automatic_runtime(runtime: AutomaticRuntime<'_>) -> Result<(PathBuf, String)> {
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
            let sha256 = sha256_file_hex(path)
                .with_context(|| format!("hash bundled ONNX Runtime {}", path.display()))?;
            return Ok((path.to_path_buf(), sha256));
        }
        AutomaticRuntime::Download(package) => package,
    };
    let root = crate::desktop_model_cache_root().join("onnxruntime");
    let install_dir = root.join(package.platform);
    if let Some(runtime) = load_verified_install(&install_dir, package.sha256)? {
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
    };
    ensure_artifact(
        &archive_path,
        artifact,
        runtime_download_options(),
        |_, _| {},
        || Ok(()),
    )
    .with_context(|| format!("download automatic ONNX Runtime for {}", package.platform))?;

    install_archive(&archive_path, package.format, package.sha256, &install_dir)?;
    load_verified_install(&install_dir, package.sha256)?
        .context("automatic ONNX Runtime install is invalid")
}

fn load_verified_install(
    install_dir: &Path,
    expected_archive_sha256: &str,
) -> Result<Option<(PathBuf, String)>> {
    let manifest = match fs::read_to_string(install_dir.join(INSTALL_MANIFEST)) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read automatic ONNX Runtime manifest"),
    };
    let mut lines = manifest.lines();
    let Some(archive_sha256) = lines
        .next()
        .and_then(|line| line.strip_prefix("archive_sha256="))
    else {
        return Ok(None);
    };
    let Some(sha256) = lines.next().and_then(|line| line.strip_prefix("sha256=")) else {
        return Ok(None);
    };
    let Some(relative) = lines.next().and_then(|line| line.strip_prefix("path=")) else {
        return Ok(None);
    };
    if archive_sha256 != expected_archive_sha256
        || lines.next().is_some()
        || sha256.len() != 64
        || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(None);
    }
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Ok(None);
    }
    let runtime_path = install_dir.join(relative);
    if !runtime_path.is_file() || sha256_file_hex(&runtime_path)? != sha256 {
        return Ok(None);
    }
    Ok(Some((runtime_path, sha256.to_owned())))
}

fn install_archive(
    archive_path: &Path,
    format: ArchiveFormat,
    archive_sha256: &str,
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
        match format {
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
        let runtime_path = find_runtime_library(&temporary)?;
        let sha256 = sha256_file_hex(&runtime_path)?;
        let relative = runtime_path
            .strip_prefix(&temporary)
            .context("automatic runtime escaped its install directory")?;
        let relative = relative
            .to_str()
            .context("automatic runtime path is not valid UTF-8")?;
        anyhow::ensure!(
            !relative.contains(['\n', '\r']),
            "automatic runtime path contains a line break"
        );
        fs::write(
            temporary.join(INSTALL_MANIFEST),
            format!("archive_sha256={archive_sha256}\nsha256={sha256}\npath={relative}\n"),
        )
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

fn find_runtime_library(root: &Path) -> Result<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let mut candidates = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)
            .with_context(|| format!("inspect runtime directory {}", directory.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                directories.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let exact = matches!(
                name.as_str(),
                "onnxruntime.dll" | "libonnxruntime.so" | "libonnxruntime.dylib"
            );
            let versioned = name.starts_with("libonnxruntime.so.")
                || (name.starts_with("libonnxruntime.") && name.ends_with(".dylib"));
            if path.is_file() && (exact || versioned) {
                candidates.push((!exact, path));
            }
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .next()
        .map(|(_, path)| path)
        .context("downloaded archive contains no ONNX Runtime shared library")
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
            assert_eq!(
                info.version,
                runtime_package(automatic_runtime_kind()).unwrap().version
            );
            assert_eq!(
                info.download_bytes,
                runtime_package(automatic_runtime_kind()).unwrap().bytes
            );
        }
    }

    #[test]
    fn bundled_runtime_hashes_the_installed_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("libonnxruntime.so");
        fs::write(&library, b"runtime before packaging").unwrap();
        let resolve = || {
            automatic_runtime(
                Some(&library),
                Some("1.30.0"),
                AutomaticOnnxRuntimeKind::Cpu,
            )
            .unwrap()
        };
        assert!(resolve().is_installed());
        let (path, original_hash) = ensure_automatic_runtime(resolve()).unwrap();
        assert_eq!(path, library);
        assert_eq!(original_hash, sha256_file_hex(&library).unwrap());

        // Model flatpak-builder changing the library after Cargo has built it.
        fs::write(&library, b"stripped runtime").unwrap();
        let (path, installed_hash) = ensure_automatic_runtime(resolve()).unwrap();
        assert_eq!(path, library);
        assert_eq!(installed_hash, sha256_file_hex(&library).unwrap());
        assert_ne!(installed_hash, original_hash);
    }

    #[test]
    fn missing_bundled_runtime_fails_without_falling_back_to_download() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("missing-libonnxruntime.so");
        let runtime = automatic_runtime(
            Some(&library),
            Some("bundled-test-version"),
            AutomaticOnnxRuntimeKind::Cpu,
        )
        .unwrap();
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
        let runtime = automatic_runtime(
            Some(directory.path()),
            Some("1.30.0"),
            AutomaticOnnxRuntimeKind::Cpu,
        )
        .unwrap();
        assert!(!runtime.is_installed());
        assert!(ensure_automatic_runtime(runtime)
            .unwrap_err()
            .to_string()
            .contains("not a regular file"));
    }

    #[test]
    fn desktop_without_a_bundle_keeps_the_pinned_download_package() {
        let runtime = automatic_runtime(
            None,
            Some("unused-bundled-version"),
            AutomaticOnnxRuntimeKind::Cpu,
        )
        .unwrap();
        let package = runtime_package(AutomaticOnnxRuntimeKind::Cpu).unwrap();
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
    fn every_runtime_kind_downloads_from_a_pinned_commit() {
        for kind in AutomaticOnnxRuntimeKind::ALL {
            let Ok(package) = runtime_package(kind) else {
                continue;
            };
            let revision = package
                .url
                .split("/resolve/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap();
            assert!(
                revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "{kind:?} downloads from mutable revision {revision}"
            );
            // Kinds that select different archives need separate install directories.
            let cpu = runtime_package(AutomaticOnnxRuntimeKind::Cpu).unwrap();
            assert_eq!(
                package.sha256 == cpu.sha256,
                package.platform == cpu.platform
            );
        }
    }

    #[test]
    fn runtime_upgrade_invalidates_the_previous_install() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("runtime-library");
        fs::write(&library, b"old runtime").unwrap();
        let hash = sha256_file_hex(&library).unwrap();
        fs::write(
            directory.path().join(INSTALL_MANIFEST),
            format!("archive_sha256=old-release\nsha256={hash}\npath=runtime-library\n"),
        )
        .unwrap();
        assert!(load_verified_install(directory.path(), "old-release")
            .unwrap()
            .is_some());
        assert!(load_verified_install(
            directory.path(),
            runtime_package(AutomaticOnnxRuntimeKind::Cpu)
                .unwrap()
                .sha256
        )
        .unwrap()
        .is_none());
    }

    #[test]
    #[ignore = "requires a network connection and writes the desktop runtime cache"]
    fn automatic_runtime_downloads_extracts_and_initializes() {
        crate::ai_masks::initialize_runtime(None, None).unwrap();
    }
}
