.syntax unified
.arm
.text
.global _start
_start:
    mov r0, #1
    ldr r1, =message
    mov r2, #6
    mov r7, #4
    svc #0
    mov r0, #0
    mov r7, #1
    svc #0
.data
message:
    .ascii "Vole!\n"
