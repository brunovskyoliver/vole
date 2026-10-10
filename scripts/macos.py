#!/usr/bin/env python3
"""Build and launch a local macOS app bundle with its guest assembler and C compiler tools."""
import argparse
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ("llvm-mc", "ld.lld", "llvm-objdump", "clang")


def run(args, environment, capture=False):
    if args[0] in {"cargo", "rustc"}:
        match = re.search(r'^channel\s*=\s*"([^"]+)"', (ROOT / "rust-toolchain.toml").read_text(), re.MULTILINE)
        if not match:
            raise RuntimeError("Cannot read the pinned Rust toolchain.")
        # A separately installed Homebrew rust must not bypass the repo's pin.
        args = ["rustup", "run", match.group(1), *args]
    return subprocess.run(list(map(str, args)), cwd=ROOT, env=environment,
                          check=True, text=True, capture_output=capture).stdout


def environment():
    result = os.environ.copy()
    paths = [str(Path(result.get("CARGO_HOME", Path.home() / ".cargo")) / "bin")]
    homebrew_paths = ["/opt/homebrew/bin", "/usr/local/bin"]
    result["PATH"] = os.pathsep.join([*paths, *homebrew_paths, result.get("PATH", "")])
    brew = shutil.which("brew", path=result["PATH"])
    if brew:
        for formula in ["rustup", "llvm", "lld"]:
            process = subprocess.run([brew, "--prefix", formula], text=True,
                                     capture_output=True, env=result)
            if process.returncode == 0:
                paths.append(str(Path(process.stdout.strip()) / "bin"))
        result["PATH"] = os.pathsep.join([*paths, *homebrew_paths, os.environ.get("PATH", "")])
    result.setdefault("MACOSX_DEPLOYMENT_TARGET", "13.0")
    return result


def require_tool(name, environment):
    tool = shutil.which(name, path=environment["PATH"])
    if not tool:
        raise RuntimeError(f"Missing {name}. Run make mac-setup first.")
    return Path(tool).absolute()


def tool_launcher(source, destination):
    # Keep Homebrew's executable in its installed location so relative dylib
    # references stay valid. Finder can run this launcher without shell setup.
    destination.write_text(f'#!/bin/sh\nexec {shlex.quote(str(source))} "$@"\n')
    destination.chmod(0o755)


def guest_tools(environment):
    folder = environment.get("VOLE_TOOLCHAIN_DIR")
    if folder:
        folder = Path(folder).expanduser().absolute()
        if (folder / "bin").is_dir():
            folder = folder / "bin"
    paths = {}
    for name in TOOLS:
        override = {"llvm-mc": "VOLE_LLVM_MC", "ld.lld": "VOLE_LLD", "clang": "VOLE_CLANG"}.get(name)
        explicit = environment.get(override) if override else None
        tool = Path(explicit).expanduser().absolute() if explicit else (folder / name if folder else require_tool(name, environment))
        if not tool.is_file() or not os.access(tool, os.X_OK):
            raise RuntimeError(f"Guest tool is not executable: {tool}")
        run([tool, "--version"], environment, capture=True)
        paths[name] = tool
    return paths


def ensure_metal(environment, install=False):
    def check():
        for tool in ["metal", "metallib"]:
            run(["xcrun", "--sdk", "macosx", "--find", tool], environment, capture=True)
        # Newer Xcode can locate a forwarding stub without its component.
        run(["xcrun", "--sdk", "macosx", "metal", "--version"], environment, capture=True)

    try:
        check()
        return
    except subprocess.CalledProcessError:
        # Tool discovery can remain stale after an Xcode/component update.
        run(["xcrun", "--kill-cache"], environment, capture=True)
    try:
        check()
        return
    except subprocess.CalledProcessError as error:
        if not install:
            detail = (error.stderr or error.stdout or "").strip()
            raise RuntimeError("Xcode's Metal component is unavailable. Run make mac-setup to install it, "
                               "or run xcodebuild -downloadComponent MetalToolchain.\n" + detail) from error

    print("Installing Xcode's Metal toolchain for GPUI shader compilation...", flush=True)
    try:
        run(["xcodebuild", "-downloadComponent", "MetalToolchain"], environment)
    except subprocess.CalledProcessError as error:
        raise RuntimeError("Metal component download failed. Retry xcodebuild -downloadComponent MetalToolchain "
                           "or install Metal Toolchain in Xcode Settings > Components, then rerun make mac-setup.") from error
    run(["xcrun", "--kill-cache"], environment, capture=True)
    try:
        check()
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        raise RuntimeError("Metal component installation completed, but Xcode still cannot resolve its tools. "
                           "Check xcodebuild -showComponent MetalToolchain and the selected Xcode installation.\n" + detail) from error


