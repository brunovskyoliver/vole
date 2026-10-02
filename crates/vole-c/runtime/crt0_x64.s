# Vole startup code for x64 (x86-64), AT&T syntax.
# The simulator starts here with RSP = 0x20000.
    .text
    .cfi_sections .debug_frame
    .globl _start
    .type _start, @function
_start:
    .cfi_startproc
    .cfi_undefined 16           # No caller: unwinding stops here.
    xorl %ebp, %ebp             # Clear the frame pointer.
    andq $-16, %rsp             # System V: RSP + 8 is 16-byte aligned at main.
    call main                   # eax = main()
    movl %eax, %edi
    call vole_exit              # vole_exit(eax) never returns.
    .cfi_endproc
    .size _start, . - _start
