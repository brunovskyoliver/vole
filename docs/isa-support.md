# Guest architectures and teaching environment

Vole assembles and simulates VOLE, ARM32, ARM64/AArch64, x86 and x64/x86-64.
The four real targets also run freestanding C compiled with Clang; see
[C and source-level debugging](c-environment.md) for that scope.
Guest architecture is independent of the computer running the application.
The real CPU targets execute a documented scalar teaching subset. They do not
boot an operating system or execute arbitrary application binaries.

## Backend decision

The release uses project-owned Rust interpreters and the Capstone decoder.
Unicorn is not included. This avoids making a distribution decision around
Unicorn's GPL-covered core and GPUI. The execution seam remains the `Machine`
trait, so a different backend can be added without changing the workbench.
The subset is smaller than a full CPU emulator, but each target executes
actual machine bytes. Source text and precomputed disassembly are optional
metadata. Edits to executable memory change subsequent instruction execution.
Unsupported instructions fault at their address and leave machine state intact.

Capstone 0.14.0 and capstone-sys 0.18.0 are pinned in Cargo.lock. Only ARM,
ARM64 and x86 decoder backends are enabled. The Rust binding is MIT licensed;
Capstone's core is BSD licensed. LLVM/LLD are separate assembly executables
under the Apache 2.0 license with LLVM exceptions. See the release dependency
notices for redistribution details. No guest code runs on the host CPU.

## Verified instruction coverage

The table lists instructions exercised through the public assembler and
machine interface, with independently specified expected registers, flags,
bytes and memory. The assembler accepts more LLVM syntax than the interpreter
executes. Successfully assembling an instruction does not establish runtime
support for every encoding or operand form.

| Target | Verified operations | Verified architecture details |
| --- | --- | --- |
| VOLE 8-bit | See [VOLE conventions](vole-spec.md) | Sixteen byte registers; 256-byte memory; supplied D/E/F extensions |
| ARM32 A32 | MOV/MVN(S), MOVW/MOVT, ADD/ADC/SUB/SBC/RSB/RSC(S), CMP/CMN/TST/TEQ, AND/ORR/EOR/BIC(S), LSL/LSR/ASR/ROR/RRX(S), MUL(S)/MLA/MLS, UMULL/SMULL/UMLAL/SMLAL(S)/UMAAL, SMMUL(R)/SMMLA/SMMLS, SMULxy/SMLAxy, SDIV/UDIV, CLZ/REV/REV16/REVSH/RBIT, UXTB/UXTH/SXTB/SXTH (with rotation), UXTAB/UXTAH/SXTAB/SXTAH, UBFX/SBFX/BFI/BFC, LDR/LDRB/LDRH/LDRSB/LDRSH/LDRD, STR/STRB/STRH/STRD, LDM/STM (IA/IB/DA/DB), PUSH/POP, B/BL/BX/BLX (register), MRS APSR, MSR APSR_nzcvq, BKPT, SVC, NOP | Conditional execution of every instruction, PC + 8 reads, PC-writing data processing and loads (jump tables), shifter carry for immediate and register shifts (including counts of 32 or more), 64-bit carry chains, pre/post-index and negative register offsets, writeback, divide-by-zero returns 0, NZCV |
| ARM64 A64 | MOV/MOVZ/MOVN/MOVK/MVN, ADD/ADDS/SUB/SUBS/ADC/SBC(S)/NEG(S)/NGC(S), CMP/CMN/TST/CCMP/CCMN, AND/ORR/EOR/BIC/ORN/EON(S), LSL/LSR/ASR/ROR (immediate and register), MUL/MNEG/MADD/MSUB, SMULL/UMULL/SMADDL/UMADDL/SMSUBL/UMSUBL/SMNEGL/UMNEGL, SMULH/UMULH, SDIV/UDIV, CSEL/CSINC/CSINV/CSNEG/CSET/CSETM/CINC/CINV/CNEG, UBFX/SBFX/UBFIZ/SBFIZ/BFI/BFXIL/BFC/UBFM/SBFM/BFM/EXTR, SXTB/SXTH/SXTW/UXTB/UXTH, REV/REV16/REV32/RBIT/CLZ/CLS, ADR/ADRP, LDR/LDUR{B,H,SB,SH,SW}, STR/STUR{B,H}, LDP/STP/LDPSW, B/B.cond/BL/BR/BLR/RET, CBZ/CBNZ/TBZ/TBNZ, BRK, SVC | W writes zero the upper half, XZR/WZR, SP, shifted and extended register operands, register-offset addressing with LSL/UXTW/SXTW, pre/post-index writeback, literal loads, division by zero gives 0 and MIN / −1 gives MIN, NZCV |
| x86 IA-32 | MOV, MOVZX/MOVSX, LEA, XCHG, PUSH/POP (register, immediate, memory), LEAVE, ADD/ADC/SUB/SBB/CMP, AND/OR/XOR/TEST, INC/DEC/NEG/NOT, SHL/SHR/SAR/ROL/ROR, SHLD/SHRD, MUL/IMUL (one, two and three operands), DIV/IDIV, CBW/CWDE/CWD/CDQ, BSWAP, BT/BTS/BTR/BTC, BSF/BSR, SETcc, CMOVcc, Jcc/JMP/CALL (immediate, register, memory), RET (with immediate), LOOP, REP MOVS/STOS, STC/CLC/CMC/CLD, NOP (multi-byte), INT3, HLT, INT 0x80 | 8/16/32-bit aliases and AH–DH, CF/PF/AF/ZF/SF/OF for arithmetic, carry chains, negation, multiply and shifts, #DE divide errors fault atomically, repeated string operations run one element per step, jump tables through memory |
| x64 x86-64 | x86 operations plus MOVABS, MOVSXD, CDQE, CQO, SYSCALL | 32-bit writes zero the upper half (including CMOVcc with a false condition), 8/16-bit writes preserve other bits, 128-bit dividends, RIP-relative addressing, RCX/R11 syscall saves |

