/* Control flow, casts, recursion, switch tables and mixed-width products. */
#include <stdbool.h>
#include <stdint.h>
#include <vole.h>

typedef unsigned long long u64;
u64 results[128];
int result_count;
static void put(u64 value) { results[result_count++] = value; }

volatile int seed = 7;
volatile long long wide_seed = -123456789012LL;

__attribute__((noinline)) int dense_switch(int x) {
    switch (x) {
    case 0: return 11;
    case 1: return 23;
    case 2: return 37;
    case 3: return 41;
    case 4: return 59;
    case 5: return 61;
    case 6: return 73;
    case 7: return 89;
    case 8: return 97;
    default: return -1;
    }
}

__attribute__((noinline)) int sparse_switch(unsigned x) {
    switch (x) {
    case 3: return 1;
    case 100: return 2;
    case 1000: return 3;
    case 65536: return 4;
    case 0xffffffffu: return 5;
    default: return 0;
    }
}

__attribute__((noinline)) unsigned fib(unsigned n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }

__attribute__((noinline)) int gcd(int a, int b) {
    while (b != 0) {
        int t = a % b;
        a = b;
        b = t;
    }
    return a;
}

__attribute__((noinline)) bool is_prime(unsigned n) {
    unsigned d;
    if (n < 2) return false;
    for (d = 2; d * d <= n; d++) {
        if (n % d == 0) return false;
    }
    return true;
}

__attribute__((noinline)) long long widening_signed(int a, int b) { return (long long)a * b; }
__attribute__((noinline)) u64 widening_unsigned(unsigned a, unsigned b) { return (u64)a * b; }
__attribute__((noinline)) u64 shift_left64(u64 v, unsigned s) { return v << (s & 63); }
__attribute__((noinline)) u64 shift_right64(u64 v, unsigned s) { return v >> (s & 63); }
__attribute__((noinline)) long long shift_arith64(long long v, unsigned s) { return v >> (s & 63); }
__attribute__((noinline)) int compare64(long long a, long long b) {
    return (a < b) + 2 * (a == b) + 4 * ((u64)a < (u64)b);
}

__attribute__((noinline)) int count_down(int n) {
    int total = 0;
    do {
        if (n % 3 == 0) {
            n--;
            continue;
        }
        if (n == 2) break;
        total += n;
        n--;
    } while (n > 0);
    return total;
}

__attribute__((noinline)) int with_goto(int n) {
    int steps = 0;
again:
    if (n != 1) {
        n = (n & 1) ? 3 * n + 1 : n / 2;
        steps++;
        goto again;
    }
    return steps;
}

__attribute__((noinline)) u64 casts(long long v) {
    signed char c = (signed char)v;
    unsigned char uc = (unsigned char)v;
    short s = (short)v;
    unsigned short us = (unsigned short)v;
    int i = (int)v;
    unsigned u = (unsigned)v;
    bool b = v & 0x100;
    long long sum = (long long)c + (long long)uc + (long long)s + (long long)us + (long long)i +
                    (long long)u + b;
    return (u64)sum;
}

typedef int (*binary)(int, int);
static int add_op(int a, int b) { return a + b; }
static int sub_op(int a, int b) { return a - b; }
static int mul_op(int a, int b) { return a * b; }
static const binary operations[] = {add_op, sub_op, mul_op};

int main(void) {
    int i;
    for (i = -1; i < 10; i++) put((u64)(long long)dense_switch(i));
    put(sparse_switch(3) + 10 * sparse_switch(100) + 100 * sparse_switch(1000) +
        1000 * sparse_switch(65536) + 10000 * sparse_switch(0xffffffffu) + 100000 * sparse_switch(seed));
    put(fib(seed + 8));
    put((u64)gcd(1071 * seed, 462 * seed));
    {
        unsigned n, count = 0;
        for (n = 0; n < 200; n++) count += is_prime(n);
        put(count);
    }
    put((u64)widening_signed(-seed * 100000, 300000));
    put(widening_unsigned(0xfffffff0u + seed, 0xffffff00u));
    for (i = 0; i < 64; i += 13) {
        put(shift_left64((u64)wide_seed, i + seed));
        put(shift_right64((u64)wide_seed, i + seed));
        put((u64)shift_arith64(wide_seed, i + seed));
    }
    put((u64)compare64(wide_seed, 5));
    put((u64)compare64(5, wide_seed));
    put((u64)compare64(wide_seed, wide_seed));
    put((u64)count_down(seed * 3));
    put((u64)with_goto(seed * 3));
    put(casts(wide_seed));
    put(casts(0x1ff80));
    for (i = 0; i < 3; i++) put((u64)(long long)operations[(seed + i) % 3](seed, -4));
    return result_count;
}
