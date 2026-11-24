"""Package a built native station without accessing audio, serial or user data."""
import argparse
import json
from pathlib import Path
import plistlib
import shutil
import tarfile
import zipfile


def package(platform, revision):
    root = Path(__file__).resolve().parents[1]
    name = f"wsjt-rusty-{platform}"
    output = root / "target/native-dist"
    output.mkdir(parents=True, exist_ok=True)
    staging = root / "target/native-package" / name
    # Each matrix job owns this packaging directory. Refuse accidental reuse.
    staging.mkdir(parents=True, exist_ok=False)
    executable = root / "target/release" / ("wsjt-native.exe" if platform.startswith("windows-") else "wsjt-native")
    if platform.startswith("macos-"):
        contents = staging / "wsjt-rusty.app/Contents"
        (contents / "MacOS").mkdir(parents=True)
        destination = contents / "MacOS/wsjt-native"
        with (contents / "Info.plist").open("wb") as stream:
            plistlib.dump({
                "CFBundleExecutable": "wsjt-native",
                "CFBundleIdentifier": "org.wsjt-rusty.native",
                "CFBundleName": "wsjt-rusty",
                "CFBundlePackageType": "APPL",
                "CFBundleVersion": "1",
                "NSMicrophoneUsageDescription": "Receive and decode amateur radio audio from your selected input.",
                "NSHighResolutionCapable": True,
                "LSMinimumSystemVersion": "13.0",
            }, stream)
    else:
        destination = staging / executable.name
    shutil.copy2(executable, destination)
    destination.chmod(0o755)
    for source in ["LICENSE", "README.md", "docs/native.md", "docs/contact-log.md", "docs/rig.md"]:
        target = staging / source
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / source, target)
    (staging / "BUILD.json").write_text(json.dumps({"platform": platform, "revision": revision}, indent=2) + "\n", encoding="utf-8")
    if platform.startswith("windows-"):
        archive = output / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as stream:
            for source in sorted(staging.rglob("*")):
                if source.is_file():
                    stream.write(source, source.relative_to(staging.parent))
    else:
        archive = output / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as stream:
            stream.add(staging, arcname=name)
    print(archive)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=["windows-x86_64", "linux-x86_64", "raspberry-pi4-aarch64", "macos-aarch64", "macos-x86_64"])
    parser.add_argument("--revision", required=True)
    args = parser.parse_args()
    package(args.platform, args.revision)
