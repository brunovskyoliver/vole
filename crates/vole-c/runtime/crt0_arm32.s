@ Vole startup code for ARM32 (A32 instructions, AAPCS).
@ The simulator starts here with SP (r13) = 0x20000, which is 8-byte aligned.
    .syntax unified
    .arm
    .text
    .cfi_sections .debug_frame
    .global _start
    .type _start, %function
_start:
    .cfi_startproc
    .cfi_undefined 14           @ No caller: unwinding stops here.
    mov r11, #0                 @ Clear the frame pointer...
    mov lr, #0                  @ ...and the link register.
    bl main                     @ r0 = main()
    bl vole_exit                @ vole_exit(r0) never returns.
    .cfi_endproc
    .size _start, . - _start

@ 64-bit division helpers from the ARM run-time ABI. They return the quotient
@ in r0:r1 and the remainder in r2:r3, so they call a C helper that stores the
@ remainder in a stack slot and then load it into r2:r3.
    .global __aeabi_ldivmod
    .type __aeabi_ldivmod, %function
__aeabi_ldivmod:
    .cfi_startproc
    push {r4, lr}
    .cfi_def_cfa_offset 8
    .cfi_offset lr, -4
    .cfi_offset r4, -8
    sub sp, sp, #16             @ [sp] = remainder pointer, [sp + 8] = remainder
    .cfi_def_cfa_offset 24
    add r4, sp, #8
    str r4, [sp]
    bl __vole_ldivmod
    ldr r2, [sp, #8]
    ldr r3, [sp, #12]
    add sp, sp, #16
    .cfi_def_cfa_offset 8
    pop {r4, pc}
    .cfi_endproc
    .size __aeabi_ldivmod, . - __aeabi_ldivmod

    .global __aeabi_uldivmod
    .type __aeabi_uldivmod, %function
__aeabi_uldivmod:
    .cfi_startproc
    push {r4, lr}
    .cfi_def_cfa_offset 8
    .cfi_offset lr, -4
    .cfi_offset r4, -8
    sub sp, sp, #16
    .cfi_def_cfa_offset 24
    add r4, sp, #8
    str r4, [sp]
    bl __vole_uldivmod
    ldr r2, [sp, #8]
    ldr r3, [sp, #12]
    add sp, sp, #16
    .cfi_def_cfa_offset 8
    pop {r4, pc}
    .cfi_endproc
    .size __aeabi_uldivmod, . - __aeabi_uldivmod
