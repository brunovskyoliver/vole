.syntax unified
.arm
.text
.global _start
_start:
    mov r1, #58
    mov r2, #67
    add r3, r1, r2
    mov r4, #8192
    str r3, [r4]
    bkpt #0