def preflight(environment, install_metal=False):
    if platform.system() != "Darwin":
        raise RuntimeError("These build commands require macOS. Use cargo run -p vole-app on Linux/Windows.")
    if sys.version_info < (3, 11):
        raise RuntimeError("Python 3.11+ is required. Run make mac-setup, then retry with Homebrew python3.")
    for tool in ["cargo", "rustc", "rustup", "cmake", "pkg-config", "xcodebuild", "xcrun", "codesign", "open"]:
        require_tool(tool, environment)
    try:
        run(["xcodebuild", "-version"], environment, capture=True)
        run(["xcrun", "--sdk", "macosx", "--find", "clang"], environment, capture=True)
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        raise RuntimeError("Full Xcode with the macOS SDK is required. Open Xcode to finish setup. "
                           "Select Xcode using xcode-select if only Command Line Tools are active.\n" + detail) from error
    ensure_metal(environment, install=install_metal)
    host = run(["rustc", "-vV"], environment, capture=True)
    match = re.search(r"^host: (.+)$", host, flags=re.MULTILINE)
    if not match or match.group(1) not in {"aarch64-apple-darwin", "x86_64-apple-darwin"}:
        raise RuntimeError("The active Rust toolchain must target a native macOS host.")
    if environment.get("CARGO_BUILD_TARGET", match.group(1)) != match.group(1):
        raise RuntimeError("Unset CARGO_BUILD_TARGET to build for this Mac's native Rust host.")
    environment["VOLE_NATIVE_HOST"] = match.group(1)
    tools = guest_tools(environment)
    environment["VOLE_LLVM_MC"] = str(tools["llvm-mc"])
    environment["VOLE_LLD"] = str(tools["ld.lld"])
    environment["VOLE_CLANG"] = str(tools["clang"])
    # Apple's Xcode clang cannot build the guest ELF images; Homebrew LLVM's clang can.
    version = run([tools["clang"], "--version"], environment, capture=True)
    if "Apple" in (version.splitlines() or [""])[0]:
        raise RuntimeError(f"{tools['clang']} is Apple clang. Run make mac-setup to install Homebrew LLVM, "
                           "or set VOLE_CLANG to an LLVM clang 14+.")
    print(f"Ready: {match.group(1)}, Xcode/Metal, Rust, LLVM guest tools and Clang.", flush=True)
    return tools


def build(environment, tools, profile):
    host = environment["VOLE_NATIVE_HOST"]
    args = ["cargo", "build", "--locked", "--target", host, "-p", "vole-app", "-p", "vole-cli"]
    if profile == "release":
        args.append("--release")
    run(args, environment)
    metadata = json.loads(run(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], environment, capture=True))
    target = Path(metadata["target_directory"])
    binaries = target / host / profile
    destination = target / "macos" / profile
    destination.mkdir(parents=True, exist_ok=True)
    app = destination / "Vole.app"
    # Build a complete sibling bundle first, keeping a previous build on failure.
    with tempfile.TemporaryDirectory(prefix=".vole-build-", dir=destination) as temporary:
        staged = Path(temporary) / "Vole.app"
        contents = staged / "Contents"
        executable_dir = contents / "MacOS"
        resource_dir = contents / "Resources"
        executable_dir.mkdir(parents=True)
        (resource_dir / "toolchain").mkdir(parents=True)
        plist = plistlib.loads((ROOT / "packaging/macos/Info.plist").read_bytes())
        version = next(package["version"] for package in metadata["packages"] if package["name"] == "vole-app")
        plist["CFBundleShortVersionString"] = version
        plist["CFBundleVersion"] = version
        (contents / "Info.plist").write_bytes(plistlib.dumps(plist))
        for name in ["vole", "vole-cli"]:
            shutil.copy2(binaries / name, executable_dir / name)
        shutil.copy2(ROOT / "packaging/macos/Vole.icns", resource_dir / "Vole.icns")
        for name, tool in tools.items():
            tool_launcher(tool, resource_dir / "toolchain" / name)
        local_env = environment.copy()
        local_env["PATH"] = "/usr/bin:/bin"
        for variable in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_TOOLCHAIN_DIR", "VOLE_CLANG"]:
            local_env.pop(variable, None)
        for name in TOOLS:
            try:
                run([resource_dir / "toolchain" / name, "--version"], local_env, capture=True)
            except subprocess.CalledProcessError as error:
                raise RuntimeError(f"Bundled launcher for {name} cannot run. Check the selected local toolchain.\n" + (error.stderr or "")) from error
        # Development tools may retain Homebrew dylib paths on this Mac.
        (resource_dir / "DEVELOPMENT-BUILD.txt").write_text(
            "Local development bundle. LLVM launchers reference the installed tools on this Mac.\n"
            "Use scripts/package.py with the pinned static toolchain for redistribution.\n")
        run(["codesign", "--force", "--sign", "-", executable_dir / "vole-cli"], environment)
        run(["codesign", "--force", "--sign", "-", staged], environment)
        run(["codesign", "--verify", "--deep", "--strict", staged], environment)
        previous = Path(temporary) / "previous.app"
        if app.exists():
            app.rename(previous)
        try:
            staged.rename(app)
        except OSError:
            if previous.exists():
                previous.rename(app)
            raise
    print(f"Built {app}", flush=True)
    return app


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["setup", "check", "build", "run", "verify"])
    parser.add_argument("--profile", choices=["debug", "release"], default="debug")
    args = parser.parse_args()
    env = environment()
    tools = preflight(env, install_metal=True) if args.action == "setup" else preflight(env)
    if args.action in {"setup", "check"}:
        return
    if args.action == "verify":
        run(["cargo", "fmt", "--all", "--check"], env)
        run(["cargo", "test", "--workspace", "--locked"], env)
        run(["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"], env)
    app = build(env, tools, args.profile)
    if args.action == "run":
        print("Opening a new app instance. Close any earlier Vole instance before rebuilding.", flush=True)
        # LaunchServices identifies the real .app and its Dock/window identity.
        run(["open", "-n", app], env)
    elif args.action == "verify":
        # Exercise tools inside the bundle rather than developer tool overrides.
        for variable in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_TOOLCHAIN_DIR", "VOLE_CLANG"]:
            env.pop(variable, None)
        env["PATH"] = "/usr/bin:/bin"
        run([sys.executable, ROOT / "scripts/verify-engines.py", "--binary", app / "Contents/MacOS/vole-cli",
             "--output", app.parent / "engine-verification.json"], env)


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"macOS build failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.strip(), file=sys.stderr)
        sys.exit(1)
