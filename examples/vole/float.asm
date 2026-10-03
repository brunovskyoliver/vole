; 0x48 means 0.5 in the teaching floating format.
; Adding two halves produces 0x58, which means 1.0.
load R1, 0x48
load R2, 0x48
addf R3, R1, R2
store R3, [0x80]
halt
