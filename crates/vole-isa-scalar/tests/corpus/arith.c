/* Integer arithmetic over every C integer type. Results are checked against an
 * independent Rust model in tests/compiler_fixtures.rs. */
#include <vole.h>

typedef unsigned long long u64;

u64 results[400];
int result_count;

static void put(u64 value) { results[result_count++] = value; }

#define OPS(NAME, T, U)                                                                 \
    __attribute__((noinline)) void NAME(T a, T b) {                                     \
        unsigned s = (unsigned)b % (unsigned)(sizeof(a + 0) * 8);                     \
        put((u64)(T)(1u * (U)a + (U)b));                                                    \
        put((u64)(T)(1u * (U)a - (U)b));                                                    \
        put((u64)(T)(1u * (U)a * (U)b));                                                    \
        put((u64)(T)(a / b));                                                          \
        put((u64)(T)(a % b));                                                          \
        put((u64)(T)(a & b));                                                          \
        put((u64)(T)(a | b));                                                          \
        put((u64)(T)(a ^ b));                                                          \
        put((u64)(T)(~a));                                                             \
        put((u64)(T)(0u - (U)a));                                                          \
        put((u64)(T)((1u * (U)a) << s));                                                      \
        put((u64)(T)(a >> s));                                                         \
        put((u64)(a < b) | (u64)(a <= b) << 1 | (u64)(a > b) << 2 | (u64)(a >= b) << 3 | \
            (u64)(a == b) << 4 | (u64)(a != b) << 5 | (u64)(!a) << 6 | (u64)(a && b) << 7); \
    }

OPS(ops_schar, signed char, unsigned char)
OPS(ops_uchar, unsigned char, unsigned char)
OPS(ops_short, short, unsigned short)
OPS(ops_ushort, unsigned short, unsigned short)
OPS(ops_int, int, unsigned)
OPS(ops_uint, unsigned, unsigned)
OPS(ops_long, long, unsigned long)
OPS(ops_ulong, unsigned long, unsigned long)
OPS(ops_llong, long long, unsigned long long)
OPS(ops_ullong, unsigned long long, unsigned long long)

/* Inputs come from memory so -O1 cannot fold the operations. */
volatile long long left_inputs[] = {100, -1234567, 0x7fffffffLL, -5, 0x123456789abcdefLL, 255};
volatile long long right_inputs[] = {7, 89, -3, -3, 0x1000000003LL, 13};

int main(void) {
    int i;
    for (i = 0; i < 6; i++) {
        long long a = left_inputs[i];
        long long b = right_inputs[i];
        ops_schar((signed char)a, (signed char)b);
        ops_uchar((unsigned char)a, (unsigned char)b);
        ops_short((short)a, (short)b);
        ops_ushort((unsigned short)a, (unsigned short)b);
        ops_int((int)a, (int)b);
        ops_uint((unsigned)a, (unsigned)b);
        ops_long((long)a, (long)b);
        ops_ulong((unsigned long)a, (unsigned long)b);
        ops_llong(a, b);
        ops_ullong((u64)a, (u64)b);
    }
    return result_count;
}
