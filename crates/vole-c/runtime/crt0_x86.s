# Vole startup code for x86 (IA-32), AT&T syntax.
# The simulator starts here with ESP = 0x20000.
    .text
    .cfi_sections .debug_frame
    .globl _start
    .type _start, @function
_start:
    .cfi_startproc
    .cfi_undefined 8            # No caller: unwinding stops here.
    xorl %ebp, %ebp             # Clear the frame pointer.
    andl $-16, %esp             # System V: ESP + 4 is 16-byte aligned at main.
    call main                   # eax = main()
    subl $12, %esp              # Keep the stack aligned for the next call.
    pushl %eax
    call vole_exit              # vole_exit(eax) never returns.
    .cfi_endproc
    .size _start, . - _start
