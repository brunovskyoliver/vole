#!/usr/bin/env python3
"""Verify observable assembler/runtime results through the shipping CLI."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def execute(binary, arguments, environment=None):
    result = subprocess.run([str(binary), *arguments], capture_output=True, text=True,
                            timeout=30, env=environment)
    if result.returncode:
        raise RuntimeError(f"{' '.join(arguments)}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def assert_addition(snapshot, architecture):
    address = 0xBB if architecture == "vole" else 0x2000
    region = next(r for r in snapshot["memory"]
                  if r["base"] <= address < r["base"] + len(r["bytes"]))
    value = region["bytes"][address - region["base"]]
    if value != 125 or not snapshot["halted"]:
        raise AssertionError(f"{architecture}: expected halted state and memory[{address:X}]=125")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/vole-cli"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    results = {}
    with tempfile.TemporaryDirectory(prefix="vole-engine-verification-") as directory:
        for architecture in ["vole", "arm32", "arm64", "x86", "x64"]:
            image = Path(directory) / f"{architecture}.bin"
            snapshot = execute(binary, ["--arch", architecture, "--json", "--export-bytes", str(image)])
            assert_addition(snapshot, architecture)
            environment = os.environ.copy()
            # Invalid explicit tool paths prove that imported machine bytes do not assemble.
            environment["VOLE_LLVM_MC"] = str(Path(directory) / "absent-llvm-mc")
            environment["VOLE_LLD"] = str(Path(directory) / "absent-ld.lld")
            imported = execute(binary, ["--arch", architecture, "--bytes", str(image), "--json"], environment)
            assert_addition(imported, architecture)
            results[architecture] = {"assembled_example": "pass", "raw_export_import": "pass",
                                     "instructions": snapshot["steps"], "stored_value": 125}
    report = {"host": os.uname().sysname if hasattr(os, "uname") else os.name,
              "binary": str(binary), "targets": results}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
