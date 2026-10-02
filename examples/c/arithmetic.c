/* Integer arithmetic on the Vole machine. There is no floating point:
 * every value here is a whole number held in integer registers. */
#include <vole.h>

int main(void) {
    int a = 17;
    int b = 5;

    printf("a = %d, b = %d\n", a, b);
    printf("a + b = %d\n", a + b);
    printf("a - b = %d\n", a - b);
    printf("a * b = %d\n", a * b);
    printf("a / b = %d (division drops the fraction)\n", a / b);
    printf("a %% b = %d (the remainder)\n", a % b);
    printf("-a / b = %d, -a %% b = %d (rounds toward zero)\n", -a / b, -a % b);

    /* Bitwise operators work on the binary digits. */
    unsigned bits = 0x5Au;
    printf("bits = 0x%x, bits & 0x0F = 0x%x, bits | 0x0F = 0x%x\n", bits, bits & 0x0Fu, bits | 0x0Fu);
    printf("bits ^ 0xFF = 0x%x, bits << 2 = 0x%x, bits >> 3 = 0x%x\n", bits ^ 0xFFu, bits << 2, bits >> 3);

    /* Unsigned arithmetic wraps around. */
    unsigned char small = 250;
    small = small + 10;
    printf("250 + 10 in an unsigned char = %u\n", (unsigned)small);

    /* long long is 64 bits on every target, even the 32-bit ones. */
    long long big = 1234567890123LL;
    printf("big = %lld, big / 1000 = %lld, big %% 1000 = %lld\n", big, big / 1000, big % 1000);

    /* Average of three values, rounded down. */
    int total = 10 + 20 + 25;
    printf("average = %d\n", total / 3);
    return 0;
}
