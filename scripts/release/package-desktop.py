#!/usr/bin/env python3
"""Package a double-clickable Orbis app without changing the CLI distributions."""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

from version import parse_version

TARGETS = {
    "x86_64-pc-windows-msvc": ".zip",
    "x86_64-apple-darwin": ".tar.gz",
    "aarch64-apple-darwin": ".tar.gz",
    "x86_64-unknown-linux-gnu": ".tar.gz",
    "aarch64-unknown-linux-gnu": ".tar.gz",
}
ROOT = Path(__file__).resolve().parents[2]


def package(version: str, target: str, binary: Path, output: Path) -> Path:
    parse_version(version)
    extension = TARGETS[target]
    if not binary.is_file():
        raise ValueError(f"Missing desktop executable: {binary}")
    name = f"orbis-desktop-{version}-{target}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / (name + extension)
    with tempfile.TemporaryDirectory(prefix="orbis-desktop-") as temporary:
        root = Path(temporary) / name
        root.mkdir()
        if target.endswith("apple-darwin"):
            app = root / "Orbis.app"
            contents = app / "Contents"
            executable = contents / "MacOS" / "orbis-desktop"
            executable.parent.mkdir(parents=True)
            with (contents / "Info.plist").open("wb") as stream:
                plistlib.dump({
                    "CFBundleName": "Orbis",
                    "CFBundleDisplayName": "Orbis",
                    "CFBundleExecutable": "orbis-desktop",
                    "CFBundleIdentifier": "io.github.Antti.Orbis.Desktop",
                    "CFBundlePackageType": "APPL",
                    "CFBundleVersion": version,
                    "CFBundleShortVersionString": version,
                    "LSMinimumSystemVersion": "11.0",
                    "NSHighResolutionCapable": True,
                    # .pkg also means a macOS installer. Do not claim that type.
                }, stream)
            launch = "Move Orbis.app to Applications, then double-click it."
        elif target.endswith("windows-msvc"):
            executable = root / "Orbis.exe"
            launch = "Double-click Orbis.exe. Keep the desktop and CLI downloads in separate folders."
        else:
            executable = root / "Orbis"
            launch = "Double-click Orbis in your file manager."
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        # Keep the font license with the app even when users move only Orbis.app.
        licenses = contents / "Resources" / "licenses" if target.endswith("apple-darwin") else root / "licenses"
        licenses.mkdir(parents=True)
        shutil.copyfile(ROOT / "orbis-desktop/assets/fonts/OFL.txt", licenses / "Inter-OFL.txt")
        signing = "Unsigned build; not notarized."
        if target.endswith("apple-darwin") and os.environ.get("MACOS_NOTARIZED") == "true":
            subprocess.run(["bash", str(ROOT / "scripts/release/sign-macos.sh"), str(app)], check=True)
            signing = "Developer ID signed; Apple notarization accepted and ticket stapled."
        (root / "BUILD-INFO.txt").write_text(signing + "\n", encoding="utf-8")
        (root / "START-HERE.txt").write_text(
            f"Orbis {version}\n\n{launch}\n\n"
            "Use Open PKG or drop a PlayStation 4 PKG onto the window.\n"
            "For extraction, choose a destination and click Extract files.\n"
            "For an update, choose the extracted base game, check the update,\n"
            "close the emulator, and click Install update.\n\n"
            "Keep update separate preserves the base. Merge into game folder replaces\n"
            "game files and saves originals beside the game in .<game>.orbis-install.\n"
            "Keep that backup folder. Manage game lets you restore or recover the base\n"
            "without the PKG. Restore before installing another merged update.\n"
            "Existing extraction files are never overwritten.\n"
            "Use Resume to continue an interrupted extraction into the same folder.\n"
            "Keep the package unchanged while Orbis is working.\n\n"
            "Windows and Linux accept a PKG path as the first argument (Open With).\n"
            "No default file associations are changed. On macOS, use Open PKG or drop a file.\n"
            "Linux needs a graphical session, OpenGL, libxkbcommon, and an XDG desktop\n"
            "portal backend for file dialogs. See orbis-desktop/README.md for details.\n",
            encoding="utf-8",
        )
        for license_file in ("LICENSE-MIT", "LICENSE-APACHE"):
            shutil.copyfile(ROOT / license_file, root / license_file)
        if extension == ".zip":
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as dest:
                for source in sorted(root.rglob("*")):
                    if source.is_file():
                        dest.write(source, source.relative_to(root.parent))
        else:
            with tarfile.open(archive, "w:gz") as dest:
                dest.add(root, arcname=name)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(package(args.version, args.target, args.binary, args.output))


if __name__ == "__main__":
    main()
