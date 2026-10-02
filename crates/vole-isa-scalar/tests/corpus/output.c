/* printf, puts and the variadic runtime. */
#include <stdarg.h>
#include <vole.h>

volatile int number = -42;
volatile unsigned long long wide = 0x123456789abcdefULL;

int main(void) {
    printf("%d %i %u %x %X %o %c %s %%\n", number, 17, 3000000000u, 0xbeef, 0xbeef, 8, 'V', "ok");
    printf("[%5d][%-5d][%05d][%3s][%-3s]\n", number, number, number, "a", "b");
    printf("%ld %lu %lld %llu %llx\n", (long)number, 99ul, (long long)wide * -1, wide, wide);
    printf("%hd %hhd %hu %hhu %zu\n", 70000, 300, 70000, 300, (unsigned long)12);
    puts("done");
    vole_print_int(-7);
    vole_putc(' ');
    vole_print_hex(255);
    vole_println("");
    return 3;
}
