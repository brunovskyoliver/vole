# Freestanding C and source-level debugging

Vole compiles one C document for a guest CPU, links it with a small teaching
runtime and runs the machine code on the same interpreters used for assembly.
Compiler debug information connects each C line to its instructions, bytes,
registers and memory. This page is the exact supported scope.

## Pipeline

```text
main.c ───────clang -c──▶ main.o ──┐
vole_runtime.c ─clang -c──▶ rt.o ──┼─ ld.lld (fixed script) ─▶ guest ELF ─▶ Program + DebugInfo
crt0_<arch>.s ──clang -c──▶ crt0.o ┘
```

`vole_c::compile` writes the document as `main.c` in a private temporary
directory, compiles it and the runtime with Clang, links with LLD, decodes the
`.text` bytes with Capstone and converts DWARF 5 into Vole's saved debug model.
The ELF is only a guest container; it never runs on the host. No host C
library, host linker, shell or include directory is used.

Compilation runs on the runtime worker thread, never on the UI thread. Clang
has a ten-second deadline and LLD five seconds; each tool runs with argument
arrays, closed stdin, a private `TMPDIR`, bounded diagnostics and output, and
`-fintegrated-cc1` so a timeout cannot orphan a compiler child process.
Environment variables that add include paths (`CPATH`, `C_INCLUDE_PATH`,
`CCC_OVERRIDE_OPTIONS` and similar) are removed. Documents are limited to
256 KiB. A typical build takes about 0.3 seconds after the runtime objects
have been compiled once per target.

## Targets and compiler flags

| Vole target | Clang target | Target flags |
| --- | --- | --- |
| ARM64 / AArch64 | `aarch64-none-elf` | `-mgeneral-regs-only` |
| x64 / x86-64 | `x86_64-none-elf` | `-mgeneral-regs-only` |
| ARM 32-bit (A32) | `armv7a-none-eabi` | `-marm -mfloat-abi=soft -mno-unaligned-access` |
| x86 32-bit | `i386-none-elf` | `-mgeneral-regs-only` |

Every document is compiled with `-std=c17 -fno-trigraphs -ffreestanding -fno-builtin
-nostdlibinc -fno-pic -fno-pie -fno-stack-protector -fno-exceptions
-fno-asynchronous-unwind-tables -fno-unwind-tables -fno-omit-frame-pointer
-fno-common -fno-vectorize -fno-slp-vectorize -gdwarf-5
-fno-color-diagnostics -ferror-limit=20 -Werror=implicit-function-declaration
-Werror=return-type -fintegrated-cc1 -fdebug-compilation-dir=.`, then the
selected optimization level, and `-Wall -Wextra` while warnings are enabled.

Compiler settings are saved with the project:

| Setting | Values | Effect |
| --- | --- | --- |
| Optimization | `-O0` (default), `-O1` | `-O0` keeps every variable in memory. `-O1` produces shorter code; some values become unavailable at some PCs. |
| Warnings | on (default), off | Adds `-Wall -Wextra`. Warnings are listed but never block a build. |

The runtime and startup code are always compiled at `-O0`. `-nostdlibinc`
keeps Clang's own freestanding resource headers and excludes host headers.
`-gdwarf-5` pins one debug format across the supported Clang versions (14+).

## Memory map

| Region | Range | Permissions | Contents |
| --- | --- | --- | --- |
| Null guard | 0x0000–0x0FFF | unmapped | Null-pointer dereferences fault |
| Code | 0x1000–0x7FFF | read, execute | `.text`, exact size, at most 28 KiB |
| Read-only data | 0x8000–0x9FFF | read | `.rodata`: string literals, `const` data, jump tables |
| Data | 0xA000–0xFFFF | read, write | `.data`, then zero-filled `.bss` |
| Stack | 0x10000–0x1FFFF | read, write | Initial SP is 0x20000 |

Writes to code or read-only data fault before the instruction changes any
state, with a message naming the region. Exceeding a region is a link error
with an explanation.

## Startup and runtime

`_start` clears the frame pointer and return address, calls `int main(void)`
with the stack aligned for the target ABI, then calls `vole_exit` with main's
result. `vole_exit` uses the teaching exit call, which halts the machine. The
debugger reports **Program exited with status N** when the machine halted
inside `vole_exit`; the status is the low 32 bits of x0, r0, ebx or rdi.

