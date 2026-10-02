.syntax unified
.arm
.text
.global _start
_start:
    mov r0, #5
    mov r1, #0
again:
    bl add_twenty_five
    subs r0, r0, #1
    bne again
    mov r4, #8192
    str r1, [r4]
    bkpt #0
add_twenty_five:
    push {r4, lr}
    add r1, r1, #25
    pop {r4, pc}
