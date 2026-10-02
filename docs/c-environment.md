# Freestanding C environment (design contract)

Status: implementation contract for the C compilation and source-debugging
release. Sections marked *Contract* are shared between the compiler,
interpreter, debugger and workbench work and must not change silently.

## Pipeline

```text
C document ──clang -c──▶ main.o ─┐
vole_runtime.c ──clang -c──▶ ... ├─ld.lld (fixed script)──▶ guest ELF ──▶ Program
crt0.s ──clang -c──▶ crt0.o ─────┘                                         + DebugInfo
```

`vole_c::compile(architecture, source, settings)` returns a `Program` with
`language = C` and `debug = Some(DebugInfo)`. The ELF is a guest container on
every host. Clang never runs the guest program and no host linker or libc is
involved. Compilation runs on the runtime worker thread, never the UI thread.

## Targets *(Contract)*

| Vole target | Clang target | Target flags |
| --- | --- | --- |
| ARM64 | `aarch64-none-elf` | `-mgeneral-regs-only` |
| x64 | `x86_64-none-elf` | `-mgeneral-regs-only` |
| ARM32 | `armv7a-none-eabi` | `-marm -mfloat-abi=soft -mno-unaligned-access` |
| x86 | `i386-none-elf` | `-mgeneral-regs-only` |

Common flags: `-std=c17 -ffreestanding -fno-builtin -nostdlibinc -fno-pic
-fno-pie -fno-stack-protector -fno-exceptions -fno-asynchronous-unwind-tables
-fno-unwind-tables -fno-omit-frame-pointer -fno-common -fno-vectorize
-fno-slp-vectorize -gdwarf-5 -fno-color-diagnostics -ferror-limit=20
-Werror=implicit-function-declaration -Werror=return-type`, plus `-O0` (default)
or `-O1`, and `-Wall -Wextra` when warnings are enabled. The runtime and
startup code are always compiled at `-O0`.

`-nostdlibinc` keeps Clang's own freestanding resource headers and excludes
host C library headers. `-gdwarf-5` pins the debug format across Clang 14–18.

## Memory map *(Contract)*

| Region | Range | Permissions | Contents |
| --- | --- | --- | --- |
| Null guard | 0x0000–0x0FFF | unmapped | Null-pointer dereferences fault |
| Code | 0x1000–0x7FFF | read, execute | `.text` (exact size, at most 28 KiB) |
| Read-only data | 0x8000–0x9FFF | read | `.rodata` (8 KiB region) |
| Data | 0xA000–0xFFFF | read, write | `.data` then `.bss` (24 KiB region) |
| Stack | 0x10000–0x1FFFF | read, write | Initial SP 0x20000 |

Writes to code or string literals fault atomically with a clear message.

## Runtime *(Contract)*

`#include <vole.h>` declares the teaching runtime. Clang's freestanding
headers (`stdint.h`, `stddef.h`, `stdbool.h`, `limits.h`, `stdarg.h`,
`stdalign.h`, `stdnoreturn.h`, `iso646.h`, `float.h`) are also allowed. Any
other `#include` is rejected before Clang runs, so documents cannot read host
files.

```c
void vole_putc(char c);
void vole_print(const char *text);            /* no newline */
void vole_println(const char *text);          /* adds a newline */
void vole_print_int(long value);
void vole_print_uint(unsigned long value);
void vole_print_hex(unsigned long value);     /* 0x prefix, lowercase */
_Noreturn void vole_exit(int status);
int putchar(int c);
int puts(const char *text);                   /* adds a newline */
int printf(const char *format, ...);          /* subset below */
void *memset(void *, int, size_t);
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
int memcmp(const void *, const void *, size_t);
size_t strlen(const char *);
```

`printf` supports `%d %i %u %x %X %o %c %s %p %%`, the `-` and `0` flags, a
decimal width, and the `hh h l ll z` length modifiers. Output uses the existing
teaching write call one byte at a time.

Startup `_start` clears the frame pointer and link register, calls
`int main(void)`, then calls `vole_exit(main's result)`. `vole_exit` issues
the teaching exit call, which halts the machine. The exit status remains in
the exit-call argument register (x0, r0, ebx, rdi).

Compiler support routines are part of the runtime: ARM32
`__aeabi_idiv/uidiv/idivmod/uidivmod/ldivmod/uldivmod` and x86
`__divdi3/__udivdi3/__moddi3/__umoddi3`, so integer division works on every
target, including 64-bit `long long` on 32-bit guests.

