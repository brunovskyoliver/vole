/* Idioms that -O1 lowers to special instructions: division by constants,
 * rotates, byte swaps, bit counting, min/max/abs selects and bitfields. */
#include <stdint.h>
#include <vole.h>

typedef unsigned long long u64;
u64 results[96];
int result_count;
static void put(u64 value) { results[result_count++] = value; }

volatile int ivalues[] = {-1000003, 77, 0x7fffffff, -2147483647 - 1, 0, 12345};
volatile long long lvalues[] = {-99999999999LL, 1234567890123LL, 0x7fffffffffffffffLL, 3};

struct flags {
    unsigned low : 3;
    signed mid : 7;
    unsigned high : 12;
    unsigned top : 10;
};

__attribute__((noinline)) int div7(int x) { return x / 7; }
__attribute__((noinline)) int mod10(int x) { return x % 10; }
__attribute__((noinline)) unsigned udiv10(unsigned x) { return x / 10; }
__attribute__((noinline)) unsigned umod3(unsigned x) { return x % 3; }
__attribute__((noinline)) int div8(int x) { return x / 8; }
__attribute__((noinline)) long long ldiv7(long long x) { return x / 7; }
__attribute__((noinline)) u64 uldiv10(u64 x) { return x / 10; }
__attribute__((noinline)) long long lmod1000(long long x) { return x % 1000; }
__attribute__((noinline)) unsigned rotl(unsigned x, unsigned s) { return (x << (s & 31)) | (x >> ((32 - s) & 31)); }
__attribute__((noinline)) u64 rotr64(u64 x, unsigned s) { return (x >> (s & 63)) | (x << ((64 - s) & 63)); }
__attribute__((noinline)) unsigned bswap(unsigned x) {
    return (x >> 24) | ((x >> 8) & 0xff00) | ((x << 8) & 0xff0000) | (x << 24);
}
__attribute__((noinline)) int leading_zeros(unsigned x) {
    int n = 0;
    if (x == 0) return 32;
    while (!(x & 0x80000000u)) {
        x <<= 1;
        n++;
    }
    return n;
}
__attribute__((noinline)) int popcount(u64 x) {
    int n = 0;
    while (x) {
        x &= x - 1;
        n++;
    }
    return n;
}
__attribute__((noinline)) int imin(int a, int b) { return a < b ? a : b; }
__attribute__((noinline)) unsigned umax(unsigned a, unsigned b) { return a > b ? a : b; }
__attribute__((noinline)) long long labs64(long long a) { return a < 0 ? -a : a; }
__attribute__((noinline)) int iabs(int a) { return a < 0 ? -a : a; }
__attribute__((noinline)) int sign(int a) { return (a > 0) - (a < 0); }
__attribute__((noinline)) unsigned pack(struct flags *f, unsigned v) {
    f->low = v;
    f->mid = (int)v >> 3;
    f->high = v >> 10;
    f->top += 1;
    return f->low + (unsigned)(f->mid * 1000) + f->high * 7 + f->top;
}
__attribute__((noinline)) int is_lower(char c) { return c >= 'a' && c <= 'z'; }
__attribute__((noinline)) unsigned short swap16(unsigned short x) { return (unsigned short)(x << 8 | x >> 8); }
__attribute__((noinline)) signed char narrow(int x) { return (signed char)((unsigned)x * 3u); }
__attribute__((noinline)) unsigned long long mulhi(u64 a, u64 b) {
    u64 al = a & 0xffffffffu, ah = a >> 32, bl = b & 0xffffffffu, bh = b >> 32;
    u64 mid = (al * bl >> 32) + (ah * bl & 0xffffffffu) + (al * bh & 0xffffffffu);
    return ah * bh + (ah * bl >> 32) + (al * bh >> 32) + (mid >> 32);
}

int main(void) {
    int i;
    struct flags f = {1, 2, 3, 1023};
    for (i = 0; i < 6; i++) {
        int x = ivalues[i];
        put((u64)(long long)div7(x));
        put((u64)(long long)mod10(x));
        put(udiv10((unsigned)x));
        put(umod3((unsigned)x));
        put((u64)(long long)div8(x));
        put(rotl((unsigned)x, (unsigned)i * 7));
        put(bswap((unsigned)x));
        put((u64)leading_zeros((unsigned)x));
        put((u64)(long long)imin(x, 5) ^ (u64)umax((unsigned)x, 1000u) << 32);
        put((u64)(long long)iabs(x == -2147483647 - 1 ? 1 : x) + (u64)(long long)sign(x) * 1000);
        put(pack(&f, (unsigned)x));
        put((u64)is_lower((char)((unsigned)x + 'a')) + swap16((unsigned short)x) * 2 + ((u64)(unsigned char)narrow(x) << 20));
    }
    for (i = 0; i < 4; i++) {
        long long x = lvalues[i];
        put((u64)ldiv7(x));
        put(uldiv10((u64)x));
        put((u64)lmod1000(x));
        put(rotr64((u64)x, (unsigned)i * 21));
        put((u64)popcount((u64)x));
        put((u64)labs64(x));
        put(mulhi((u64)x, 0x9e3779b97f4a7c15ULL));
    }
    return result_count;
}
