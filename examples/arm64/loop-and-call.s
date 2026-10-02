.text
.global _start
_start:
    mov x0, #5
    mov x1, #0
again:
    bl add_twenty_five
    subs x0, x0, #1
    b.ne again
    mov x4, #8192
    str x1, [x4]
    brk #0
add_twenty_five:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    add x1, x1, #25
    ldp x29, x30, [sp], #16
    ret
