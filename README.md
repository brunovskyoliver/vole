<img src="assets/icon.svg" width="64" height="64" alt="">

# Vole

A native GPUI workbench for assembling programs and watching their
instructions change registers and main memory.

Targets are VOLE 8-bit, ARM/AArch32, ARM64/AArch64, x86/IA-32, and x86-64/x64.
VOLE implements the supplied instruction table, including D/E/F extensions.
The real CPU targets execute documented scalar teaching subsets, not arbitrary
OS applications. Higher-level compilation is a later extension.

## Project status

The Rust workspace, GPUI desktop, headless CLI, assemblers, interpreters,
debugger, projects, examples, and packaging are implemented. Native Linux
visual and interaction verification passed locally. macOS and Windows build
workflows are configured; their native runtime checks are still unverified.

- [Implementation status and remaining host checks](docs/implementation-status.md)
- [Native screenshots and verification evidence](docs/verification/README.md)
- [Implementation plan](docs/implementation-plan.md)
- [Dark workbench design](docs/design/workbench.md)
- [Static visual proposal](docs/design/vole-workbench.svg)
- [Rendered visual preview](docs/design/vole-workbench.png)
- [Simulation and toolchain research](docs/research/simulation.md)
- [GPUI and platform research](docs/research/gpui-platforms.md)
- [Supported instructions and teaching ABI](docs/isa-support.md)
- [VOLE machine conventions](docs/vole-spec.md)
- [Native platform verification](docs/native-verification.md)

## Run

Install Rust with rustup. The repository pins Rust 1.99.0 and its formatting
and lint components. On Debian/Ubuntu, `bash scripts/setup-linux.sh` installs
native build/graphics tools and LLVM. macOS needs Xcode and its command-line
tools; Windows needs the MSVC C++ build tools and Windows SDK.

```sh
cargo run -p vole-app
cargo run -p vole-cli -- --arch vole
cargo run -p vole-cli -- --arch arm64 --json
```

VOLE and imported machine images work without LLVM. ARM/x86 source assembly
needs LLVM MC and LLD 14+ on PATH, explicit `VOLE_LLVM_MC`/`VOLE_LLD` paths,
or `VOLE_TOOLCHAIN_DIR`. Portable packages bundle their assembly tools. The
release toolchain builder pins LLVM 18.1.8; the local verified toolchain is
LLVM 14.0.6. See the support document for syntax, memory layout, and limits.

Choose a target, load its example, then Assemble. Step advances one instruction;
Run/Pause executes in bounded batches. Reverse step restores retained machine
state. Click an instruction gutter or press F9 to toggle a breakpoint. Memory
and registers are editable while paused, with read/write watchpoints available.

Open `.s`/`.asm` source, `.bin`/`.prg` bytes, whitespace hexadecimal `.hex`,
or `.voleproject`/JSON projects. Projects retain source, architecture, assembled
image, current registers/memory/flags/output, breakpoints and pane sizes.
Prior undo history is not persisted. Raw images use origin 00 for VOLE and
0x1000 for the real ISAs; save a project for other origins or entry points.

| Action | Shortcut |
| --- | --- |
| Assemble | Ctrl+Enter / Cmd+Enter |
| Run / Pause | F5 |
| Step | F10 |
| Reverse step | Shift+F10 |
| Toggle breakpoint | F9 |
| Reset | Ctrl+R / Cmd+R |
| Open / Save | Ctrl+O / Ctrl+S, or Cmd on macOS |
| Fullscreen / Help | F11 / F1 |

## Verify and package

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
cargo build -p vole-cli -p vole-app
python3 scripts/verify-engines.py
```

`scripts/verify-native.py` captures actual X11 windows at wide and laptop sizes,
rejects blank render frames, and records Step/Reverse screenshots for review.
These captures test GPUI itself; the design SVG is only the original proposal.
Native macOS acceptance includes the installed `.app`, Dock reopen, fullscreen,
accessibility classification and observed AeroSpace tiling with neighboring
window geometry. CI compilation alone does not establish those behaviors.

```sh
python3 scripts/build-toolchain.py --prefix dist/llvm-toolchain
python3 scripts/package.py --toolchain-dir dist/llvm-toolchain
```

The packager builds a native portable archive, bundles tools/notices, and runs
all five guest examples and raw-byte round trips through the packaged CLI.
Release workflows upload artifacts without publishing releases. Public macOS
signing/notarization requires owner credentials. No remote repository or PR
has been created for this local project.

## Foundation

Rust owns architecture descriptions, assembler results, debugger state,
and deterministic traces. GPUI renders the desktop interface. Each guest uses
a project-owned interpreter; LLVM assembles the real ISAs and Capstone decodes
their bytes. A serialized background runtime keeps runs off the UI thread.
Platform builds and native window behavior are acceptance gates.

See the plan for the intended Cargo workspace, milestone order, scope of each
target, and checks required before release.
