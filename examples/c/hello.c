/* Text output with the Vole runtime. */
#include <vole.h>

int main(void) {
    /* The smallest building block: one character at a time. */
    putchar('H');
    putchar('i');
    putchar('\n');

    /* Whole strings, with and without a newline. */
    vole_print("Hello, ");
    vole_println("Vole!");
    puts("puts adds a newline too.");

    /* Numbers without printf. */
    vole_print("int: ");
    vole_print_int(-42);
    vole_print(", hex: ");
    vole_print_hex(48879);
    vole_putc('\n');

    /* printf formats several values at once. */
    const char *name = "Ada";
    char initial = name[0];
    printf("Name: %s, initial: %c, length: %u\n", name, initial, (unsigned)strlen(name));
    printf("Decimal %d, hex %x, upper hex %X, octal %o\n", 255, 255, 255, 255);
    printf("Width: [%5d] [%-5d] [%05d]\n", 42, 42, 42);
    printf("Percent sign: 100%%\n");
    return 0;
}
