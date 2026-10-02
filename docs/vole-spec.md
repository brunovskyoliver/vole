# VOLE machine profile

The implemented profile is `simpsim-extended-v1`. It follows the supplied
instruction table and the SimpSim author's instruction help, including D, E
and F. Signed F, boundary fetch and strict reserved-bit handling are explicit
Vole conventions. They have not been compared with a running SimpSim binary.
[Author's instruction help](https://www.anne-gert.nl/projects/simpsim/images/instructions.gif).

## State and instructions

The machine has sixteen byte registers, R0 through RF, 256 writable byte
cells, and a byte PC. Every instruction is two bytes; the high nibble is the
opcode. A fetch at FF reads its second byte from 00. Sequential PC advances
by two modulo 256. Branch targets are byte addresses and may be odd.
Loading a program zeroes unassigned cells and registers, then applies initial
register values. Reset restores that original loaded state, including memory.

| Bytes | Readable instruction | Effect |
| --- | --- | --- |
| 1RXY | `load R, [XY]` | R gets the memory byte |
| 2RXY | `load R, XY` | R gets the immediate byte |
| 3RXY | `store R, [XY]` | Memory gets R |
| 40RS | `move S, R` | Destination S gets source R |
| 5RST | `addi R, S, T` | R gets S + T modulo 256 |
| 6RST | `addf R, S, T` | Teaching-format floating addition |
| 7RST | `or R, S, T` | Bitwise OR |
| 8RST | `and R, S, T` | Bitwise AND |
| 9RST | `xor R, S, T` | Bitwise XOR |
| AR0X | `ror R, X` | Rotate the byte right; count reduced modulo 8 |
| BRXY | `jmpEQ R=R0, XY` | Branch if R equals R0 |
| B0XY | `jmp XY` | Unconditional branch alias |
| C000 | `halt` | Advance PC and halt |
| D0RS | `load R, [S]` | Load through S's byte address |
| E0RS | `store R, [S]` | Store through S's byte address |
| FRXY | `jmpLE R<=R0, XY` | Branch if signed8(R) <= signed8(R0) |

Opcode zero, nonzero reserved nibbles and halt encodings other than C000
fault before changing state. Integer values wrap only for integer addition
and rotation. Assembler values outside their documented ranges are errors.

## Teaching floating point

A byte is `s eee mmmm`. Its value is
`(-1)^s * (mmmm / 16) * 2^(eee - 4)`. There is no hidden leading bit,
infinity, NaN or IEEE floating format. Princeton's Brookshear worked
solutions confirm the fractional mantissa, excess-4 exponent, the EF encoding
of -3.75, and truncation of a fifth mantissa digit.
[Princeton course solutions](https://www.cs.princeton.edu/courses/archive/fall99/cs111/probs/2_soln.html).

Addition decodes both operands exactly into integer units of 1/256. It then
adds them before selecting an output encoding. Results normalize to mantissa
8..15 where possible, using exponent zero for smaller values. Discarded low
bits truncate toward zero. Any input with zero mantissa represents zero;
output zero is always 00. All byte encodings are accepted as inputs.
The smallest positive value is 1/256, and the largest is 7.5. A result whose
exact magnitude exceeds 7.5 faults atomically. These overflow, zero and
normalization choices are this application's conventions.

| Inputs | Output | Meaning |
| --- | --- | --- |
| 48 + 48 | 58 | 0.5 + 0.5 = 1 |
| 68 + 38 | 69 | 2 + 0.25 = 2.25 |
| 18 + 1F | 2B | 0.1796875 truncates to 0.171875 |
| C8 + 48 | 00 | -0.5 + 0.5 = 0 |
| 01 + 01 | 02 | 1/256 + 1/256 = 2/256 |
| 7F + 7F | fault | 7.5 + 7.5 cannot be represented |

## Assembler

Mnemonics, registers and labels are case insensitive. Labels use `name:`;
names begin with a letter or underscore and contain letters, digits or
underscores. `;` starts a comment outside a quoted string. Operands use
commas. `mov` is an alias for `move`. Conditional branches accept either
`R1=R0` / `R1<=R0` or the shorter `R1` operand.

Numbers may be decimal, suffix-h hexadecimal or `0x` hexadecimal. Byte
literals accept -128..255 and encode negatives in two's complement. Memory
addresses and origins accept 0..255; rotation counts accept 0..15. Symbols
may be used as operands and in `label + number` or `label - number`
expressions. Instruction and db operands resolve forward labels in the
second pass. `org` requires a number or a previously defined label.

`org` changes the output location. `db` emits bytes or single/double quoted
ASCII strings. Strings support `\n`, `\r`, `\t`, `\0`, escaped quotes and
escaped backslashes. For arbitrary encodings, provide explicit db bytes.
Colons and semicolons inside strings remain data. Emission beyond FF,
overlapping origins, duplicate labels and unresolved symbols report their
source line. The entry address is the first assembled instruction, or the
first emitted region for data-only machine images.

## Output, debugger and reversal

Every executed write to RF appends one raw byte to simulated output, even if
the register value is unchanged. This follows the author's RF output
example. No bytes trigger screen clearing or destructive terminal controls;
the presentation layer decides how to show control bytes. Debugger edits of
RF do not emit output.
[Author's RF output example](https://www.anne-gert.nl/projects/simpsim/examples/outtest.asm).

A successful step records before/after PC, decoded instruction, changed
registers, memory writes, actual data reads and appended output. Instruction
fetches are excluded from data-read watchpoints. Failed steps leave all
observable state and retained history unchanged.

Reverse Step restores registers, memory, PC, halted state, step count and
output length. Up to 4096 successful steps are retained. Reversing past the
retained boundary reports an error. Valid debugger register/memory edits
clear that history; invalid edits are atomic and preserve history. PC edits
resume a halted machine. Reset and program loads clear history.

The decoder powers instruction explanations and read/write highlighting.
Self-modified bytes are decoded live; original source-line attribution is
kept only while the bytes still match their original instruction.

## Acceptance examples

`examples/vole/addition.asm` assembles to the independent acceptance bytes
`21 3A 22 43 53 12 33 BB C0 00`. Its final R3 and memory BB equal 7D.
The other examples exercise indirect data reads, loops, signed comparisons
and teaching floating point. They are original project examples.

## Raw machine images and projects

Raw VOLE import loads 1..256 bytes starting at 00, with entry 00 and zeroed
remaining memory/registers. Raw bytes do not encode a target, origin,
breakpoints, segment permissions or saved execution state. The selected target supplies that
interpretation. Export requires both the first mapped region and entry to be
00, preserves address gaps as zero bytes, and rejects other origins rather
than silently moving the program. Save a versioned project for programs
using `org` with another entry or for a resumable execution snapshot.

ARM32, ARM64, x86 and x64 raw imports start at 1000h. Their first 4096 bytes
are code; subsequent bytes populate main memory and stack at the fixed
teaching addresses. Their raw image limit is 1F000h bytes. A normal source
export includes the mapped data/stack image, and import initializes the
target's stack pointer. Exports beyond this importable span are rejected;
save a project to retain a wider sparse layout. See `isa-support.md` for the
teaching memory map.
