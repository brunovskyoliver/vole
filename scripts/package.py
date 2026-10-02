#!/usr/bin/env python3
"""Create a native portable package with LLVM/Clang tools, notices and smoke evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tomllib
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ("llvm-mc", "ld.lld", "llvm-objdump", "clang")
# Independently computed: 5! = 120 through a loop, a call and an array.
C_SMOKE = """#include <vole.h>
static int factorial(int n) { int result = 1; for (int i = 2; i <= n; i++) result *= i; return result; }
int main(void) { int values[3] = {factorial(3), factorial(4), factorial(5)}; int *last = &values[2];
    printf("Vole C %d %d\\n", values[0] + values[1], *last); return *last == 120 ? 0 : 1; }
"""
C_SMOKE_OUTPUT = "Vole C 30 120\n"
GLIBC = re.compile(r"^(lib(c|m|dl|rt|pthread|resolv|util|anl)\.so\.|ld-linux|linux-vdso)")


def run(args, **kwargs):
    return subprocess.run(list(map(str, args)), check=True, capture_output=True, text=True, **kwargs).stdout


def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def executable(name, folder=None):
    suffix = ".exe" if os.name == "nt" else ""
    if folder:
        folder = folder / "bin" if (folder / "bin").is_dir() else folder
        result = folder / (name + suffix)
        if result.is_file():
            return result.absolute()
        raise RuntimeError(f"Missing required bundled tool: {result}")
    for candidate in [name + "-14", name, *(f"{name}-{version}" for version in range(23, 14, -1))]:
        path = shutil.which(candidate)
        if path:
            return Path(path).absolute()
    raise RuntimeError(f"Cannot find {name}; provide --toolchain-dir from scripts/build-toolchain.py")


def dpkg_notice(path, destination, records):
    if not shutil.which("dpkg-query"):
        return
    matches = []
    for candidate in [path, Path("/usr") / str(path).lstrip("/")]:
        process = subprocess.run(["dpkg-query", "-S", str(candidate)], capture_output=True, text=True)
        if process.returncode == 0:
            matches.extend(process.stdout.splitlines())
    for match in matches:
        package = match.rsplit(": ", 1)[0]
        if package in records:
            continue
        result = run(["dpkg-query", "-W", "-f=${binary:Package}\t${Version}\t${source:Package}\t${source:Version}", package])
        fields = result.split("\t")
        records[package] = {"version": fields[1], "source_package": fields[2], "source_version": fields[3]}
        copyright_file = Path("/usr/share/doc") / package.split(":")[0] / "copyright"
        if copyright_file.exists():
            copy(copyright_file, destination / (package.replace(":", "_") + "-copyright.txt"))
        else:
            records[package]["notice_missing"] = str(copyright_file)


def linux_libraries(tool, folder, notices, records):
    text = run(["ldd", tool])
    if "not found" in text:
        raise RuntimeError(f"Missing runtime dependency for {tool}:\n{text}")
    for line in text.splitlines():
        match = re.search(r"([^\s]+)\s+=>\s+(/[^\s]+)", line)
        if not match:
            continue
        soname, path = match.groups()
        if GLIBC.match(soname):
            continue
        library = Path(path)
        copy(library, folder / soname)
        dpkg_notice(library, notices, records)
    return text


def cargo_notices(destination):
    metadata = json.loads(run(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT))
    records = []
    workspace = set(metadata["workspace_members"])
    locked = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    checksums = {(package["name"], package["version"]): package.get("checksum") for package in locked}
    for package in metadata["packages"]:
        source = Path(package["manifest_path"]).parent
        if package["id"] in workspace and source.is_relative_to(ROOT / "crates"):
            continue
        files = {file for pattern in ["LICENSE*", "LICENCE*", "COPYING*", "NOTICE*"] for file in source.rglob(pattern) if file.is_file()}
        if package.get("license_file"):
            file = source / package["license_file"]
            if file.is_file():
                files.add(file)
        copied = []
        for file in sorted(files):
            relative = Path("crates") / f"{package['name']}-{package['version']}" / file.relative_to(source)
            copy(file, destination / relative)
            copied.append(str(relative))
        record = {"name": package["name"], "version": package["version"], "license": package["license"],
            "authors": package["authors"], "repository": package["repository"], "source": package["source"], "notice_files": copied}
        if package["source"] is None:
            # Preserve patched/vendored dependency sources as well as their notices.
            archive_name = f"{package['name']}-{package['version']}-path-source.tar.gz"
            relative = Path("source-archives") / archive_name
            archive = destination / relative
            archive.parent.mkdir(parents=True, exist_ok=True)
            def source_filter(entry):
                parts = Path(entry.name).parts
                if any(part in {".git", "target", "dist", "build"} or part.startswith(".env") for part in parts):
                    return None
                return entry
            with tarfile.open(archive, "w:gz") as source_archive:
                source_archive.add(source, arcname=f"{package['name']}-{package['version']}", filter=source_filter)
            with archive.open("rb") as stream:
                checksum = hashlib.file_digest(stream, "sha256").hexdigest()
            record["source_archive"] = str(relative)
            record["source_archive_sha256"] = checksum
            record["source_kind"] = "vendored-path"
        if not copied:
            # Published .crate archives retain declarations and source copyright
            # headers even when their package excludes an upstream LICENSE file.
            if package["source"] is not None:
                archive_name = f"{package['name']}-{package['version']}.crate"
                archive = source.parent.parent.parent / "cache" / source.parent.name / archive_name
                if not archive.is_file():
                    raise RuntimeError(f"Missing original source archive needed for notices: {archive}")
                with archive.open("rb") as stream:
                    checksum = hashlib.file_digest(stream, "sha256").hexdigest()
                expected = checksums.get((package["name"], package["version"]))
                if expected and checksum != expected:
                    raise RuntimeError(f"Crate archive checksum disagrees with Cargo.lock: {archive_name}")
                relative = Path("source-archives") / archive_name
                copy(archive, destination / relative)
                record["source_archive"] = str(relative)
                record["source_archive_sha256"] = checksum
            identifiers = re.findall(r"[A-Za-z0-9][A-Za-z0-9.+-]*", package["license"] or "")
            record["license_texts"] = []
            for identifier in identifiers:
                if identifier in {"AND", "OR", "WITH"}:
                    continue
                text = ROOT / "packaging/licenses" / (identifier + ".txt")
                if not text.exists():
                    raise RuntimeError(f"Missing declared license text {identifier} for {archive_name}")
                relative = Path("license-texts") / text.name
                copy(text, destination / relative)
                record["license_texts"].append(str(relative))
        records.append(record)
    (destination / "rust-dependencies.json").write_text(json.dumps(records, indent=2) + "\n")
    return len(records)


def bundle_tools(folder, source_folder, notices, host):
    folder.mkdir(parents=True, exist_ok=True)
    version_records = {}
    system_packages = {}
    dependencies = {}
    suffix = ".exe" if host == "windows" else ""
    for name in TOOLS:
        tool = executable(name, source_folder)
        version_records[name] = run([tool, "--version"]).strip()
        if host == "linux":
            dpkg_notice(tool, notices / "system-libraries", system_packages)
            dependencies[name] = linux_libraries(tool, folder / "lib", notices / "system-libraries", system_packages)
            copy(tool, folder / ".bin" / name)
            launcher = folder / name
            launcher.write_text('#!/bin/sh\nset -eu\nTOOL_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)\n'
                'export LD_LIBRARY_PATH="$TOOL_ROOT/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"\n'
                f'exec "$TOOL_ROOT/.bin/{name}" "$@"\n')
            launcher.chmod(0o755)
        else:
            copy(tool, folder / (name + suffix))
            if host == "macos":
                dependencies[name] = run(["otool", "-L", tool])
                lines = dependencies[name].splitlines()[1:]
                external = [line.strip().split(" (", 1)[0] for line in lines if not line.strip().startswith(("/usr/lib/", "/System/Library/"))]
                if external:
                    raise RuntimeError(f"{name} has non-system dylibs {external}; build the pinned static LLVM toolchain")
    copy_resource_headers(executable("clang", source_folder), folder, version_records)
    if source_folder:
        for candidate in [source_folder / "licenses", source_folder.parent / "licenses"]:
            if candidate.is_dir():
                shutil.copytree(candidate, notices / "llvm", dirs_exist_ok=True)
        for candidate in [source_folder / "manifest.json", source_folder.parent / "manifest.json"]:
            if candidate.exists():
                copy(candidate, notices / "llvm-build.json")
                break
    if system_packages:
        (notices / "system-packages.json").write_text(json.dumps(system_packages, indent=2) + "\n")
    (notices / "toolchain-versions.json").write_text(json.dumps(version_records, indent=2) + "\n")
    (notices / "toolchain-dependencies.json").write_text(json.dumps(dependencies, indent=2) + "\n")


def copy_resource_headers(clang, folder, records):
    """Bundle Clang's freestanding headers where the compiler looks beside the bundled clang."""
    resource = Path(run([clang, "-print-resource-dir"]).strip())
    headers = resource / "include"
    if not (headers / "stddef.h").is_file():
        raise RuntimeError(f"Clang resource headers are missing at {headers}")
    destination = folder / "lib" / "clang" / resource.name / "include"
    shutil.copytree(headers, destination, dirs_exist_ok=True)
    records["clang-resource-headers"] = str(destination.relative_to(folder))


