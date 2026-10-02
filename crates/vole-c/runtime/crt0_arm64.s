// Vole startup code for ARM64 (AArch64).
// The simulator starts here with SP = 0x20000, which is already 16-byte aligned.
    .text
    .cfi_sections .debug_frame
    .global _start
    .type _start, %function
_start:
    .cfi_startproc
    .cfi_undefined 30           // No caller: unwinding stops here.
    mov x29, #0                 // Clear the frame pointer...
    mov x30, #0                 // ...and the link register.
    bl main                     // w0 = main()
    bl vole_exit                // vole_exit(w0) never returns.
    .cfi_endproc
    .size _start, . - _start
