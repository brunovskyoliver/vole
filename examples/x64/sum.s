.intel_syntax noprefix
.text
.global _start
_start:
    mov rax, 58
    mov rbx, 67
    add rax, rbx
    mov qword ptr [8192], rax
    int3
