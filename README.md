<img src="assets/icon.svg" width="64" height="64" alt="">

# Vole

A native GPUI workbench for assembling or compiling programs and watching their
instructions change registers and main memory.

Targets are VOLE 8-bit, ARM/AArch32, ARM64/AArch64, x86/IA-32, and x86-64/x64.
VOLE implements the supplied instruction table, including D/E/F extensions.
The real CPU targets execute documented scalar teaching subsets, not arbitrary
OS applications. They also run freestanding C compiled with Clang, with
source-line breakpoints, Step Into/Over/Out, a call stack and variables
connected to the C lines, machine code, bytes, registers and memory.

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
- [Freestanding C and source-level debugging](docs/c-environment.md)
- [Supported instructions and teaching ABI](docs/isa-support.md)
- [VOLE machine conventions](docs/vole-spec.md)
- [Native platform verification](docs/native-verification.md)

## Run

On a Mac with full Xcode and Homebrew installed:

```sh
make mac-setup
make mac-run
```

This builds and opens a native `Vole.app` for your Mac. See
[macOS development commands](docs/macos-development.md) for optimized builds,
verification and Metal toolchain setup.

Install Rust with rustup. The repository pins Rust 1.99.0 and its formatting
and lint components. On Debian/Ubuntu, `bash scripts/setup-linux.sh` installs
native build/graphics tools, LLVM MC, LLD and Clang 14. `make mac-setup`
installs Homebrew LLVM (which includes Clang) and LLD. Windows needs the MSVC
C++ build tools and Windows SDK, plus LLVM with Clang and LLD on `PATH` (for
example from the official LLVM installer) or the pinned toolchain from
`python scripts/build-toolchain.py`; Windows runtime use is not yet verified.

```sh
cargo run -p vole-app
cargo run -p vole-app -- --arch arm64 --language c
cargo run -p vole-cli -- --arch vole
cargo run -p vole-cli -- --arch arm64 --json
cargo run -p vole-cli -- --arch x64 --source examples/c/tour.c
```

VOLE and imported machine images work without LLVM. ARM/x86 source assembly
needs LLVM MC and LLD 14+ on PATH, explicit `VOLE_LLVM_MC`/`VOLE_LLD` paths,
or `VOLE_TOOLCHAIN_DIR`. C needs Clang 14+ and LLD; `VOLE_CLANG` and
`VOLE_CLANG_RESOURCE_DIR` select a specific compiler and its freestanding
headers. Portable packages bundle the assembler, linker, Clang and Clang's
resource headers. The release toolchain builder pins LLVM 18.1.8; the local
verified toolchain is LLVM/Clang 14.0.6. See the support documents for
syntax, memory layout, the C scope and limits.

### C

Choose a real target and switch the toggle beside it to **C**. The default
C example is a short tour (arithmetic, loops, recursion, arrays, pointers and
`printf`); **Examples** lists the others in `examples/c`. Compile with
Ctrl/Cmd+Enter. Click a line and press F9 for a breakpoint, then Continue
(F5). The machine code is grouped under the C line it came from, the call
stack and variables follow the selected frame, and each variable's location
chip jumps the memory grid to its bytes. Step over (F10), Step into (F11) and
Step out (Shift+F11) move by C statements; Instruction (Ctrl/Cmd+F10 or
Alt+F10) and Back (Shift+F10) move by machine instructions. Choose `-O1` to
see optimized code; values the compiler no longer keeps say why they are
unavailable. Unsupported C features are reported before compiling, with an
explanation. See [the C environment](docs/c-environment.md) for the exact
scope.

### Assembly

Choose a target, load its example, then Assemble. Step advances one instruction;
Run/Pause executes in bounded batches. Reverse step restores retained machine
state. Click an instruction gutter or press F9 to toggle a breakpoint. Memory
and registers are editable while paused, with read/write watchpoints available.

Open `.c` or `.s`/`.asm` source, `.bin`/`.prg` bytes, whitespace hexadecimal
`.hex`, or `.voleproject`/JSON projects. Projects retain source, language,
architecture, compiler settings, the built image with its debug metadata,
current registers/memory/flags/output, breakpoints and pane sizes. C projects
are saved as version 2; assembly projects stay version 1 so earlier releases
can open them.
Prior undo history is not persisted. Raw images use origin 00 for VOLE and
0x1000 for the real ISAs; save a project for other origins or entry points.

| Action | Shortcut |
| --- | --- |
| Assemble or compile | Ctrl+Enter / Cmd+Enter |
| Run / Continue / Pause | F5 |
| Step one instruction | F10 in assembly; Ctrl/Cmd+F10 or Alt+F10 in C |
| Step over / into / out (C) | F10 / F11 / Shift+F11 |
| Reverse one instruction | Shift+F10 |
| Toggle breakpoint (cursor line) | F9 |
| Reset | Ctrl+R / Cmd+R |
| Open / Save | Ctrl+O / Ctrl+S, or Cmd on macOS |
| Fullscreen | F11 in assembly, Ctrl+Shift+F (Ctrl+Cmd+F on macOS) anywhere |
| Help | F1 |

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
all five guest examples, raw-byte round trips and a compiled C program on all
four real targets through the packaged CLI. `verify-engines.py` also compiles
and runs a C program with independently computed output on every real target.
Release workflows upload artifacts without publishing releases. Public macOS
signing/notarization requires owner credentials. The source repository is
[brunovskyoliver/vole](https://github.com/brunovskyoliver/vole).

## Foundation

Rust owns architecture descriptions, assembler results, debugger state,
and deterministic traces. GPUI renders the desktop interface. Each guest uses
a project-owned interpreter; LLVM assembles the real ISAs, Clang compiles C,
gimli reads its DWARF once into a saved debug model, and Capstone decodes the
bytes. A serialized background runtime keeps compilation and runs off the UI
thread.
Platform builds and native window behavior are acceptance gates.

See the plan for the intended Cargo workspace, milestone order, scope of each
target, and checks required before release.
