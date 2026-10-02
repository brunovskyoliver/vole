#!/usr/bin/env python3
"""Capture evidence from an actual GPUI X11 window, including laptop layouts."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys
import time


def run(*args):
    return subprocess.run(list(map(str, args)), check=True, capture_output=True,
                          text=True, timeout=10).stdout.strip()


def geometry(window):
    text = run("xwininfo", "-root") if window == "root" else run("xwininfo", "-id", window)
    fields = {}
    for field in ["Absolute upper-left X", "Absolute upper-left Y", "Width", "Height"]:
        match = re.search(r"^\s*" + re.escape(field) + r":\s*(-?\d+)", text, re.MULTILINE)
        if not match:
            raise RuntimeError(f"Cannot read {field} from X11 window geometry")
        fields[field] = int(match.group(1))
    return fields


def capture(window, destination):
    # The client backing pixmap may be stale under a compositor. Capture the
    # actual displayed root surface and crop its client rectangle instead.
    run("xdotool", "windowraise", window)
    run("xdotool", "windowfocus", "--sync", window)
    position = geometry(window)
    screen = geometry("root")
    width, height = position["Width"], position["Height"]
    x, y = position["Absolute upper-left X"], position["Absolute upper-left Y"]
    if width >= screen["Width"] or height >= screen["Height"]:
        raise RuntimeError("Requested window does not fit the display; use the documented 1800x1200 Xvfb screen")
    if x < 0 or y < 0 or x + width > screen["Width"] or y + height > screen["Height"]:
        run("xdotool", "windowmove", "--sync", window, 0, 0)
        time.sleep(0.3)
        position = geometry(window)
        x, y = position["Absolute upper-left X"], position["Absolute upper-left Y"]
    rectangle = f"{width}x{height}+{x}+{y}"
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        run("import", "-window", "root", "-crop", rectangle, "+repage", destination)
        deviation = float(run("identify", "-format", "%[fx:standard_deviation]", destination))
        colors = int(run("identify", "-format", "%k", destination))
        mean = float(run("identify", "-format", "%[fx:mean]", destination))
        # A stale smaller frame can leave most of the newly resized client
        # white. A complete dark workbench must fill the captured rectangle.
        if deviation > 0.003 and colors > 16 and mean < 0.3:
            return
        time.sleep(0.5)
    raise RuntimeError(f"Window capture stayed blank: {destination}")


def stop_child(process):
    """Stop exactly the subprocess instance created by this verifier."""
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def keyboard(window, *keys):
    # XSendEvent (--window) is ignored by GPUI's normal keyboard path. XTest
    # delivers desktop keyboard events; verify focus belongs to our window.
    run("xdotool", "windowactivate", "--sync", window)
    if run("xdotool", "getactivewindow") != str(window):
        raise RuntimeError("Cannot send keyboard actions: the owned GPUI window is not active")
    run("xdotool", "key", "--delay", "150", *keys)


def capture_instruction_count(window, destination, expected):
    # Check displayed machine state rather than treating incidental pixel
    # changes or delivery of a key event as evidence that execution occurred.
    deadline = time.monotonic() + 30
    footer = destination.parent / "footer-ocr.png"
    observed = ""
    while time.monotonic() < deadline:
        capture(window, destination)
        run("convert", destination, "-crop", "1440x34+0+906", "+repage",
            "-resize", "300%", "-colorspace", "gray", footer)
        ocr_environment = os.environ.copy()
        ocr_environment["OMP_THREAD_LIMIT"] = "1"
        observed = subprocess.run(["tesseract", str(footer), "stdout", "--psm", "7"],
                                  env=ocr_environment, check=True, capture_output=True,
                                  text=True, timeout=10).stdout.strip()
        # The twelve-pixel mono numeral 5 is sometimes read as uppercase S.
        # Normalize common OCR digit confusions and retain the original text.
        counts = [count.translate(str.maketrans({"S": "5", "O": "0", "I": "1", "L": "1", "l": "1"}))
                  for count in re.findall(r"\b([0-9SOILl]+)\s*instructions\b", observed)]
        if str(expected) in counts:
            return {"expected_instruction_count": expected,
                    "observed_instruction_count": expected,
                    "observed_footer": observed, "screenshot": destination.name}
        time.sleep(0.5)
    raise RuntimeError(f"Display did not reach {expected} executed instructions: {observed!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/vole"))
    parser.add_argument("--output", type=Path, default=Path("dist/native-evidence"))
    parser.add_argument("--arch", choices=["vole", "arm32", "arm64", "x86", "x64"], default="vole")
    parser.add_argument("--demo", action="store_true",
                        help="Start at the built-in two-step teaching example state")
    parser.add_argument("--startup-wait", type=float, default=8,
                        help="Seconds to allow asynchronous assembly and the first frame (default: 8)")
    parser.add_argument("--start-window-manager", action="store_true",
                        help="Start Openbox if a private Xvfb display has no window manager")
    parser.add_argument("--start-compositor", action="store_true",
                        help="Start xcompmgr on a private Xvfb display; omit on an existing desktop")
    args = parser.parse_args()
    if not 0 <= args.startup_wait <= 60:
        parser.error("--startup-wait must be between zero and 60 seconds")
    if sys.platform != "linux":
        parser.error("This automation verifies X11 on Linux. Follow docs/native-verification.md for macOS/Windows.")
    binary = args.binary.resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        parser.error(f"Missing executable {binary}; build the native application first")
    tools = ["xdotool", "xprop", "xwininfo", "import", "identify", "convert", "tesseract"]
    if args.start_window_manager:
        tools.append("openbox")
    if args.start_compositor:
        tools.append("xcompmgr")
    for tool in tools:
        if not shutil.which(tool):
            parser.error(f"Missing {tool}; run scripts/setup-linux.sh")
    if not os.environ.get("DISPLAY"):
        parser.error("No X11 display. Run with xvfb-run -a -s '-screen 0 1800x1200x24 -noreset'.")
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "verification.json").unlink(missing_ok=True)
    with binary.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    processes = []
    logs = []

    def start(command, log_name, environment=None):
        log = (args.output / log_name).open("w")
        logs.append(log)
        process = subprocess.Popen(command, stdout=log, stderr=log, env=environment)
        processes.append(process)
        return process

    try:
        if args.start_window_manager:
            property_text = run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK")
            if "not found" in property_text or "no such atom" in property_text:
                manager = start(["openbox"], "window-manager.log")
                for _ in range(50):
                    if manager.poll() is not None:
                        raise RuntimeError("Openbox exited during startup; inspect window-manager.log")
                    property_text = run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK")
                    if "not found" not in property_text and "no such atom" not in property_text:
                        break
                    time.sleep(0.1)
                else:
                    raise RuntimeError("Openbox did not register a window manager within five seconds")
        if args.start_compositor:
            compositor = start(["xcompmgr", "-a"], "compositor.log")
            time.sleep(0.3)
            if compositor.poll() is not None:
                raise RuntimeError("xcompmgr exited during startup; omit --start-compositor on an existing composited desktop")
        environment = os.environ.copy()
        environment.pop("WAYLAND_DISPLAY", None)
        bundled_tools = (binary.parent / "toolchain").is_dir()
        if bundled_tools:
            for variable in ["VOLE_LLVM_MC", "VOLE_LLD", "VOLE_TOOLCHAIN_DIR"]:
                environment.pop(variable, None)
        command = [str(binary), "--arch", args.arch]
        if args.demo:
            command.append("--visual-demo")
        process = start(command, "application.log", environment)
        # /proc binds evidence to the running image even if Cargo replaces the pathname.
        try:
            with Path(f"/proc/{process.pid}/exe").open("rb") as stream:
                checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        except OSError:
            pass
        window = None
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(f"GPUI exited with {process.returncode}; inspect application.log")
            search = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(process.pid)],
                                    capture_output=True, text=True, timeout=10)
            if search.returncode == 0 and search.stdout.strip():
                window = search.stdout.splitlines()[0]
                break
            time.sleep(0.2)
        if not window:
            raise RuntimeError("No visible native GPUI window appeared within 30 seconds")
        properties = run("xprop", "-id", window, "_NET_WM_WINDOW_TYPE", "WM_CLASS", "_NET_WM_PID",
                         "WM_NAME", "WM_PROTOCOLS", "WM_TRANSIENT_FOR")
        (args.output / "window-properties.txt").write_text(properties + "\n")
        window_geometry = run("xwininfo", "-id", window)
        def unset(name):
            return any(line.startswith(name) and ("not found" in line or "no such atom" in line)
                       for line in properties.splitlines())
        implicit_normal = unset("_NET_WM_WINDOW_TYPE") and unset("WM_TRANSIENT_FOR")
        if "Override Redirect State: no" not in window_geometry or not ("_NET_WM_WINDOW_TYPE_NORMAL" in properties or implicit_normal):
            raise RuntimeError("GPUI window is not a normal managed application window")
        run("xdotool", "windowfocus", "--sync", window)
        time.sleep(args.startup_wait)
        captures = []
        for width, height in [(1440, 940), (1280, 800), (1100, 760), (850, 650), (720, 520)]:
            run("xdotool", "windowsize", "--sync", window, width, height)
            time.sleep(1)
            filename = f"workbench-{width}x{height}.png"
            capture(window, args.output / filename)
            window_geometry = run("xwininfo", "-id", window)
            (args.output / f"geometry-{width}x{height}.txt").write_text(window_geometry + "\n")
            captures.append({"requested_width": width, "requested_height": height, "screenshot": filename})
        run("xdotool", "windowsize", "--sync", window, 1440, 940)
        time.sleep(1)
        initial_steps = 2 if args.demo else 0
        observations = [capture_instruction_count(window, args.output / "before-steps.png", initial_steps)]
        for step in range(initial_steps + 1, initial_steps + 4):
            keyboard(window, "F10")
            filename = "after-three-steps.png" if step == initial_steps + 3 else f"after-step-{step}.png"
            observations.append(capture_instruction_count(window, args.output / filename, step))
        keyboard(window, "shift+F10")
        observations.append(capture_instruction_count(window, args.output / "after-reverse.png", initial_steps + 2))
        if process.poll() is not None:
            raise RuntimeError("Application exited during resize/step checks")
        result = {"platform": "linux-x11", "binary": str(binary), "binary_sha256": checksum,
                  "architecture": args.arch, "time_utc": datetime.now(timezone.utc).isoformat(),
                  "demo_initial_steps": 2 if args.demo else 0,
                  "startup_wait_seconds": args.startup_wait,
                  "bundled_toolchain_environment": bundled_tools,
                  "success": True, "normal_window": True, "captures": captures,
                  "capture_method": "Composed root surface cropped to xwininfo absolute client geometry, with owned window raised",
                  "keyboard_actions_sent": ["F10", "F10", "F10", "Shift+F10"],
                  "keyboard_delivery": "XTest events after owned-window activation and active-window ID verification",
                  "keyboard_observations": observations,
                  "visual_review": "Required: inspect screenshots for clipping, pane alignment, text contrast and state changes.",
                  "limits": "Virtual X11 display. Physical GPU, Wayland, native file dialogs and other OSes need separate acceptance."}
        (args.output / "failure.json").unlink(missing_ok=True)
        (args.output / "verification.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    except Exception as error:
        failure = {"platform": "linux-x11", "binary": str(binary), "binary_sha256": checksum,
                   "architecture": args.arch, "time_utc": datetime.now(timezone.utc).isoformat(),
                   "success": False, "error": str(error)}
        (args.output / "verification.json").unlink(missing_ok=True)
        (args.output / "failure.json").write_text(json.dumps(failure, indent=2) + "\n")
        raise
    finally:
        for process in reversed(processes):
            stop_child(process)
        for log in logs:
            log.close()


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"Native verification failed: {error}", file=sys.stderr)
        sys.exit(1)
