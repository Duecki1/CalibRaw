#!/usr/bin/env python3
"""Build the experimental iOS app for a simulator, device or unsigned IPA."""

import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import shlex
import shutil
import subprocess
import sys
import zipfile


ROOT = Path(__file__).resolve().parent.parent
BUNDLE_ID = "de.duecki.calibraw.simulator"
MINIMUM_IOS = "17.0"
ONNX_RUNTIME_VERSION = "1.24.2"
ONNX_RUNTIME_SHA256 = "f7100a992d2a8135168c8afd831e6a58b465349101982aa58b3e11d36e600b54"


def run(arguments, environment, capture=False):
    print("+ " + shlex.join(map(str, arguments)), flush=True)
    return subprocess.run(
        list(map(str, arguments)),
        cwd=ROOT,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    ).stdout


def version_number(version):
    major, minor, patch = map(int, (version + ".0.0").split(".")[:3])
    return (major << 16) | (minor << 8) | patch


def simulator(environment, requested):
    runtimes = json.loads(run(["xcrun", "simctl", "list", "runtimes", "--json"], environment, True))
    ios = [runtime for runtime in runtimes["runtimes"] if runtime["isAvailable"] and runtime["name"].startswith("iOS ") and version_number(runtime["version"]) >= version_number(MINIMUM_IOS)]
    if not ios:
        raise RuntimeError("Install a compatible iOS simulator runtime in Xcode > Settings > Components, then rerun this script")
    runtime_ids = {runtime["identifier"] for runtime in ios}
    devices = json.loads(run(["xcrun", "simctl", "list", "devices", "available", "--json"], environment, True))
    available = [device for runtime_id, group in devices["devices"].items() if runtime_id in runtime_ids for device in group]
    if requested:
        matches = [device for device in available if device["udid"] == requested]
        if not matches:
            raise RuntimeError(f"Simulator {requested} is not available")
        return matches[0]
    booted = [device for device in available if device["state"] == "Booted"]
    if booted:
        return booted[0]
    runtime = max(ios, key=lambda item: version_number(item["version"]))
    existing = devices["devices"].get(runtime["identifier"], [])
    if existing:
        return next((device for device in existing if device["name"] == "iPhone 17"), existing[0])
    types = json.loads(run(["xcrun", "simctl", "list", "devicetypes", "--json"], environment, True))
    runtime_version = version_number(runtime["version"])
    supported_types = {device["identifier"] for device in runtime.get("supportedDeviceTypes", [])}
    phones = [device for device in types["devicetypes"] if device["name"].startswith("iPhone") and device["minRuntimeVersion"] <= runtime_version <= device["maxRuntimeVersion"] and (not supported_types or device["identifier"] in supported_types)]
    if not phones:
        raise RuntimeError(f"No installed iPhone device type supports {runtime['name']}")
    device_type = next((device for device in phones if device["name"] == "iPhone 17"), phones[0])
    udid = run(["xcrun", "simctl", "create", "CalibRaw iOS", device_type["identifier"], runtime["identifier"]], environment, True).strip()
    return {"udid": udid, "state": "Shutdown"}


def stage_onnx_runtime(target_dir, environment):
    archive = target_dir / f"onnxruntime-ios-{ONNX_RUNTIME_VERSION}.zip"
    run([
        sys.executable, ROOT / "scripts/bootstrap_download.py",
        f"https://download.onnxruntime.ai/pod-archive-onnxruntime-c-{ONNX_RUNTIME_VERSION}.zip",
        archive, ONNX_RUNTIME_SHA256,
    ], environment)
    runtime_dir = target_dir / f"ios-onnxruntime-{ONNX_RUNTIME_VERSION}"
    framework = runtime_dir / "onnxruntime.xcframework"
    required = [framework / name / "onnxruntime.framework/onnxruntime"
                for name in ["ios-arm64", "ios-arm64_x86_64-simulator"]]
    license_path = runtime_dir / "LICENSE"
    if not all(path.is_file() for path in [*required, license_path]):
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                if member.filename.startswith("onnxruntime.xcframework/ios-") or member.filename in [
                    "onnxruntime.xcframework/Info.plist", "LICENSE",
                ]:
                    source.extract(member, runtime_dir)
    if not all(path.is_file() for path in [*required, license_path]):
        raise RuntimeError("The pinned ONNX Runtime archive lacks its iOS slices or license")
    return framework, license_path


