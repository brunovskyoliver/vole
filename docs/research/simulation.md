# Simulation and toolchain research

Researched 2026-10-02. Facts cite primary sources. Recommendations are project
design choices; a working integration has not been demonstrated.

## Reference behavior

SimpSim is an assembler and simulator based on Brookshear's teaching machine.
Its author describes run, step, break, an assembly editor, file import/export,
examples, trace/disassembly, help, and text output. These are the workflow
requirements to carry into Vole.
[Author's project page](https://www.anne-gert.nl/projects/simpsim/).

The supplied table matches the author's instruction-help image, including
indirect load/store D/E and conditional branch F. A base-only Brookshear
implementation would miss part of the requested machine.
[Author's instruction table](https://www.anne-gert.nl/projects/simpsim/images/instructions.gif).

The author's syntax and examples use optional labels, semicolon comments,
registers R0 through RF, decimal numbers, suffix-h hex, `org`, `db`, and
strings. The output examples print through writes to RF. Preserve that as a
named compatibility profile, since it is a device convention rather than a
universal VOLE instruction rule.
[Syntax help](https://www.anne-gert.nl/projects/simpsim/images/syntax.gif),
[output example](https://www.anne-gert.nl/projects/simpsim/examples/outtest.asm),
[author's template](https://www.anne-gert.nl/projects/simpsim/examples/template.asm).

## Architecture scope

RISC and CISC are instruction-set families, not assembly languages. Offer
concrete targets and syntax modes.

| Target | Register width | Proposed initial guest scope |
| --- | --- | --- |
| VOLE / SimpSim extended | 8-bit | 16 registers, 256 byte cells, two-byte instructions |
| ARM / AArch32 | 32-bit | A32 scalar teaching programs; Thumb later |
| ARM64 / AArch64 | 64-bit, with 32-bit W views | A64 scalar teaching programs |
| x86 / IA-32 | 32-bit | Flat-address scalar programs, Intel syntax |
| x86-64 / x64 | 64-bit, with narrower views | Flat-address scalar programs, Intel syntax |

Arm's overview distinguishes A32/T32/A64, instruction sizes and register
widths. A64 instructions are 32 bits wide despite its 64-bit registers.
Target width must not be a generic toggle on a VOLE machine.
[Arm ISA overview](https://developer.arm.com/-/media/Files/pdf/graphics-and-multimedia/ARMv8_InstructionSetOverview.pdf),
[Intel architecture manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).

Host and guest architectures are separate. An Apple Silicon app must simulate
x86; a Windows x64 app must simulate ARM64. Verify each backend on every
shipping host. RISC-V is a useful future addition, not a replacement for ARM.

## Execution engines

Implement VOLE directly in Rust. Complete state capture is inexpensive. Use
one decoder for execution, disassembly, operand highlighting and help. The
reference does not settle floating-point rounding/overflow, F signedness,
reserved bits or end-of-memory fetch. Record explicit policies before
implementing those behaviors; do not borrow incompatible semantics silently.

Unicorn is the technical candidate for real ISAs. Its README advertises ARM,
ARM64, x86 16/32/64-bit, native Windows/macOS/Linux, instrumentation and Rust
bindings. The latest release inspected was 2.1.4; main's manifest says 2.1.5.
Pin a tested release and matching bindings rather than treating main as a
release.
[Unicorn README](https://github.com/unicorn-engine/unicorn),
[release 2.1.4](https://github.com/unicorn-engine/unicorn/releases/tag/2.1.4),
[Rust workspace](https://github.com/unicorn-engine/unicorn/blob/master/Cargo.toml).

The public API has memory mapping, register access, execution limits, hooks
and CPU contexts. Wrap it behind a headless worker interface. Use single
instruction execution for Step, bounded batches for Run, and hooks for
breakpoints and memory access. CPU contexts alone are not memory/device undo;
save those separately.
[API header](https://github.com/unicorn-engine/unicorn/blob/master/include/unicorn/unicorn.h).

The FAQ documents version-dependent PC accuracy, multiple memory-hook events
per instruction, cache invalidation after external code edits, and missing
syscall handlers. This is CPU simulation; provide a documented teaching
environment. Do not imply arbitrary OS applications can execute. Label
instruction counts honestly; this is not cycle-accurate hardware simulation.
[Unicorn FAQ](https://github.com/unicorn-engine/unicorn/blob/master/docs/FAQ.md).

Licensing is an adoption gate. The engine README and Rust manifest declare
GPLv2/GPL-2.0, while the public header declares LGPL2. The header does not
establish that the whole engine is permissive. Audit the exact engine/binding
versions against the GPUI dependency set and intended distribution. Process
isolation is technically useful but does not establish license compatibility.
This repository assigns no project license yet.
[Engine license](https://github.com/unicorn-engine/unicorn/blob/master/COPYING),
[Rust manifest](https://github.com/unicorn-engine/unicorn/blob/master/Cargo.toml),
[header](https://github.com/unicorn-engine/unicorn/blob/master/include/unicorn/unicorn.h).

If that gate fails, build a project-owned Rust interpreter for an explicitly
documented scalar subset of each ISA. That costs more work and is subset
support. Architecture selectors that only relabel registers are unacceptable.

## Assembly and human-readable translation

Use a two-pass VOLE assembler that returns bytes, labels, diagnostics and
source-to-address mapping.

LLVM MC is the first real-ISA assembler candidate. Its command guide documents
target triples, object output, instruction encodings, assembly DWARF and
Intel/AT&T output dialects. LLD resolves relocations. Prove labels, branches,
data references and debug mapping across all four targets in a toolchain spike.
[LLVM MC guide](https://llvm.org/docs/CommandGuide/llvm-mc.html),
[LLD](https://lld.llvm.org/).

Recommendation: emit a controlled ELF object on every host, link with a
generated guest memory layout, then parse supported load segments and debug
metadata in Rust. Dumping `.text` alone does not resolve relocations or include
data. Reject unresolved symbols, dynamic linking and unsupported relocations
with precise diagnostics. Initially accept one self-contained source document.

Ship a pinned toolchain bundle after packaging is proven. External discovery
is acceptable during development; VOLE works without LLVM. Invoke tools with
argument arrays, private temporary files, timeouts and bounded output. Never
route source text through a shell.

Capstone is a suitable decoding candidate: its README lists ARM/AArch64/x86,
operand details, register metadata, Rust bindings and a BSD license. Select a
stable engine/binding pair instead of adopting an alpha because main has newer
architecture work. Register metadata is not a complete semantic model.
[Capstone README](https://github.com/capstone-engine/capstone).

Keystone is another assembler candidate, but its README describes GPLv2 with
exceptions and commercial licensing. Prefer the LLVM spike first. Its
historical website redirected to unrelated content during research, so use
the upstream repository as the source.
[Keystone README](https://github.com/keystone-engine/keystone),
[client exception](https://github.com/keystone-engine/keystone/blob/master/EXCEPTIONS-CLIENT).

Readable translation has two layers: actual disassembly and deterministic
instruction explanations. `5312` becomes `addi R3, R1, R2`, then
`R3 = wrap8(R1 + R2)`. Show observed execution values separately. Unknown
explanations retain accurate disassembly with an unavailable state. C-like
whole-program decompilation is outside the first release.

## Runtime and memory

Use `u64` guest addresses, width-aware register values, byte-addressed memory,
explicit endianness and permissions. Show every VOLE cell. Larger guests use
bounded sparse mappings, virtualized rows, address search, typed views and
stack/code/data labels. Never allocate a full 32-bit or 64-bit address space.

One worker owns machine state. Serialize commands; tag snapshots by session,
program revision and sequence. Run uses bounded batches with Pause checks.
Edits require pause; a new load starts new history. Undo must include
overlapping writes, register aliases and simulated output, with bounded
retention. Opaque instructions can require full checkpoints or an explicit
reversal limitation. Reversal beyond retained history cannot claim success.

## Verification limits

Read the reference site, author's instruction/syntax images and examples,
upstream manifests/APIs, and official ISA/toolchain docs. No backend or
toolchain was compiled or executed. Cargo/rustc/LLVM/CMake are absent from
PATH in this Linux environment. Native window behavior and shipping-host
combinations remain implementation acceptance work.
