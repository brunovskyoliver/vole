.text
.global _start
_start:
    mov x0, #1
    adr x1, message
    mov x2, #6
    mov x8, #64
    svc #0
    mov x0, #0
    mov x8, #93
    svc #0
.data
message:
    .ascii "Vole!\n"
