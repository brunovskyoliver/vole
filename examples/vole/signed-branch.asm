; The F branch uses signed two's-complement comparison.
load R0, 0
load R1, -1
jmpLE R1<=R0, negative
load RF, 78
halt
negative: load RF, 89
halt
