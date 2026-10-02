# Implementation status

The approved assembler and simulator are implemented as a Rust workspace with
a native GPUI desktop and a headless verification CLI. Higher-level language
compilation remains a later release. VOLE implements the complete supplied
instruction table. ARM32, ARM64, x86 and x64 execute actual bytes using the
[documented scalar instruction subsets](isa-support.md).

## Delivered behavior

The dark workbench connects editable assembly, machine bytes, readable
instructions, operation explanations, registers, flags, main memory, history
and simulated output. It supports Assemble, Step, Run/Pause, Reset, reverse
step on all five guests, breakpoints and memory read/write watchpoints.
Source edits disable execution until reassembly. Paused register/flag/memory
edits clear undo history. Self-modification updates live disassembly; Reset
restores the initial image and its source associations.

Projects preserve source, target, image, current machine state, breakpoints
and pane sizes. Source-only documents and raw-byte/hex imports are
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

The final locked workspace passes 74 tests, strict Clippy and formatting.
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

The repository is published at [brunovskyoliver/vole](https://github.com/brunovskyoliver/vole).
[Makefile commands](macos-development.md) build and launch a local macOS app
bundle. Signing and notarization remain owner-controlled release steps.
Full ISA support, privileged/OS execution,
SIMD, cycle timing and higher-level compilation are outside this release.
