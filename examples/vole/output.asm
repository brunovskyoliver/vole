; Print a zero-terminated string using indirect loads and RF output.
load R0, 0
load R1, message
load R2, 1
next: load R3, [R1]
jmpEQ R3=R0, done
move RF, R3
addi R1, R1, R2
jmp next
done: halt
message: db "Vole ready.\n", 0
