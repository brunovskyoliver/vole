.intel_syntax noprefix
.text
.global _start
_start:
    mov eax, 58
    mov ebx, 67
    add eax, ebx
    mov dword ptr [8192], eax
    int3
