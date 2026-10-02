/* Functions, nested calls and recursion. Step Into a call to see a new
 * stack frame appear; Step Out to return to the caller. */
#include <vole.h>

int square(int x) {
    return x * x;
}

/* Calls another function: the call stack grows two frames deep. */
int sum_of_squares(int a, int b) {
    return square(a) + square(b);
}

/* Recursion: fib(n) calls itself twice. */
int fib(int n) {
    if (n < 2) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

/* Euclid's algorithm, written recursively. */
int gcd(int a, int b) {
    if (b == 0) {
        return a;
    }
    return gcd(b, a % b);
}

/* A static local keeps its value between calls. */
int next_ticket(void) {
    static int counter = 100;
    counter++;
    return counter;
}

int main(void) {
    printf("square(7) = %d\n", square(7));
    printf("sum_of_squares(3, 4) = %d\n", sum_of_squares(3, 4));
    for (int i = 0; i <= 10; i++) {
        printf("%d ", fib(i));
    }
    putchar('\n');
    printf("gcd(84, 36) = %d\n", gcd(84, 36));
    int first = next_ticket();
    int second = next_ticket();
    printf("tickets: %d, %d\n", first, second);
    return 0;
}
