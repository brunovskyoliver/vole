#!/usr/bin/env python3
"""Verify observable assembler, C compiler and runtime results through the shipping CLI."""
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


# Expected output is computed by hand: 3!+4! = 30, 5! = 120, and the sum of the
# squares 1..4 = 30 reached through nested calls and pointer writes.
C_PROGRAM = """#include <vole.h>
static int square(int x) { return x * x; }
static void add_square(int *total, int x) { *total += square(x); }
static int factorial(int n) { int result = 1; for (int i = 2; i <= n; i++) result *= i; return result; }
int main(void) {
    int values[3] = {factorial(3), factorial(4), factorial(5)};
    int squares = 0;
    for (int i = 1; i <= 4; i++) add_square(&squares, i);
    printf("Vole C %d %d %d\\n", values[0] + values[1], values[2], squares);
    return values[2] == 120 ? 0 : 1;
}
"""
C_OUTPUT = "Vole C 30 120 30\n"


def verify_c(binary, directory):
    source = Path(directory) / "verify.c"
    source.write_text(C_PROGRAM)
    results = {}
    for architecture in ["arm64", "x64", "arm32", "x86"]:
        snapshot = execute(binary, ["--arch", architecture, "--source", str(source), "--steps", "1000000", "--json"])
        output = bytes(snapshot["output"]).decode()
        if not snapshot["halted"] or output != C_OUTPUT:
            raise AssertionError(f"{architecture}: C program printed {output!r}, expected {C_OUTPUT!r}")
        results[architecture] = {"compiled_c": "pass", "instructions": snapshot["steps"], "output": output}
    return results


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
        c_results = verify_c(binary, directory)
    report = {"host": os.uname().sysname if hasattr(os, "uname") else os.name,
              "binary": str(binary), "targets": results, "c_targets": c_results}
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
