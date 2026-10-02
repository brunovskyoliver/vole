"""Check macOS build orchestration without pretending to run a Mac window."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("macos", Path(__file__).resolve().parents[1] / "macos.py")
macos = importlib.util.module_from_spec(spec)
spec.loader.exec_module(macos)


class MacBuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="vole build tests ")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.target = self.root / "custom target"
        self.environment = {"VOLE_NATIVE_HOST": "aarch64-apple-darwin"}
        plist = self.root / "packaging/macos/Info.plist"
        plist.parent.mkdir(parents=True)
        plist.write_bytes((macos.ROOT / "packaging/macos/Info.plist").read_bytes())
        self.tools = {}
        for name in macos.TOOLS:
            tool = self.root / "LLVM tools" / name
            tool.parent.mkdir(exist_ok=True)
            tool.write_bytes(f"tool:{name}".encode())
            tool.chmod(0o755)
            self.tools[name] = tool
        for profile in ["debug", "release"]:
            folder = self.target / self.environment["VOLE_NATIVE_HOST"] / profile
            folder.mkdir(parents=True)
            for name in ["vole", "vole-cli"]:
                binary = folder / name
                binary.write_bytes(f"binary:{profile}:{name}".encode())
                binary.chmod(0o755)
        self.calls = []

    def test_cargo_always_runs_through_the_repository_pinned_rustup_toolchain(self):
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.0"\n')
        with patch.object(macos, "ROOT", self.root), patch.object(macos.subprocess, "run") as process:
            macos.run(["cargo", "build", "--locked"], {"PATH": "/a/brewed/rust/bin"})
        self.assertEqual(process.call_args.args[0], ["rustup", "run", "1.99.0", "cargo", "build", "--locked"])

    def fake_run(self, args, environment, capture=False):
        self.calls.append(list(map(str, args)))
        if args[:2] == ["cargo", "metadata"]:
            return json.dumps({"target_directory": str(self.target), "packages": [{"name": "vole-app", "version": "0.1.0"}]})
        return ""

    def test_bundle_and_launch_use_complete_native_app_with_paths_containing_spaces(self):
        with patch.object(macos, "ROOT", self.root), patch.object(macos, "run", side_effect=self.fake_run), \
             patch.object(macos, "environment", return_value=self.environment), patch.object(macos, "preflight", return_value=self.tools), \
             patch("sys.argv", ["macos.py", "run", "--profile", "release"]):
            macos.main()
        app = self.target / "macos/release/Vole.app"
        plist = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        self.assertEqual(plist["CFBundleIdentifier"], "dev.vole.Workbench")
        self.assertEqual(plist["CFBundleExecutable"], "vole")
        self.assertFalse(plist["LSUIElement"])
        self.assertEqual((app / "Contents/MacOS/vole").read_bytes(), b"binary:release:vole")
        self.assertTrue(os.access(app / "Contents/MacOS/vole", os.X_OK))
        for name in macos.TOOLS:
            launcher = app / "Contents/Resources/toolchain" / name
            self.assertIn(str(self.tools[name]), launcher.read_text())
            self.assertTrue(os.access(launcher, os.X_OK))
        self.assertIn(["cargo", "build", "--locked", "--target", "aarch64-apple-darwin", "-p", "vole-app", "-p", "vole-cli", "--release"], self.calls)
        signing = [args for args in self.calls if args[:2] == ["codesign", "--force"]]
        self.assertTrue(signing[0][-1].endswith("Contents/MacOS/vole-cli"))
        self.assertTrue(signing[1][-1].endswith("Vole.app"))
        self.assertEqual(self.calls[-1], ["open", "-n", str(app)])

    def test_tool_launcher_preserves_arguments_and_quoted_paths_without_developer_path(self):
        source = self.root / "tool's folder" / "llvm-mc"
        source.parent.mkdir()
        source.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        source.chmod(0o755)
        launcher = self.root / "launcher"
        macos.tool_launcher(source, launcher)
        result = subprocess.run([launcher, "--version", "argument with spaces", "$(literal)"],
                                env={"PATH": "/usr/bin:/bin"}, capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout, "--version\nargument with spaces\n$(literal)\n")

    def test_failed_signing_preserves_previous_bundle(self):
        app = self.target / "macos/debug/Vole.app"
        app.mkdir(parents=True)
        (app / "previous-build.txt").write_text("keep")

        def fail_sign(args, environment, capture=False):
            if args[0] == "codesign":
                raise subprocess.CalledProcessError(1, args)
            return self.fake_run(args, environment, capture)

        with patch.object(macos, "ROOT", self.root), patch.object(macos, "run", side_effect=fail_sign):
            with self.assertRaises(subprocess.CalledProcessError):
                macos.build(self.environment, self.tools, "debug")
        self.assertEqual((app / "previous-build.txt").read_text(), "keep")
        self.assertEqual(list(app.parent.glob(".vole-build-*")), [])

    def test_verification_uses_packaged_tools_and_clears_developer_overrides(self):
        env = {"PATH": "/developer/tools", "VOLE_LLVM_MC": "/developer/mc", "VOLE_LLD": "/developer/lld", "VOLE_TOOLCHAIN_DIR": "/developer/tools"}
        app = self.target / "macos/debug/Vole.app"
        with patch.object(macos, "environment", return_value=env), patch.object(macos, "preflight", return_value=self.tools), \
             patch.object(macos, "build", return_value=app), patch.object(macos, "run", side_effect=self.fake_run), \
             patch("sys.argv", ["macos.py", "verify"]):
            macos.main()
        for name in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_TOOLCHAIN_DIR"]:
            self.assertNotIn(name, env)
        self.assertEqual(env["PATH"], "/usr/bin:/bin")
        self.assertIn(str(app / "Contents/MacOS/vole-cli"), self.calls[-1])
        self.assertTrue(any(args[:2] == ["cargo", "test"] for args in self.calls))

    def test_apple_clang_is_rejected_for_guest_c(self):
        def apple_clang(args, environment, capture=False):
            if args[0] == "rustc":
                return "rustc 1.99.0\nhost: aarch64-apple-darwin\n"
            if str(args[0]).endswith("clang"):
                return "Apple clang version 17.0.0 (clang-1700.0.13.3)\n"
            return ""

        tools = dict(self.tools, clang=Path("/usr/bin/clang"))
        with patch.object(macos.platform, "system", return_value="Darwin"), \
             patch.object(macos, "require_tool", return_value=Path("/mock/tool")), \
             patch.object(macos, "guest_tools", return_value=tools), \
             patch.object(macos, "run", side_effect=apple_clang):
            with self.assertRaisesRegex(RuntimeError, "Apple clang"):
                macos.preflight({"PATH": ""})

    def test_native_host_guard_prevents_build_on_linux(self):
        with patch.object(macos.platform, "system", return_value="Linux"), patch.object(macos, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "require macOS"):
                macos.preflight({"PATH": ""})
            run.assert_not_called()

    def test_preflight_selects_native_rust_host_before_building(self):
        def native_tools(args, environment, capture=False):
            return "rustc 1.99.0\nhost: aarch64-apple-darwin\n" if args[0] == "rustc" else ""

        env = {"PATH": ""}
        with patch.object(macos.platform, "system", return_value="Darwin"), \
             patch.object(macos, "require_tool", return_value=Path("/mock/tool")), \
             patch.object(macos, "guest_tools", return_value=self.tools), \
             patch.object(macos, "run", side_effect=native_tools):
            macos.preflight(env)
            self.assertEqual(env["VOLE_NATIVE_HOST"], "aarch64-apple-darwin")
            self.assertEqual(env["VOLE_LLD"], str(self.tools["ld.lld"]))
            env["CARGO_BUILD_TARGET"] = "x86_64-unknown-linux-gnu"
            with self.assertRaisesRegex(RuntimeError, "Unset CARGO_BUILD_TARGET"):
                macos.preflight(env)

    def test_missing_metal_has_actionable_error_before_cargo_build(self):
        def missing_metal(args, environment, capture=False):
            if "metal" in args:
                raise subprocess.CalledProcessError(1, args, stderr="cannot execute metal")
            return ""

        with patch.object(macos.platform, "system", return_value="Darwin"), \
             patch.object(macos, "require_tool", return_value=Path("/mock/tool")), \
             patch.object(macos, "run", side_effect=missing_metal):
            with self.assertRaisesRegex(RuntimeError, "downloadComponent MetalToolchain"):
                macos.preflight({"PATH": ""})


if __name__ == "__main__":
    unittest.main()
