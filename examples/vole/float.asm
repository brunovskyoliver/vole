; 48h means 0.5 in the teaching floating format.
; Adding two halves produces 58h, which means 1.0.
load R1, 48h
load R2, 48h
addf R3, R1, R2
store R3, [80h]
halt