## Supported C scope

Supported: `char`, `short`, `int`, `long`, `long long` (signed and unsigned),
`_Bool`, `enum`, pointers, arrays (including multi-dimensional), `struct`
and `union` values in memory, `typedef`, `const`/`volatile`, string literals,
integer arithmetic/bitwise/shift/comparison operators, `if/else`, `switch`,
`while`, `do`, `for`, `break`, `continue`, `goto`, `return`, functions
(including recursion and pointer arguments), local/static/global variables,
and the runtime above.

Not supported, reported before compilation with an explanation: `float`,
`double`, `long double`, `_Complex`, `__int128`, `_Atomic`, `_Thread_local`,
inline assembly, and `#include` outside the allow list. Calls to functions not
in the document or runtime (e.g. `malloc`, `scanf`, `fopen`) fail at link
time with an explanation that the freestanding runtime has no heap, input or
files. Variadic user functions are not part of the documented scope.

## Debug model *(Contract)*

`vole_core::debug::DebugInfo` is extracted from DWARF 5 once, at build time,
and saved in projects. Files index 0 is the user's document (`main.c`,
`user = true`); startup and runtime files have `user = false`.

- Line rows come from `.debug_line`. `Instruction::source_line` is set only
  for rows in the user file.
- Functions/variables come from `.debug_info`. Frame base and variable
  locations support `DW_OP_fbreg`, `DW_OP_addr`, `DW_OP_bregN`, `DW_OP_regN`
  and location lists of those. Anything else becomes `Location::Unavailable`
  with a reason.
- Unwind rows come from `.debug_frame` CFI and use machine register names.
  DWARF register numbers map as: AArch64 0–30 → x0–x30, 31 → sp; x86-64
  0 rax, 1 rdx, 2 rcx, 3 rbx, 4 rsi, 5 rdi, 6 rbp, 7 rsp, 8–15 r8–r15,
  16 → return address; i386 0 eax, 1 ecx, 2 edx, 3 ebx, 4 esp, 5 ebp, 6 esi,
  7 edi, 8 → return address; ARM 0–12 r0–r12, 13 r13 (sp), 14 r14 (lr), 15 pc.

## Source debugging semantics *(Contract)*

- **Source breakpoint** on line L resolves to the lowest-address `is_stmt` row
  for L in the user file. If that address is a function's `low_pc`, it moves to
  that function's `prologue_end`. Lines without code snap forward to the next
  line with code. Addresses are re-resolved after every build.
- **Step Into** runs until PC is at the start of a user `is_stmt` row and
  either the line differs, the frame (CFA) differs, or control jumped backward
  (a new loop iteration). Runtime code without user lines is stepped through.
- **Step Over** is Step Into that ignores rows while the current CFA is below
  the starting CFA (inside a callee). Breakpoints inside callees still stop.
- **Step Out** runs until the CFA is above the starting CFA (the frame
  returned), stopping on the first instruction back in the caller.
- All source steps are bounded by the run instruction budget, can be paused,
  stop on breakpoints, watchpoints, faults and halts, and leave every executed
  instruction in reverse history.
- **Variables**: parameters and locals of each frame whose scope contains the
  frame PC, plus globals. Values come from guest memory/registers. Unavailable
  values say why: prologue not finished, declared later and not reached,
  optimized out at this PC, unsupported location, or unmapped memory.

## Primary references

- Clang command line reference: <https://clang.llvm.org/docs/ClangCommandLineReference.html>
- Clang cross-compilation: <https://clang.llvm.org/docs/CrossCompilation.html>
- Clang freestanding/`-ffreestanding` and resource headers: <https://clang.llvm.org/docs/UsersManual.html>
- LLD ELF linker scripts: <https://lld.llvm.org/ELF/linker_script.html>
- DWARF Version 5 standard: <https://dwarfstd.org/dwarf5std.html>
- Arm run-time ABI (`__aeabi_*` helpers): <https://github.com/ARM-software/abi-aa/blob/main/rtabi32/rtabi32.rst>
- Procedure call standards: AAPCS64 <https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst>, System V x86-64 psABI <https://gitlab.com/x86-psABIs/x86-64-ABI>
- gimli DWARF reader: <https://docs.rs/gimli/0.32.3/gimli/>