`#include <vole.h>` (or `"vole.h"`) declares the runtime:

```c
void vole_putc(char c);
void vole_print(const char *text);          /* no newline */
void vole_println(const char *text);        /* adds a newline */
void vole_print_int(long value);
void vole_print_uint(unsigned long value);
void vole_print_hex(unsigned long value);   /* 0x prefix, lowercase */
_Noreturn void vole_exit(int status);
int putchar(int c);
int puts(const char *text);                 /* adds a newline */
int printf(const char *format, ...);
void *memset(void *, int, size_t);
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
int memcmp(const void *, const void *, size_t);
size_t strlen(const char *);
```

`printf` supports `%d %i %u %x %X %o %c %s %p %%`, the `-` and `0` flags, a
decimal width, and the `hh h l ll z` length modifiers. Output is written one
byte at a time through the teaching write call and appears in the Output
panel. The C library names (`printf`, `putchar`, `puts`, `mem*`, `strlen`)
are weak, so a document may define its own `strlen` or `memcpy`. Redefining a
`vole_*` function is reported as already defined by the runtime.

The runtime also provides the support routines Clang calls on 32-bit guests:
ARM `__aeabi_idiv`, `__aeabi_uidiv`, `__aeabi_idivmod`, `__aeabi_uidivmod`,
`__aeabi_ldivmod`, `__aeabi_uldivmod`, `__aeabi_memcpy*`, `__aeabi_memmove*`,
`__aeabi_memset*` and `__aeabi_memclr*`; x86 `__divdi3`, `__udivdi3`,
`__moddi3` and `__umoddi3`. ARMv7-A has no A32 divide instruction, so 32-bit
ARM division always goes through these helpers. Division by zero in the
helpers returns quotient 0, matching ARM64 `sdiv`/`udiv`. On x86 and x64 a
hardware divide by zero raises the #DE divide error and the machine faults.

With an empty `main`, startup and runtime code take under 9 KiB of the code
region and under 1 KiB of read-only data.

## Supported C

Supported: `char`, `short`, `int`, `long` and `long long`, signed and
unsigned, `_Bool`, `enum`, pointers (including function pointers), arrays
(including multi-dimensional), `struct` and `union` values, bit-fields,
`typedef`, `const` and `volatile`, string and character literals, every
integer arithmetic, bitwise, shift, comparison and logical operator,
`if`/`else`, `switch` (including jump tables), `while`, `do`, `for`,
`break`, `continue`, `goto`, `return`, functions with recursion, by-value
struct arguments and results, local, `static` and global variables, and the
runtime above. 64-bit `long long` arithmetic works on 32-bit guests.

Allowed headers: `vole.h`, `stdint.h`, `stddef.h`, `stdbool.h`, `limits.h`,
`stdarg.h`, `stdalign.h`, `stdnoreturn.h`, `iso646.h` and `float.h`, from
Clang's resource directory.

Reported before Clang runs, with the line, column and an explanation:

- floating-point types (`float`, `double`, `long double`, `_Float16`,
  `__fp16` and similar) and floating-point constants such as `1.5` or `1e3`;
- `_Complex`/`_Imaginary`, `__int128`, `_Atomic` and thread-local storage;
- inline assembly (`asm`, `__asm__`);
- `#include` of anything outside the allowed headers, `#include_next`,
  `#import`, `#embed`, `#line` and line markers, `__has_include`, `_Pragma`,
  and pragmas other than `#pragma once` and diagnostic pragmas. The check reads
  the document as the preprocessor does: byte order marks, CR line ends,
  backslash continuations (including trailing spaces) and the `%:` digraph.

These checks exist for clear messages; they are not the security boundary.
Before compiling, Vole runs Clang's preprocessor with `-M` and reads the list
of files Clang actually opened. If it names anything other than the document,
the private `vole.h` and Clang's own resource headers, the build stops and
the preprocessor's output is withheld, so a document cannot read files on the
computer however an include is spelled. Token pasting can still form
`__has_include` or `_Pragma` after the lexical check; that can reveal whether
a host path exists, but not its contents. A pasted `float` is not caught by
the scope check and either compiles without floating-point instructions or is
rejected by Clang or the linker. Every tool's private directory is limited to
64 MiB while it runs, including temporary files the tool renames at the end.

