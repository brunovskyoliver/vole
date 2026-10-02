.intel_syntax noprefix
.text
.global _start
_start:
    mov ecx, 5
    mov eax, 0
again:
    call add_twenty_five
    dec ecx
    jnz again
    mov dword ptr [8192], eax
    int3
add_twenty_five:
    push ebx
    add eax, 25
    pop ebx
    ret
