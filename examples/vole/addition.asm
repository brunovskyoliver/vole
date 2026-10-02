; Add 58 and 67, then inspect cell BB.
load R1, 3Ah
load R2, 43h
addi R3, R1, R2
store R3, [0BBh]
halt