Calling a function that is neither in the document nor the runtime, such as
`malloc`, `scanf` or `fopen`, is an error that explains the freestanding
runtime has no heap, no input and no files, and lists the available
functions. Variadic functions written in the document are not part of the
documented scope. There is no `main(int argc, char **argv)`.

## Debug model

`vole_core::debug::DebugInfo` is extracted from DWARF 5 once, at build time,
and saved inside the project's image, so reopening a project never needs the
compiler. File 0 is the user's document (`main.c`); startup and runtime
files are marked as library code.

| DWARF input | Saved as |
| --- | --- |
| `.debug_line` rows | Address, file, line, column, `is_stmt`, `prologue_end`, `end_sequence` |
| `DW_TAG_subprogram` | Name, address range, declaration, frame base, `prologue_end` address |
| Parameters, locals, lexical blocks, static locals | Name, type, location, declaration line, scope ranges |
| CU-level variables with `DW_OP_addr` | Globals |
| Base, pointer, array, struct, union, enum, typedef, const, volatile, function types | Type table |
| `.debug_frame` CFI | Unwind rows: CFA rule and register rules per address range |

Supported locations are `DW_OP_fbreg`, `DW_OP_addr`, `DW_OP_bregN`,
`DW_OP_regN` and location lists of those. On ARM64 at `-O0` Clang describes
locals relative to `sp`. Constants produced by optimization, entry values and
pieced values are saved as unavailable with the compiler's reason (for
example "optimized into the constant 3"). Variables of inlined calls are not
listed at `-O1`. DWARF register numbers map to the interpreters' names:
AArch64 0–30 → x0–x30, 31 → sp; x86-64 0 rax, 1 rdx, 2 rcx, 3 rbx, 4 rsi,
5 rdi, 6 rbp, 7 rsp, 8–15 r8–r15, 16 return address; i386 0 eax, 1 ecx,
2 edx, 3 ebx, 4 esp, 5 ebp, 6 esi, 7 edi, 8 return address; ARM 0–14
r0–r14, 15 pc.

`Instruction::source_line` is set for every instruction covered by a user
line row, which is how the machine-code pane groups instructions under their
C line. A loaded or restored C image is validated: sorted tables, in-range
indices, a 32- or 64-bit target and present debug information.

## Debugging semantics

**Source breakpoints.** A breakpoint on line L resolves to the lowest-address
`is_stmt` row for L in the document. If that address is a function's entry,
it moves to the end of the prologue so parameters are readable. A line without
code snaps forward to the next line with code; the status bar says which line
and address are used, or that nothing after the line has code. Breakpoints are
stored as line numbers, move with lines inserted or deleted above them (a
breakpoint inside deleted text is removed), and are re-resolved after every
build, load and restore. While the document differs from the last successful
build, the machine-code pane keeps showing that build's own C text and the
editor shows no execution marks.
Instruction breakpoints set in the machine-code pane are kept separately.

**Step into** (F11) runs until the PC reaches the start of a user `is_stmt`
row and the line differs, the frame differs, or control jumped backward to a
new loop iteration. It enters user functions, stopping after their prologue,
and steps through runtime code without stopping there. From startup code it
stops at the first line of `main`.

**Step over** (F10) is Step into that ignores rows while the canonical frame
address (CFA) is below the starting frame's, so calls run to completion.
Breakpoints inside called functions still stop.

**Step out** (Shift+F11) runs until the current frame returns and stops on the
first instruction back in the caller, which is usually the middle of the
calling line. From runtime code it returns to the innermost user frame.

**Instruction step** (Ctrl/Cmd+F10 or Alt+F10) and **Back** (Shift+F10)
execute and reverse one machine instruction. Every instruction executed by a
source step stays in the bounded reverse history (4096 instructions, about
two MiB). Continue (F5) runs to the next breakpoint, watchpoint, fault or
exit; after stopping on a breakpoint, Continue moves past it.

Source steps and Continue are bounded by the one-million-instruction run
budget, run in batches on the worker thread and can be paused.

**Call stack.** Frames are unwound with the saved CFI rows: CFA, return
address and callee-saved registers are recovered for each caller. Unwinding
stops at `_start`, code without debug information, a zero or unknown return
address, a CFA that does not increase, or 64 frames. Outer frames show the line
of the call.

