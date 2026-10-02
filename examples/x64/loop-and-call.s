.intel_syntax noprefix
.text
.global _start
_start:
    mov rcx, 5
    mov rax, 0
again:
    call add_twenty_five
    dec rcx
    jnz again
    mov qword ptr [8192], rax
    int3
add_twenty_five:
    push rbx
    add rax, 25
    pop rbx
    ret
