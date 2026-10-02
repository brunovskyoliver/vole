# Native acceptance and release artifacts

Run the application and verify its behavior on each shipping host. Guest
simulation tests and a successful cross-platform build are necessary, but
do not establish native window behavior. Evidence for a host belongs in
`dist/native-evidence` or an equivalent retained CI artifact.

## Automated checks

`ci.yml` runs the locked workspace tests and Clippy on Linux x64, macOS
Apple Silicon, macOS Intel and Windows x64. Each host uses a source-built
LLVM 18.1.8 toolchain. The release-artifact workflow builds both native
executables, packages the tools and notices, and runs all five bundled
guest addition programs from the packaged CLI. It checks their halt state,
register result and main-memory write independently. It then uploads an
unsigned portable artifact. It does not publish a GitHub release.

Runner names were checked against GitHub's current runner table. The matrix
uses `ubuntu-24.04`, `macos-15`, `macos-15-intel` and `windows-2022`.
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

For Linux build prerequisites:

```sh
./scripts/setup-linux.sh
cargo test --workspace --locked
cargo build --release --locked -p vole-app -p vole-cli
python3 scripts/package.py
xvfb-run -a -s '-screen 0 1800x1200x24 -noreset' \
  python3 scripts/verify-native.py --start-window-manager --start-compositor --binary target/release/vole
```

Extract the Linux tarball and run its `vole` executable, or run `install.sh`
to copy the package into the current user's data directory and register its
application launcher. No administrator privileges are needed.

Use `--binary-dir target/debug` for a local debug package. For release
packages with the pinned toolchain rather than system LLVM:

```sh
python3 scripts/build-toolchain.py
python3 scripts/package.py --toolchain-dir target/toolchain
```

Use `--arch arm32`, `--arch arm64`, `--arch x86` or `--arch x64` with a
separate `--output` directory to capture each real-ISA workbench layout.
Add `--demo` to start at the two-step teaching example state. The script
waits eight seconds for asynchronous assembly and the first frame; use
`--startup-wait` to adjust this on a slower host. It captures the composed
display and crops to the client geometry reported by X11, avoiding stale
client backing pixmaps under a compositor. Keep the owned window unobscured.

The Linux native script inspects a real GPUI X11 window's normal-window
property, captures five window sizes, sends Step and Reverse shortcuts,
and captures their resulting displays. It delivers keyboard events to its
verified active window, then uses Tesseract on the displayed footer to wait
for each expected executed-instruction count before sending another action.
The sizes are 1440x940, 1280x800,
1100x760, 850x650 and 720x520. OCR confirms instruction counts; inspect
register/memory values, visual quality and layout manually.
A private Xvfb session starts Openbox and a compositor only when explicitly
requested; the verifier stops only processes it created. Missing or blank
frames fail and preserve a failure receipt. A virtual X11 display does not
verify Wayland or physical GPU behavior.

Linux native open/save dialogs use xdg-desktop-portal. The setup script
installs the portal service and GTK chooser backend, but they must run inside
a desktop D-Bus session. A bare Xvfb screenshot run does not test file dialogs.

## macOS window acceptance

The package contains `Vole.app`, with `CFBundlePackageType=APPL`, a normal
`NSApplication`, and `LSUIElement=false`. This makes the bundle eligible for
ordinary Dock, window-manager and application-switcher behavior. Verify the
running program on both Apple Silicon and Intel machines. The app and
source-built tools target macOS 13.0, and the bundle declares that minimum.
That deployment setting requires an actual macOS 13 acceptance run before
claiming macOS 13 support. CI builds currently run on macOS 15.

Open the app bundle with Finder. Check its Dock entry, Command-Tab entry,
native titlebar controls, focus after clicking, resize constraints, full
screen and reopening after its last window closes. Confirm that the main
window is an ordinary resizable window using Accessibility Inspector.
Tile it with AeroSpace, switch spaces, focus an adjacent application, return
to Vole, and resize the tile. Capture the main window and the AeroSpace
window listing. Check a MacBook-sized 1280x800 usable area and display-scale
changes. Native signing/notarization remains required before public delivery.

## Windows acceptance

Extract the portable zip and launch `vole.exe`. Optionally run `install.ps1`
to copy it into the current user's LocalAppData and create a Start-menu
shortcut. Check the taskbar and Alt-Tab entries, native caption controls,
snap layouts, monitor moves, keyboard focus and close/reopen behavior.
Test 100%, 150% and 200% scaling. If the host lacks the Microsoft C++ runtime,
install the current supported Visual C++ Redistributable before launch.
Unsigned artifacts may prompt SmartScreen; release-owner signing is not
configured by this project.

## Product acceptance on every native host

Assemble, step, reverse, run, pause, set a breakpoint and reset VOLE. Confirm
all 256 cells, source selection, instruction explanations, RF output,
register edits and self-modifying code. Then assemble and run the bundled
ARM32, ARM64, x86 and x64 examples. Check 32/64-bit address width, register
aliases, main memory, fault diagnostics and responsive pause for loops.

Save a project, close the program, reopen and restore its source/target/
breakpoints. Exercise native open/save cancellation and a path containing
spaces. Repeat the smallest supported layout; text, controls and pane
contents must remain accessible without overlap. Use keyboard-only focus,
check visible focus indicators, and capture the final screen.

| Shipping host | Native build | Five guest tests | Window/layout | Save/reopen | AeroSpace |
| --- | --- | --- | --- | --- | --- |
| Linux x64 X11 | Local build evidence required | CLI and seam tests | Native screenshot evidence required | Manual native picker check | Not applicable |
| Linux x64 Wayland | Same executable | Same engine | Physical host check required | Manual native picker check | Not applicable |
| macOS ARM64 | CI run required | Native CI run required | Physical host check required | Physical host check required | Physical host check required |
| macOS Intel | CI run required | Native CI run required | Physical host check required | Physical host check required | Physical host check required |
| Windows x64 | CI run required | Native CI run required | Physical host check required | Physical host check required | Not applicable |

Keep unperformed checks open. Neither the presence of a workflow nor an app
bundle is evidence that it has successfully run on a native host.
