"""Replay setup with a missing Xcode metallib using local executable fixtures."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]

# Only the platform commands are fixtures. The real setup shell script and
# Python preflight run, with Darwin selected inside this private test process.
DRIVER = r'''
import importlib.util
import subprocess
import json
import os
from pathlib import Path
import sys

root = Path(os.environ["VOLE_SETUP_FIXTURE"])
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with (root / "commands.jsonl").open("a") as log:
    log.write(json.dumps([name, *args]) + "\n")
installed = root / "metal-installed"
if name == "uname":
    print("Darwin")
elif name == "brew":
    if args[0] == "--prefix":
        print(root)
elif name == "xcodebuild":
    if args == ["-version"]:
        print("Xcode 27.0\nBuild version 18A100")
    elif args == ["-downloadComponent", "MetalToolchain"]:
        if os.environ.get("VOLE_SETUP_DOWNLOAD_FAIL"):
            print("Metal component download failed", file=sys.stderr)
            sys.exit(1)
        installed.touch()
elif name == "xcrun":
    if args == ["--kill-cache"]:
        (root / "cache-cleared").touch()
    elif "--find" in args:
        tool = args[-1]
        stale = os.environ.get("VOLE_SETUP_CACHE_STALE") and not (root / "cache-cleared").exists()
        if tool == "metallib" and (not installed.exists() or stale or os.environ.get("VOLE_SETUP_STILL_MISSING")):
            print('xcrun: error: unable to find utility "metallib", not a developer tool or in PATH', file=sys.stderr)
            sys.exit(1)
        print(root / "bin" / tool)
    elif "--version" in args:
        print("Apple Metal version 27.0")
elif name == "rustup":
    if args[0] == "run":
        print("rustc 1.99.0\nhost: aarch64-apple-darwin")
elif name == "python3":
    script = Path(args[0]).resolve()
    spec = importlib.util.spec_from_file_location("macos", script)
    macos = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(macos)
    macos.platform.system = lambda: "Darwin"
    # Preserve fixture PATH even on CI Macs with real Homebrew installations.
    macos.environment = lambda: os.environ.copy()
    sys.argv = args
    try:
        macos.main()
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"macOS build failed: {error}", file=sys.stderr)
        sys.exit(1)
else:
    print("fixture tool version 18.0")
'''


class MacSetupTests(unittest.TestCase):
    def setUp(self):
        if os.name == "nt":
            self.skipTest("Shell setup fixture uses POSIX executables; Python build tests cover Windows.")
        self.temporary = tempfile.TemporaryDirectory(prefix="vole setup tests ")
        self.addCleanup(self.temporary.cleanup)
        self.fixture = Path(self.temporary.name)
        folder = self.fixture / "bin"
        folder.mkdir()
        for name in ["uname", "brew", "rustup", "xcodebuild", "xcrun", "python3", "cargo", "rustc",
                     "cmake", "pkg-config", "codesign", "open", "llvm-mc", "ld.lld", "llvm-objdump", "clang"]:
            executable = folder / name
            executable.write_text(f"#!{sys.executable}\n" + DRIVER)
            executable.chmod(0o755)
        self.environment = os.environ.copy()
        for name in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_CLANG", "VOLE_TOOLCHAIN_DIR", "CARGO_BUILD_TARGET", "DEVELOPER_DIR", "TOOLCHAINS"]:
            self.environment.pop(name, None)
        for name in ["VOLE_SETUP_DOWNLOAD_FAIL", "VOLE_SETUP_CACHE_STALE", "VOLE_SETUP_STILL_MISSING"]:
            self.environment.pop(name, None)
        self.environment.update({"VOLE_SETUP_FIXTURE": str(self.fixture), "CARGO_HOME": str(self.fixture / "cargo"),
                                 "PATH": str(folder) + os.pathsep + self.environment["PATH"]})

    def setup(self):
        return subprocess.run(["bash", ROOT / "scripts/setup-macos.sh"], cwd=ROOT, env=self.environment,
                              capture_output=True, text=True, timeout=30)

    def commands(self):
        return [json.loads(line) for line in (self.fixture / "commands.jsonl").read_text().splitlines()]

    def test_setup_installs_missing_metallib_and_rechecks_preflight(self):
        result = self.setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Ready: aarch64-apple-darwin", result.stdout)
        self.assertEqual(self.commands().count(["xcodebuild", "-downloadComponent", "MetalToolchain"]), 1)
        self.assertTrue((self.fixture / "metal-installed").exists())

    def test_setup_does_not_download_an_installed_working_component(self):
        (self.fixture / "metal-installed").touch()
        result = self.setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(["xcodebuild", "-downloadComponent", "MetalToolchain"], self.commands())

    def test_stale_tool_discovery_is_retried_without_redownloading(self):
        (self.fixture / "metal-installed").touch()
        self.environment["VOLE_SETUP_CACHE_STALE"] = "1"
        result = self.setup()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(["xcrun", "--kill-cache"], self.commands())
        self.assertNotIn(["xcodebuild", "-downloadComponent", "MetalToolchain"], self.commands())

    def test_failed_component_download_remains_a_setup_failure(self):
        self.environment["VOLE_SETUP_DOWNLOAD_FAIL"] = "1"
        result = self.setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Metal component download failed", result.stderr)
        self.assertNotIn("Ready:", result.stdout)
        self.assertEqual(self.commands().count(["xcodebuild", "-downloadComponent", "MetalToolchain"]), 1)

    def test_component_download_does_not_hide_unresolved_metallib(self):
        self.environment["VOLE_SETUP_STILL_MISSING"] = "1"
        result = self.setup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("still cannot resolve its tools", result.stderr)
        self.assertIn('unable to find utility "metallib"', result.stderr)
        self.assertNotIn("Ready:", result.stdout)


if __name__ == "__main__":
    unittest.main()