def c_smoke(binary, folder, environment):
    results = {}
    with tempfile.TemporaryDirectory(prefix="vole-c-smoke-") as directory:
        source = Path(directory) / "smoke.c"
        source.write_text(C_SMOKE)
        for target in ["arm64", "x64", "arm32", "x86"]:
            state = json.loads(run([binary, "--arch", target, "--source", source, "--json"],
                                   cwd=folder, env=environment, timeout=60))
            output = bytes(state["output"]).decode()
            if not state["halted"] or output != C_SMOKE_OUTPUT:
                raise RuntimeError(f"Packaged C smoke on {target} printed {output!r}")
            results[target] = {"halted": True, "steps": state["steps"], "output": output}
    return results


def smoke(binary, folder):
    # Tool discovery must use the package even when developer LLVM is on PATH.
    environment = os.environ.copy()
    environment.pop("VOLE_LLVM_MC", None)
    environment.pop("VOLE_LLD", None)
    environment.pop("VOLE_TOOLCHAIN_DIR", None)
    environment.pop("VOLE_CLANG", None)
    environment.pop("VOLE_CLANG_RESOURCE_DIR", None)
    environment["PATH"] = str(Path(sys.executable).parent) if os.name == "nt" else "/usr/bin:/bin"
    results = {}
    for target in ["vole", "arm32", "arm64", "x86", "x64"]:
        state = json.loads(run([binary, "--arch", target, "--json"], cwd=folder, env=environment, timeout=30))
        if not state["halted"]:
            raise RuntimeError(f"Packaged {target} example did not halt")
        registers = {register["name"].lower(): register["value"] for register in state["registers"]}
        result_register = {"vole": "r3", "arm32": "r3", "arm64": "x3", "x86": "eax", "x64": "rax"}[target]
        if registers[result_register] != 125:
            raise RuntimeError(f"Packaged {target} addition result is not 125: {registers}")
        # Independently validate the first output data cell for every architecture.
        address = 0xbb if target == "vole" else 0x2000
        region = next(region for region in state["memory"] if region["base"] <= address < region["base"] + len(region["bytes"]))
        if region["bytes"][address - region["base"]] != 125:
            raise RuntimeError(f"Packaged {target} did not write 125 into main memory")
        results[target] = {"halted": True, "steps": state["steps"], "register": result_register, "result": 125, "memory_address": address}
    results["c"] = c_smoke(binary, folder, environment)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, default=ROOT / "target/release")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--toolchain-dir", type=Path)
    parser.add_argument("--skip-smoke", action="store_true", help="Prepare an unverified package; mark manifest accordingly")
    args = parser.parse_args()
    host = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}.get(platform.system())
    if not host:
        parser.error("Packaging supports native Linux, macOS and Windows hosts")
    architecture = {"AMD64": "x64", "x86_64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(platform.machine(), platform.machine())
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    name = f"vole-{version}-{host}-{architecture}"
    destination = args.output.resolve() / name
    if destination.exists():
        parser.error(f"Refusing to overwrite {destination}; remove that previous package explicitly")
    suffix = ".exe" if host == "windows" else ""
    binaries = args.binary_dir.resolve()
    for filename in ["vole", "vole-cli"]:
        if not (binaries / (filename + suffix)).is_file():
            parser.error(f"Missing {binaries / (filename + suffix)}; build both app and CLI first")
    if host == "macos":
        package = destination / "Vole.app"
        application = package / "Contents/MacOS"
        resources = package / "Contents/Resources"
        plist = plistlib.loads((ROOT / "packaging/macos/Info.plist").read_bytes())
        plist["CFBundleShortVersionString"] = version
        plist["CFBundleVersion"] = version
        (package / "Contents").mkdir(parents=True)
        (package / "Contents/Info.plist").write_bytes(plistlib.dumps(plist))
    else:
        application = destination
        resources = destination
    for filename in ["vole", "vole-cli"]:
        copy(binaries / (filename + suffix), application / (filename + suffix))
    notices = resources / "notices"
    notices.mkdir(parents=True, exist_ok=True)
    copy(ROOT / "docs/dependency-notices.md", notices / "README.md")
    copy(ROOT / "Cargo.lock", notices / "Cargo.lock")
    shutil.copytree(ROOT / "packaging/licenses", notices / "license-texts", dirs_exist_ok=True)
    dependency_count = cargo_notices(notices)
    for file in (ROOT / "crates/vole-app/assets/fonts").glob("OFL-*.txt"):
        copy(file, notices / "fonts" / file.name)
    bundle_tools(resources / "toolchain", args.toolchain_dir.resolve() if args.toolchain_dir else None, notices, host)
    if host == "linux":
        dependencies = {filename: run(["ldd", application / filename]) for filename in ["vole", "vole-cli"]}
        (notices / "application-dependencies.json").write_text(json.dumps(dependencies, indent=2) + "\n")
    shutil.copytree(ROOT / "examples", resources / "examples")
    for filename in ["vole-spec.md", "isa-support.md", "c-environment.md", "native-verification.md"]:
        copy(ROOT / "docs" / filename, resources / "help" / filename)
    if host == "linux":
        copy(ROOT / "packaging/linux.desktop", destination / "vole.desktop")
        copy(ROOT / "packaging/install-linux.sh", destination / "install.sh")
    elif host == "windows":
        copy(ROOT / "packaging/windows/install.ps1", destination / "install.ps1")
    results = {} if args.skip_smoke else smoke(application / ("vole-cli" + suffix), destination)
    if not args.skip_smoke:
        environment = os.environ.copy()
        for variable in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_TOOLCHAIN_DIR", "VOLE_CLANG", "VOLE_CLANG_RESOURCE_DIR"]:
            environment.pop(variable, None)
        run([sys.executable, ROOT / "scripts/verify-engines.py", "--binary", application / ("vole-cli" + suffix),
             "--output", resources / "engine-roundtrip-verification.json"], env=environment, timeout=180)
    manifest = {"version": version, "host": host, "architecture": architecture, "rust_dependencies": dependency_count,
        "guest_smoke": results, "engine_roundtrips_verified": not args.skip_smoke, "native_window_verified": False,
        "build_host": platform.platform(), "c_runtime": platform.libc_ver(),
        "note": "Guest smoke checks do not establish native window behavior or distribution signing."}
    (resources / "package-verification.json").write_text(json.dumps(manifest, indent=2) + "\n")
    checksums = []
    for file in sorted(destination.rglob("*")):
        if file.is_file():
            with file.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            checksums.append(f"{digest}  {file.relative_to(destination)}")
    (destination / "SHA256SUMS").write_text("\n".join(checksums) + "\n")
    archive = shutil.make_archive(str(args.output.resolve() / name), "zip" if host != "linux" else "gztar", root_dir=destination.parent, base_dir=destination.name)
    print(json.dumps({"package": str(destination), "archive": archive, "guest_smoke": results}, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"Packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
