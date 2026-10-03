; Add 58 and 67, then inspect cell BB.
load R1, 0x3A
load R2, 0x43
addi R3, R1, R2
store R3, [0xBB]
halt
