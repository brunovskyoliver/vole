.intel_syntax noprefix
.text
.global _start
_start:
    mov eax, 4
    mov ebx, 1
    mov ecx, offset message
    mov edx, 6
    int 0x80
    mov eax, 1
    mov ebx, 0
    int 0x80
.data
message:
    .ascii "Vole!\n"
