.intel_syntax noprefix
.text
.global _start
_start:
    mov eax, 1
    mov edi, 1
    lea rsi, [rip + message]
    mov edx, 6
    syscall
    mov eax, 60
    mov edi, 0
    syscall
.data
message:
    .ascii "Vole!\n"
