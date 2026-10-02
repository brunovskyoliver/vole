/* Loops and decisions. */
#include <vole.h>

int main(void) {
    /* for: count up. */
    for (int i = 1; i <= 5; i++) {
        printf("%d ", i);
    }
    putchar('\n');

    /* while: halve a number until it reaches 1. */
    int n = 40;
    while (n > 1) {
        printf("%d -> ", n);
        n = n / 2;
    }
    printf("%d\n", n);

    /* do-while: the body runs at least once. */
    int digits = 0;
    int value = 9075;
    do {
        digits++;
        value /= 10;
    } while (value != 0);
    printf("9075 has %d digits\n", digits);

    /* break and continue: odd numbers below 12. */
    for (int i = 0; ; i++) {
        if (i >= 12) {
            break;
        }
        if (i % 2 == 0) {
            continue;
        }
        printf("%d ", i);
    }
    putchar('\n');

    /* switch picks one case. */
    for (int day = 0; day < 3; day++) {
        switch (day) {
        case 0:
            puts("Monday");
            break;
        case 1:
            puts("Tuesday");
            break;
        default:
            puts("Another day");
            break;
        }
    }

    /* Nested loops: a small multiplication table. */
    for (int row = 1; row <= 3; row++) {
        for (int column = 1; column <= 4; column++) {
            printf("%4d", row * column);
        }
        putchar('\n');
    }
    return 0;
}
