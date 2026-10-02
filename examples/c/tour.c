/* A tour of C on the Vole machine.
 * Set a breakpoint on a line, then use Step Into, Step Over and Step Out
 * and watch the variables, registers and memory change. */
#include <vole.h>

int scores[5] = {72, 85, 90, 64, 88}; /* a global array in the data region */
const char *title = "Vole C tour";    /* the text lives in read-only data */

/* Recursion: every call gets its own stack frame. */
int factorial(int n) {
    if (n <= 1) {
        return 1;
    }
    return n * factorial(n - 1);
}

/* An array argument arrives as a pointer to its first element. */
int sum(const int *values, int count) {
    int total = 0;
    for (int i = 0; i < count; i++) {
        total += values[i];
    }
    return total;
}

/* Pointers let a function change the caller's variables. */
void swap(int *a, int *b) {
    int temporary = *a;
    *a = *b;
    *b = temporary;
}

int main(void) {
    int x = 7;
    int y = 3;

    vole_println(title);
    printf("x + y = %d, x - y = %d\n", x + y, x - y);
    printf("x * y = %d, x / y = %d, x %% y = %d\n", x * y, x / y, x % y);

    swap(&x, &y);
    printf("after swap: x = %d, y = %d\n", x, y);

    int total = sum(scores, 5);
    printf("total score = %d, average = %d\n", total, total / 5);

    for (int n = 1; n <= 5; n++) {
        printf("%d! = %d\n", n, factorial(n));
    }

    unsigned mask = 0xF0u | (1u << 2);
    printf("mask = 0x%x\n", mask);
    return 0;
}