def stage_icons(output_dir, sdk_name, environment):
    catalog = output_dir / "icons.xcassets"
    app_icon = catalog / "AppIcon.appiconset"
    app_icon.mkdir(parents=True, exist_ok=True)
    renderer = """
import Foundation
import CoreGraphics
import ImageIO
guard let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: CommandLine.arguments[1]) as CFURL, nil),
    let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
    let context = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8,
        bytesPerRow: 4096, space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else {
    fatalError("Could not create opaque app icon")
}
let bounds = CGRect(x: 0, y: 0, width: 1024, height: 1024)
context.setFillColor(red: 1, green: 1, blue: 1, alpha: 1)
context.fill(bounds)
context.draw(image, in: bounds)
guard let rendered = context.makeImage(),
    let destination = CGImageDestinationCreateWithURL(URL(fileURLWithPath: CommandLine.arguments[2]) as CFURL,
        "public.png" as CFString, 1, nil) else {
    fatalError("Could not encode app icon")
}
CGImageDestinationAddImage(destination, rendered, nil)
guard CGImageDestinationFinalize(destination) else { fatalError("Could not write app icon") }
"""
    run(["xcrun", "--sdk", "macosx", "swift", "-e", renderer,
         ROOT / "fastlane/metadata/android/en-US/images/icon.png", app_icon / "AppIcon.png"], environment)
    contents = {
        "images": [{"filename": "AppIcon.png", "idiom": "universal", "platform": "ios", "size": "1024x1024"}],
        "info": {"author": "xcode", "version": 1},
    }
    (app_icon / "Contents.json").write_text(json.dumps(contents), encoding="utf-8")
    resources = output_dir / "icon-resources"
    resources.mkdir(parents=True, exist_ok=True)
    partial_info = output_dir / "icon-info.plist"
    run([
        "xcrun", "actool", "--compile", resources, "--platform", sdk_name,
        "--minimum-deployment-target", MINIMUM_IOS, "--app-icon", "AppIcon",
        "--target-device", "iphone", "--target-device", "ipad",
        "--output-partial-info-plist", partial_info, catalog,
    ], environment)
    with partial_info.open("rb") as source:
        return resources, plistlib.load(source)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", action="store_true")
    parser.add_argument("--no-run", action="store_true", help="Only build the app bundle")
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument("--device", help="Simulator UDID to install and launch on")
    destination.add_argument("--physical-device", help="USB-connected iPhone UDID to sign, install and launch on")
    destination.add_argument("--ipa", action="store_true", help="Build an unsigned ARM64 IPA for re-signing with AltStore")
    parser.add_argument("--team", help="Apple development team ID, required for a physical device")
    options = parser.parse_args()
    if options.physical_device and not options.team:
        raise RuntimeError("A physical-device build requires --team and an Apple developer account configured in Xcode")
    if sys.platform != "darwin":
        raise RuntimeError("The simulator build requires macOS and full Xcode")
    environment = os.environ.copy()
    if "DEVELOPER_DIR" not in environment:
        selected = run(["xcode-select", "-p"], environment, True).strip()
        environment["DEVELOPER_DIR"] = selected if ".app/Contents/Developer" in selected else "/Applications/Xcode.app/Contents/Developer"
    for tool in ["cargo", "rustc", "autoreconf", "make", "xcrun", "codesign"]:
        if not shutil.which(tool, path=environment.get("PATH")):
            raise RuntimeError(f"Required tool {tool} is missing from PATH")
    device_build = bool(options.physical_device or options.ipa)
    arm = device_build or platform.machine() == "arm64"
    target = "aarch64-apple-ios" if device_build else ("aarch64-apple-ios-sim" if arm else "x86_64-apple-ios")
    architecture = "arm64" if arm else "x86_64"
    sdk_name = "iphoneos" if device_build else "iphonesimulator"
    target_libraries = Path(run(["rustc", "--print", "target-libdir", "--target", target], environment, True).strip())
    if not list(target_libraries.glob("libstd-*.rlib")):
        raise RuntimeError(f"Install the Rust simulator target with `rustup target add {target}` and use its cargo/rustc on PATH")
    sdk = run(["xcrun", "--sdk", sdk_name, "--show-sdk-path"], environment, True).strip()
    environment["SDKROOT"] = sdk
    clang_target = f"{architecture}-apple-ios{MINIMUM_IOS}" + ("" if device_build else "-simulator")
    flags = shlex.join(["-target", clang_target, "-isysroot", sdk])
    target_dir = Path(environment.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target_dir.is_absolute():
        target_dir = ROOT / target_dir
    target_dir = target_dir.resolve()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    target_dir.mkdir(parents=True, exist_ok=True)
    onnx_framework, onnx_license = stage_onnx_runtime(target_dir, environment)
    environment["ORT_IOS_XCFWK_PATH"] = str(onnx_framework)
    # The Clang target names architecture, minimum iOS and simulator or device,
    # so builds for different slices never share LibRaw objects.
    prefix = target_dir / "ios-native" / clang_target / "libraw"
    native_environment = environment.copy()
    native_environment.update({
        "CC": run(["xcrun", "--sdk", sdk_name, "--find", "clang"], environment, True).strip(),
        "CXX": run(["xcrun", "--sdk", sdk_name, "--find", "clang++"], environment, True).strip(),
        "CFLAGS": flags,
        "CXXFLAGS": flags,
        "LDFLAGS": flags,
        "LIBTOOLIZE": "glibtoolize",
        "PKG_CONFIG_PATH": "",
        "PKG_CONFIG_LIBDIR": str(prefix / "lib/pkgconfig"),
    })
    run([
        "bash", ROOT / "scripts/build_libraw.sh", prefix,
        "--host=aarch64-apple-darwin" if arm else "--host=x86_64-apple-darwin",
        "--enable-static", "--disable-shared", "--disable-openmp",
        "--disable-lcms", "--disable-jpeg",
    ], native_environment)
    environment.update({
        "CALIBRAW_LIBRAW_ROOT": str(prefix),
        "BINDGEN_EXTRA_CLANG_ARGS": flags,
        "IPHONEOS_DEPLOYMENT_TARGET": MINIMUM_IOS,
    })
    environment.pop("CALIBRAW_ALLOW_NO_LIBRAW", None)
    command = ["cargo", "build", "--locked", "-p", "calibraw-ui", "--bin", "calibraw", "--target", target]
    if options.release:
        command.append("--release")
    run(command, environment)
    profile = "release" if options.release else "debug"
    output_dir = target_dir / ("ios-ipa" if options.ipa else ("ios-device" if options.physical_device else "ios-simulator"))
    bundle = output_dir / profile / "CalibRaw.app"
    bundle.mkdir(parents=True, exist_ok=True)
    shutil.copy2(target_dir / target / profile / "calibraw", bundle / "CalibRaw")
    shutil.copy2(onnx_license, bundle / "onnxruntime-LICENSE.txt")
    icon_resources, icon_info = stage_icons(output_dir / profile, sdk_name, environment)
    for resource in icon_resources.iterdir():
        shutil.copy2(resource, bundle / resource.name)
    metadata = json.loads(run(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], environment, True))
    version = next(package["version"] for package in metadata["packages"] if package["name"] == "calibraw-ui")
    info = {
        "CFBundleExecutable": "CalibRaw",
        "CFBundleIdentifier": "de.duecki.calibraw" if options.ipa else ("de.duecki.calibraw.ios-dev" if options.physical_device else BUNDLE_ID),
        "CFBundleName": "CalibRaw",
        "CFBundleDisplayName": "CalibRaw",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": "1",
        "CFBundleSupportedPlatforms": ["iPhoneOS" if device_build else "iPhoneSimulator"],
        "MinimumOSVersion": MINIMUM_IOS,
        "UIDeviceFamily": [1, 2],
        "UILaunchScreen": {},
        "UIApplicationSceneManifest": {
            "UIApplicationSupportsMultipleScenes": False,
            "UISceneConfigurations": {
                "UIWindowSceneSessionRoleApplication": [{
                    "UISceneConfigurationName": "CalibRaw",
                    "UISceneDelegateClassName": "CalibRawSceneDelegate",
                }],
            },
        },
        "UIFileSharingEnabled": True,
        "LSSupportsOpeningDocumentsInPlace": True,
        "NSPhotoLibraryAddUsageDescription": "Save your exported photos to your photo library.",
        "UISupportedInterfaceOrientations": [
            "UIInterfaceOrientationPortrait", "UIInterfaceOrientationLandscapeLeft",
            "UIInterfaceOrientationLandscapeRight",
        ],
    }
    info.update(icon_info)
    with (bundle / "Info.plist").open("wb") as output:
        plistlib.dump(info, output)
    if options.ipa:
        for name in ["COPYING", "THIRD_PARTY_NOTICES.md"]:
            shutil.copy2(ROOT / name, bundle / name)
        # Keep requested entitlements in the ad-hoc signature for IPA re-signing.
        run(["codesign", "--force", "--sign", "-", "--entitlements",
             ROOT / "packaging/ios/CalibRaw.entitlements", bundle], environment)
        ipa = output_dir / profile / "CalibRaw-ios-arm64.ipa"
        with zipfile.ZipFile(ipa, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(bundle.rglob("*")):
                if path.is_file():
                    archive.write(path, Path("Payload") / path.relative_to(bundle.parent))
        print(f"Built unsigned {ipa}", flush=True)
        return
    if options.physical_device:
        derived_data = output_dir / "xcode"
        configuration = "Release" if options.release else "Debug"
        run([
            "xcodebuild", "-project", ROOT / "packaging/ios/CalibRaw.xcodeproj",
            "-scheme", "CalibRaw", "-configuration", configuration,
            "-destination", f"id={options.physical_device}",
            "-derivedDataPath", derived_data,
            "-allowProvisioningUpdates", "-allowProvisioningDeviceRegistration",
            f"DEVELOPMENT_TEAM={options.team}",
            f"CALIBRAW_IOS_BINARY={bundle / 'CalibRaw'}",
            f"CALIBRAW_IOS_ICON_DIR={icon_resources}",
            f"CALIBRAW_IOS_RUNTIME_LICENSE={onnx_license}",
            f"INFOPLIST_FILE={bundle / 'Info.plist'}", "build",
        ], environment)
        signed_bundle = derived_data / "Build/Products" / f"{configuration}-iphoneos/CalibRaw.app"
        print(f"Signed {signed_bundle}", flush=True)
        if not options.no_run:
            run(["xcrun", "devicectl", "device", "install", "app", "--device", options.physical_device, signed_bundle], environment)
            run(["xcrun", "devicectl", "device", "process", "launch", "--device", options.physical_device, "--terminate-existing", info["CFBundleIdentifier"]], environment)
        return
    run(["codesign", "--force", "--sign", "-", bundle], environment)
    print(f"Built {bundle}", flush=True)
    if options.no_run:
        return
    device = simulator(environment, options.device)
    if device["state"] != "Booted":
        run(["xcrun", "simctl", "boot", device["udid"]], environment)
    run(["xcrun", "simctl", "bootstatus", device["udid"], "-b"], environment)
    simulator_app = Path(environment["DEVELOPER_DIR"]) / "Applications/Simulator.app"
    if simulator_app.is_dir():
        run(["open", simulator_app, "--args", "-CurrentDeviceUDID", device["udid"]], environment)
    else:
        print("Simulator GUI is missing; installing and launching headlessly", flush=True)
    run(["xcrun", "simctl", "install", device["udid"], bundle], environment)
    run(["xcrun", "simctl", "launch", "--terminate-running-process", device["udid"], BUNDLE_ID], environment)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        sys.exit(f"error: {error}")