Instructions outside this table fault with an Unsupported message at their
address. The subset covers every instruction Clang 14 emits for the
[documented C scope](c-environment.md) at `-O0` and `-O1` on all four
targets, which `crates/vole-isa-scalar/tests/compiler_corpus.rs` verifies by
compiling and running a C corpus. Each instruction is decoded by Capstone
before the corresponding Rust handler executes. Decoded instructions are
cached per address while their bytes are unchanged, so self-modifying code
still decodes the new bytes.

Explanations describe decoded operations and observed state changes. An
instruction without a specific explanation says so. Explanations are not
higher-level decompilation, original C recovery or cycle-accurate timing.

## Memory and execution

All targets use little-endian memory. Scalar programs use this initial layout:

| Region | Address range | Permission and purpose |
| --- | --- | --- |
| Program | 0x1000 to at most 0x1FFF | Executable and editable code, at most 4096 bytes from LLVM |
| Main memory | 0x2000 to 0xFFFF | Writable data, initialized from .data/.rodata/.bss |
| Stack | 0x10000 to 0x1FFFF | Writable stack; initial SP is 0x20000 |

Guest mappings are bounded to four MiB total, checked for overlap and address
overflow. Reading an unmapped address faults without allocating memory.
Instruction fetch requires an executable mapping. A32/A64 instruction and
branch addresses must be four-byte aligned. Thumb is outside this release.
Code is editable for self-modifying teaching programs; source-line association
is retained only while an instruction's bytes match its assembled image.

One instruction is atomic. A fault after the first half of a pair store or
a stack update restores all prior memory, registers, flags, PC and output.
Main-memory reads and writes are included in the trace for watchpoints.
Instruction-fetch reads are excluded. Reset reloads the initial program.
Reverse step restores the complete retained instruction state on every target.
History retains at most 4096 instructions and approximately two MiB of deltas.
Older entries expire. Machine edits clear history. Output is capped at one MiB.
A saved snapshot restores the bounded guest state but clears old undo history.

Writes to a read-only region (C code and constant data) fault before any
state changes. ARM alignment checks and the ARM Q/GE flags are not modelled.
Constrained-unpredictable encodings, such as writeback onto the value register
or LDM with the base in the list, fault.

Undefined x86 flags retain a deterministic prior value, except flags explicitly
set by the supported instruction. Flat addressing excludes segment overrides,
address-size overrides and virtual-memory/privilege control. ARM status-control
instructions, exception-return forms that write PC and flags, SIMD, floating
point, atomics and exclusive accesses are outside the scalar model. `REP MOVS`
and `REP STOS` (emitted by Clang for struct copies) execute one element per
step, as interruptible hardware does, so every element is reversible and
budgeted; they are never host loops. `REPNE`, `CMPS`, `SCAS` and `LODS` fault.

## Teaching output and stop conventions

ARM `bkpt #0` and `brk #0`, and x86 `int3`/`hlt`, halt normally. ARM
`bkpt #1` or `brk #1` appends the low byte of R0 or X0 to output and continues.
Other ARM breakpoint numbers fault. These are application teaching conventions,
not architectural breakpoint services.

