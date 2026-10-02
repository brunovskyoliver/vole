.text
.global _start
_start:
    mov x1, #58
    mov x2, #67
    add x3, x1, x2
    mov x4, #8192
    str x3, [x4]
    brk #0
