"""Check iOS native build routing without Xcode, Cargo, or downloads."""

import importlib.util
import os
from pathlib import Path
import shlex
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "build_ios.py"
SPEC = importlib.util.spec_from_file_location("build_ios", SCRIPT)
build_ios = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build_ios)


class ReachedCargo(Exception):
    """Stop before compilation after observing native and Rust setup."""


class IosBuildTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.target_dir = Path(temporary.name).resolve()
        self.target_libraries = self.target_dir / "rust-target-libraries"
        self.target_libraries.mkdir()
        (self.target_libraries / "libstd-test.rlib").touch()

    def configure_build(self, arguments, host="arm64"):
        calls = []

        def run(command, environment, capture=False):
            command = list(map(str, command))
            calls.append((command, environment.copy()))
            if command[:3] == ["rustc", "--print", "target-libdir"]:
                return str(self.target_libraries)
            if command[0] == "xcrun":
                if command[-1] == "--show-sdk-path":
                    return f"/SDKs/{command[2]}.sdk"
                if command[3] == "--find":
                    return f"/tools/{command[2]}/{command[4]}"
            if command[0] == "bash":
                return ""
            if command[:2] == ["cargo", "build"]:
                raise ReachedCargo
            self.fail(f"Unexpected command: {command}")

        environment = {
            "PATH": "/tools",
            "DEVELOPER_DIR": "/Applications/Xcode.app/Contents/Developer",
            "CARGO_TARGET_DIR": str(self.target_dir),
        }
        with (
            patch.object(build_ios.sys, "argv", [str(SCRIPT), *arguments]),
            patch.object(build_ios.sys, "platform", "darwin"),
            patch.object(build_ios.platform, "machine", return_value=host),
            patch.object(build_ios.shutil, "which", return_value="/tools/tool"),
            patch.dict(os.environ, environment, clear=True),
            patch.object(build_ios, "run", side_effect=run),
            patch.object(build_ios, "stage_onnx_runtime", return_value=(
                self.target_dir / "onnxruntime.xcframework",
                self.target_dir / "LICENSE",
            )),
            self.assertRaises(ReachedCargo),
        ):
            build_ios.main()
        native = next(call for call in calls if call[0][0] == "bash")
        cargo = next(call for call in calls if call[0][:2] == ["cargo", "build"])
        return native, cargo

    def test_native_and_rust_targets_match(self):
        cases = [
            (["--ipa"], "x86_64", "aarch64-apple-ios", "arm64", "iphoneos", ""),
            (["--physical-device", "phone", "--team", "team"], "arm64",
             "aarch64-apple-ios", "arm64", "iphoneos", ""),
            (["--no-run"], "arm64", "aarch64-apple-ios-sim", "arm64",
             "iphonesimulator", "-simulator"),
            (["--no-run"], "x86_64", "x86_64-apple-ios", "x86_64",
             "iphonesimulator", "-simulator"),
        ]
        for arguments, host, target, architecture, sdk, suffix in cases:
            with self.subTest(arguments=arguments, host=host):
                (native, native_env), (cargo, cargo_env) = self.configure_build(arguments, host)
                self.assertEqual(cargo[cargo.index("--target") + 1], target)
                flags = [
                    "-target", f"{architecture}-apple-ios{build_ios.MINIMUM_IOS}{suffix}",
                    "-isysroot", f"/SDKs/{sdk}.sdk",
                ]
                for variable in ("CFLAGS", "CXXFLAGS", "LDFLAGS"):
                    self.assertEqual(shlex.split(native_env[variable]), flags)
                self.assertEqual(shlex.split(cargo_env["BINDGEN_EXTRA_CLANG_ARGS"]), flags)
                self.assertEqual(native_env["CC"], f"/tools/{sdk}/clang")
                self.assertEqual(native_env["CXX"], f"/tools/{sdk}/clang++")
                self.assertEqual(cargo_env["SDKROOT"], f"/SDKs/{sdk}.sdk")
                self.assertEqual(cargo_env["CALIBRAW_LIBRAW_ROOT"], native[2])

    def test_ipa_bypasses_previously_cached_simulator_libraw(self):
        # Old IPA builds used the device Rust target but simulator Clang flags.
        legacy = self.target_dir / "ios-native/aarch64-apple-ios" / build_ios.MINIMUM_IOS / "libraw"
        pkgconfig = legacy / "lib/pkgconfig/libraw.pc"
        pkgconfig.parent.mkdir(parents=True)
        pkgconfig.write_text("cached simulator archive", encoding="utf-8")
        stamp = legacy / "share/calibraw/libraw-revision"
        stamp.parent.mkdir(parents=True)
        revision = next(
            line.split('"')[1]
            for line in (build_ios.ROOT / "Cargo.toml").read_text(encoding="utf-8").splitlines()
            if line.startswith("libraw_revision = ")
        )
        stamp.write_text(revision + "\n", encoding="utf-8")

        (native, _), (_, cargo_env) = self.configure_build(["--release", "--ipa"])

        self.assertNotEqual(Path(native[2]), legacy)
        self.assertEqual(cargo_env["CALIBRAW_LIBRAW_ROOT"], native[2])
        self.assertEqual(pkgconfig.read_text(encoding="utf-8"), "cached simulator archive")


if __name__ == "__main__":
    unittest.main()