**Variables.** Each frame lists parameters, then locals whose lexical scope
contains the frame's PC; globals from the document follow. Values are read
from guest memory and registers and formatted by type: signed and unsigned
integers, characters as `65 'A'`, booleans, enumerators by name, pointers as
an address with a string preview for character pointers, arrays (first 64
elements) and struct or union members as expandable children. Each value shows
where it lives, such as `[x29−4]`, `[sp+24]`, `x19` or `global 0x0000A000`;
selecting a memory location outlines the variable's bytes in the memory grid.
Values that changed since the previous stop use the write colour.

A value is shown as unavailable, with the reason, when:

- the function prologue has not finished, so frame slots are not set up;
- the local is declared on a later line that has not been reached yet;
- an optimized location list has no entry for this PC ("optimized out");
- the compiler replaced it with a constant or used an unsupported location;
- an outer frame kept it in a register that the call did not preserve;
- its address is not mapped, or its type is unknown.

## Workbench

Choose **C** beside the target menu (C is available for ARM32, ARM64, x86 and
x64), edit, then **Compile** (Ctrl/Cmd+Enter). The C source, machine code
grouped by C line with addresses and bytes, the decoded instruction, call
stack, variables, registers, memory and output stay synchronized: the current
line is filled in the editor and its instruction group has a rail; selecting
a C line highlights its instructions; selecting an instruction moves the
cursor and memory view. Diagnostics list severity, line and column, the
message and a hint, and selecting one moves the cursor there. F9 toggles a
breakpoint on the cursor line. Fullscreen is Ctrl+Shift+F (Ctrl+Cmd+F on
macOS) in C mode; F11 stays fullscreen in assembly mode.

## Projects

A C project is saved as version 2 of the `.voleproject` format, with
`language`, `compiler` settings, `source_breakpoints` (lines), the compiled
image including its debug model, and the current machine state. Assembly
projects are still written as version 1 without the new fields, so earlier
releases can open them, and every existing version 1 project still opens.
A version 1 file that claims C content is rejected. Undo history is not saved;
it begins again after a restore.

## Command line

```sh
vole-cli --source examples/c/tour.c                 # ARM64, prints output and exit status
vole-cli --arch x86 --source examples/c/tour.c --opt O1
vole-cli --arch arm32 --source examples/c/functions.c --break 12 --json
vole-cli --arch x64 --source examples/c/pointers.c --step-over 5
```

`.c` files and C projects select C automatically. Each `--break` stop prints
the call stack and the innermost frame's variables. `--json` prints a report
with `state`, `exit_status`, `output`, `steps` and the stops.

## Examples

`examples/c` contains `tour.c` (the default C example), `hello.c`,
`arithmetic.c`, `loops.c`, `functions.c`, `arrays.c` and `pointers.c`. Every
example builds and runs on all four targets at `-O0` and `-O1`; the tests
compare their output with expectations checked against a native host build.

## Toolchain

C needs Clang 14 or newer plus LLD; LLVM MC is only needed for assembly. Clang
is found through `VOLE_CLANG`, `VOLE_TOOLCHAIN_DIR`, a `toolchain` directory
beside the executable (or the macOS bundle's `Contents/Resources/toolchain`),
then `clang-14`, `clang` and `clang-15`…`clang-23` on `PATH`.
`VOLE_CLANG_RESOURCE_DIR` selects Clang's resource directory explicitly; a
bundled `toolchain/lib/clang/<major>/include` is passed with `-resource-dir`
automatically. Apple's Xcode clang is not supported for guest images; use
Homebrew LLVM on macOS. See the README for setup on each host.

## Primary references

- Clang command line reference: <https://clang.llvm.org/docs/ClangCommandLineReference.html>
- Clang cross-compilation: <https://clang.llvm.org/docs/CrossCompilation.html>
- Clang user's manual (freestanding mode, resource headers): <https://clang.llvm.org/docs/UsersManual.html>
- LLD ELF linker scripts: <https://lld.llvm.org/ELF/linker_script.html>
- DWARF Version 5 standard: <https://dwarfstd.org/dwarf5std.html>
- Arm run-time ABI (`__aeabi_*` helpers): <https://github.com/ARM-software/abi-aa/blob/main/rtabi32/rtabi32.rst>
- AAPCS64: <https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst>
- System V x86-64 psABI: <https://gitlab.com/x86-psABIs/x86-64-ABI>
- gimli DWARF reader: <https://docs.rs/gimli/0.32.3/gimli/>
