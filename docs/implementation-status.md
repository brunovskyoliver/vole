# Implementation status

The approved assembler and simulator are implemented as a Rust workspace with
a native GPUI desktop and a headless verification CLI. VOLE implements the
complete supplied instruction table. ARM32, ARM64, x86 and x64 execute actual
bytes using the [documented scalar instruction subsets](isa-support.md), and
run [freestanding C compiled with Clang](c-environment.md) with source-level
debugging.

## C compilation and source debugging

`vole-c` compiles one C document with Clang for `aarch64-none-elf`,
`x86_64-none-elf`, `armv7a-none-eabi` or `i386-none-elf`, links it with
startup code and a small teaching runtime using LLD and a fixed memory map,
and converts DWARF 5 line, variable, type and call-frame information into a
saved debug model with gimli. Unsupported features (floating point, inline
assembly, host includes and similar) are reported before Clang runs, with
explanations; Clang and LLD errors are mapped to document lines and columns.
Compilation is bounded and runs on the runtime worker thread.

The interpreters were extended to every instruction Clang 14 emits for that
scope at `-O0` and `-O1`, and decoding was cached, making stepping 20–45 times
faster. Faults remain atomic, writes to code and constant data fault, and
reverse execution covers every new instruction.

`vole-debug` unwinds frames with the saved CFI, evaluates variable locations,
formats values by type and explains unavailable values. The runtime adds
source-line breakpoints and bounded, pausable Step Into, Step Over and Step
Out. The workbench adds a C mode: editor, machine code grouped by C line with
bytes, call stack, variables with location chips linked to memory, registers,
output, clickable diagnostics and C syntax highlighting, in wide and compact
layouts. C projects (version 2) save source, target, compiler settings,
source breakpoints, the image with its debug model and machine state;
version 1 assembly projects still open and assembly projects are still
written as version 1. `vole-cli` compiles and debugs C from the command
line. Toolchain builds, packages, macOS bundles and CI include Clang and its
resource headers.

## Delivered behavior

The dark workbench connects editable assembly, machine bytes, readable
instructions, operation explanations, registers, flags, main memory, history
and simulated output. It supports Assemble, Step, Run/Pause, Reset, reverse
step on all five guests, breakpoints and memory read/write watchpoints.
Source edits disable execution until reassembly. Paused register/flag/memory
edits clear undo history. Self-modification updates live disassembly; Reset
restores the initial image and its source associations.

Projects preserve source, target, image, current machine state and
breakpoints. The workbench layout (hidden, moved and resized panels, one
layout per language) and text size are user preferences saved in
`workbench.json`, not part of a project; older projects' pane sizes are kept
and written back unchanged. Source-only documents and raw-byte/hex imports are
also supported. Project writes use a temporary file and atomic replacement.
Image/source mismatches are rejected. Exported raw bytes use a documented
fixed origin; projects preserve custom origins and entry points.

Original examples cover sum, loops/calls and text output for every real ISA,
plus VOLE extensions and floating point. Portable packages include the LLVM
assembler/linker, dependency notices, examples, documentation and the CLI.
The release workflows prepare artifacts without publishing releases.

## Implementation decisions

Project-owned interpreters replaced the proposed Unicorn backend at the
license/distribution gate. The `Machine` interface keeps execution independent
of GPUI. A serialized background runtime thread owns the machines. Bounded
run batches, tool deadlines, mapping/output limits and atomic instruction
faults keep guest execution controlled and reversible.

Published GPUI/Platform 0.3.7 and GPUI Kit 0.7.0 are pinned in Cargo.lock;
Rust 1.99.0 is pinned in rust-toolchain.toml. IBM Plex fonts are bundled with
their OFL notices. The Linux backend contains a narrow
[documented X11 visual patch](../vendor/gpui-pre-linux/VOLE-PATCH.md).
On this host, the upstream 32-bit X11 visual produced blank frames under
Vulkan; selecting the opaque 24-bit visual for the app's normal, opaque native
titlebar window restored rendering. A minimal native probe isolated the cause
before the workbench was tested. Wayland had rendered successfully before
the patch.

## Host acceptance

The C release adds the tests listed in [verification evidence](verification/README.md#c-compilation-and-source-debugging).
The previous assembler release passed 74 tests, strict Clippy and formatting.
See [verification evidence](verification/README.md) for test output, native
screenshots, interaction observations and the separate review axes.

Linux x64 development and packaged binaries are exercised on the local host.
Native screenshot and interaction evidence is collected from GPUI windows,
not the original design SVG. The verification environment uses X11/Openbox,
Xvfb and Mesa llvmpipe; Wayland rendering was also checked under Weston.
This does not establish compatibility with every hardware Vulkan driver.

macOS Apple Silicon/Intel and Windows x64 builds have native packaging and
CI jobs prepared. They have **not** been built or launched on those hosts in
this Linux environment. The macOS normal window, real traffic lights, native
menus, Cmd shortcuts and Dock reopen behavior are implemented, but Retina,
fullscreen, accessibility classification and live AeroSpace tiling still need
the [native acceptance checks](native-verification.md). Windows DPI scaling,
native controls and install/launch/save/reopen also need host verification.
Linux ARM64 and Windows ARM64 host packages are not part of this first matrix.

For C, the native checks performed are Linux x64 only: the GPUI workbench in
C mode on all four guest targets under X11/Xvfb, and the CLI and engine
verification with the host's Clang/LLD 14.0.6. Clang inside the pinned LLVM
18.1.8 toolchain build, the bundled resource headers, Homebrew Clang in the
macOS bundle and Clang on Windows are wired into the scripts and CI but have
**not** been run in this environment; CI will exercise them on its macOS and
Windows runners. Guest behavior itself is host-independent and covered by the
fixture and corpus tests.

The repository is published at [brunovskyoliver/vole](https://github.com/brunovskyoliver/vole).
[Makefile commands](macos-development.md) build and launch a local macOS app
bundle. Signing and notarization remain owner-controlled release steps.
Full ISA support, privileged/OS execution, SIMD, floating point, cycle
timing, a C heap or input, and languages other than C are outside this release.
