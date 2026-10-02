#!/usr/bin/env python3
"""Build the same file-local LLVM assembler/linker on every release host."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LLVM_VERSION = "18.1.8"
LLVM_COMMIT = "3b5b5c1ec4a3095ab096dd780e84d7ab81f3d7ff"


def run(*args):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(list(map(str, args)), check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, default=ROOT / "target/toolchain")
    parser.add_argument("--build-dir", type=Path, default=ROOT / "target/toolchain-build")
    parser.add_argument("--jobs", type=int, default=min(os.cpu_count() or 2, 4))
    parser.add_argument("--clean-build", action="store_true", help="Remove the source/build directory after copying release tools")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    prefix, build_root = args.prefix.resolve(), args.build_dir.resolve()
    if args.clean_build and (prefix == build_root or build_root in prefix.parents):
        parser.error("--clean-build cannot remove a build directory containing the toolchain prefix")
    extension = ".exe" if os.name == "nt" else ""
    manifest = prefix / "manifest.json"
    if manifest.exists() and json.loads(manifest.read_text())["commit"] == LLVM_COMMIT:
        if all((prefix / "bin" / (tool + extension)).is_file() for tool in ["llvm-mc", "ld.lld", "llvm-objdump"]):
            print(f"Pinned LLVM {LLVM_VERSION} toolchain already built at {prefix}")
            return
    source = build_root / "source"
    if not (source / ".git").exists():
        source.mkdir(parents=True, exist_ok=True)
        run("git", "init", source)
        run("git", "-C", source, "remote", "add", "origin", "https://github.com/llvm/llvm-project.git")
    if os.name == "nt":
        run("git", "-C", source, "config", "core.longpaths", "true")
    run("git", "-C", source, "fetch", "--depth=1", "origin", LLVM_COMMIT)
    run("git", "-C", source, "sparse-checkout", "init", "--cone")
    run("git", "-C", source, "sparse-checkout", "set", "llvm", "lld", "cmake", "third-party")
    run("git", "-C", source, "checkout", "--detach", LLVM_COMMIT)
    build = build_root / "build"
    options = [
        "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release", "-DLLVM_ENABLE_PROJECTS=lld",
        "-DLLVM_TARGETS_TO_BUILD=AArch64;ARM;X86", "-DLLVM_ENABLE_ASSERTIONS=OFF",
        "-DLLVM_INCLUDE_TESTS=OFF", "-DLLVM_INCLUDE_BENCHMARKS=OFF", "-DLLVM_INCLUDE_EXAMPLES=OFF",
        "-DLLVM_ENABLE_TERMINFO=OFF", "-DLLVM_ENABLE_ZLIB=OFF", "-DLLVM_ENABLE_ZSTD=OFF",
        "-DLLVM_ENABLE_LIBXML2=OFF", "-DLLVM_BUILD_LLVM_DYLIB=OFF", "-DLLVM_LINK_LLVM_DYLIB=OFF",
        "-DLLVM_PARALLEL_LINK_JOBS=1", "-DLLVM_USE_CRT_RELEASE=MT",
    ]
    if sys.platform == "darwin":
        options.append("-DCMAKE_OSX_DEPLOYMENT_TARGET=13.0")
    run("cmake", "-S", source / "llvm", "-B", build, *options)
    run("cmake", "--build", build, "--parallel", args.jobs, "--target", "llvm-mc", "lld", "llvm-objdump")
    binaries = prefix / "bin"
    binaries.mkdir(parents=True, exist_ok=True)
    for name, built in [("llvm-mc", "llvm-mc"), ("ld.lld", "lld"), ("llvm-objdump", "llvm-objdump")]:
        shutil.copy2(build / "bin" / (built + extension), binaries / (name + extension))
        run(binaries / (name + extension), "--version")
    licenses = prefix / "licenses"
    licenses.mkdir(exist_ok=True)
    for name in ["llvm", "lld", "third-party"]:
        for pattern in ["LICENSE*", "COPYING*", "NOTICE*"]:
            for file in (source / name).rglob(pattern):
                if file.is_file():
                    destination = licenses / name / file.relative_to(source / name)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(file, destination)
    manifest.write_text(json.dumps({"version": LLVM_VERSION, "commit": LLVM_COMMIT,
        "source": f"https://github.com/llvm/llvm-project/tree/{LLVM_COMMIT}",
        "targets": ["AArch64", "ARM", "X86"], "dynamic_llvm_library": False}, indent=2) + "\n")
    if args.clean_build:
        shutil.rmtree(build_root)
    print(f"Built release toolchain at {prefix}")


if __name__ == "__main__":
    main()
