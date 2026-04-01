	.text
	.globl main
main:
	li t0, 0
	xor t0, t0, 6
	seqz t0, t0
	sub s7, 0, t0
	sub t3, 0, s7
	add s11, 0, t3
	mv a0, s11