The app also recognizes a deliberately small Linux-style syscall ABI. It
implements stdout writes and exit internally, without invoking host syscalls.

| Target | Trap | Write number | Exit number | Number register | Write arguments |
| --- | --- | --- | --- | --- | --- |
| ARM32 | `svc #0` | 4 | 1 | R7 | R0 = 1, R1 = address, R2 = length |
| ARM64 | `svc #0` | 64 | 93 | X8 | X0 = 1, X1 = address, X2 = length |
| x86 | `int 0x80` | 4 | 1 | EAX | EBX = 1, ECX = address, EDX = length |
| x64 | `syscall` | 1 | 60 | RAX | RDI = 1, RSI = address, RDX = length |

Write returns the byte count in register 0/EAX/RAX. Exit halts. Exit status is
available in the original argument register but is not a process exit status.
Other calls and descriptors fault, with no file or network access.

## Assembly toolchain

The current verified combination is LLVM MC 14.0.6 and LLD 14.0.6 on Linux x64.
Install both tools for real-ISA assembly. VOLE and previously assembled raw
machine images do not need LLVM. The tool search accepts:

- `VOLE_LLVM_MC` and `VOLE_LLD`, each an explicit executable path.
- `VOLE_TOOLCHAIN_DIR`, a directory containing `llvm-mc` and `ld.lld`.
- A `toolchain` directory beside the application executable, or macOS bundle
  `Contents/Resources/toolchain`.
- Versioned or unversioned executable names on PATH, with `.exe` on Windows.

The loader uses `armv7-none-eabi`, `aarch64-none-elf`, `i386-none-elf` or
`x86_64-none-elf`. ELF is a guest container on every host, including macOS and
Windows. LLVM MC assembles to an object, LLD resolves labels/relocations using
a fixed memory script, and the Rust `object` crate loads sections and symbols.
Private per-instruction symbols supply source line mapping after linking.
ARM32 assembly begins in ARMv7-A A32 mode. No host linker or shell is used.

Source is limited to 256 KiB. Each external tool has a five-second deadline
and bounded image/diagnostic output. Commands use argument arrays, closed
stdin and private temporary directories. `.include`, `.incbin`, macros,
repetition and unbounded allocation directives are rejected. `.text`, `.data`,
`.rodata`, `.bss`, bounded alignment/storage, labels, constants and ordinary
literal directives are available. `_start` is the entry symbol. LLVM diagnostics
are remapped to the user's original line numbers.

Use unified ARM assembly, or Intel x86 syntax with `.intel_syntax noprefix`.
In Intel syntax, `mov ecx, offset message` loads the address;
`mov ecx, message` loads memory at that symbol. ARM literal pools are resolved
by LLVM; keep data/literal bytes out of paths that execute as instructions.

## Examples and verification

Every target directory has original `sum.s`, `loop-and-call.s` and `hello.s`
programs. Sum and loop programs write decimal 125 at 0x2000 and halt. Hello
prints `Vole!` followed by a newline using the teaching syscall ABI.

Run `cargo test -p vole-isa-scalar`. It checks all shipped scalar samples and
independent first-instruction byte fixtures, arithmetic/flag results, aliases,
branches, PC behavior, sparse mappings, atomic faults, self-modification,
reversal, reset, saved snapshots, ELF symbols and bounded diagnostics.
`tests/compiler_fixtures.rs` adds 30 tests with about 115 cases whose expected
registers, flags and memory were computed by hand from the Arm and Intel
manuals, including division by zero, `INT_MIN / -1`, 64-bit carries on 32-bit
targets and conditional selects. Every case also reverses all of its steps
and compares with the initial state, and atomic-fault cases cover pair,
multiple and repeated stores into read-only memory, #DE and `xchg`.
`tests/compiler_corpus.rs` compiles five C programs for all four targets at
`-O0` and `-O1` and compares about 2.4 million executed instructions' results
with independent Rust models; it skips with a message when Clang is absent.
Native runtime verification on macOS and Windows remains separate from these
host-independent guest fixtures.

Primary references: [LLVM MC command guide](https://llvm.org/docs/CommandGuide/llvm-mc.html),
[LLD ELF linker scripts](https://lld.llvm.org/ELF/linker_script.html),
[Capstone Rust API and license](https://docs.rs/capstone/0.14.0/capstone/),
[Arm architecture references](https://developer.arm.com/documentation), and
[Intel software developer manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html).
