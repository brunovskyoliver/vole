/* Arrays, pointers, structs, unions, strings and static data. */
#include <stdint.h>
#include <vole.h>

typedef unsigned long long u64;
u64 results[64];
int result_count;
static void put(u64 value) { results[result_count++] = value; }

struct point {
    short x;
    char tag;
    long long weight;
    int y;
};

struct big {
    int values[20];
    struct point corner;
};

union word {
    unsigned u;
    unsigned char bytes[4];
    unsigned short halves[2];
};

static const int table[3][4] = {{1, 2, 3, 4}, {5, 6, 7, 8}, {9, 10, 11, 12}};
volatile int scale = 3;
static struct big shared;
const char *const greeting = "Vole C!";

__attribute__((noinline)) void matmul(int out[3][3], const int a[3][4], const int b[4][3]) {
    int i, j, k;
    for (i = 0; i < 3; i++)
        for (j = 0; j < 3; j++) {
            out[i][j] = 0;
            for (k = 0; k < 4; k++) out[i][j] += a[i][k] * b[k][j];
        }
}

__attribute__((noinline)) struct point make_point(short x, int y, char tag) {
    struct point p;
    p.x = x;
    p.y = y;
    p.tag = tag;
    p.weight = (long long)x * y;
    return p;
}

__attribute__((noinline)) struct big copy_big(struct big value) {
    value.values[19] += 1;
    return value;
}

__attribute__((noinline)) long sum_bytes(const unsigned char *p, const unsigned char *end) {
    long total = 0;
    while (p < end) total += *p++;
    return total;
}

__attribute__((noinline)) int counter(void) {
    static int calls = 10;
    return ++calls;
}

__attribute__((noinline)) void reverse(char *text) {
    char *end = text + strlen(text) - 1;
    while (text < end) {
        char t = *text;
        *text++ = *end;
        *end-- = t;
    }
}

int main(void) {
    int b[4][3];
    int out[3][3];
    int i, j;
    char text[16];
    struct point p, q;
    struct big copy;
    union word w;
    for (i = 0; i < 4; i++)
        for (j = 0; j < 3; j++) b[i][j] = (i + 1) * scale - j;
    matmul(out, table, b);
    for (i = 0; i < 3; i++)
        for (j = 0; j < 3; j++) put((u64)(long long)out[i][j]);
    p = make_point(-300, 70000, 'q');
    q = p;
    q.x += 1;
    put((u64)(long long)p.x);
    put((u64)(long long)q.x);
    put((u64)p.weight);
    put((u64)(unsigned char)q.tag);
    for (i = 0; i < 20; i++) shared.values[i] = i * scale;
    shared.corner = p;
    copy = copy_big(shared);
    put((u64)copy.values[19]);
    put((u64)shared.values[19]);
    put((u64)(long long)copy.corner.y);
    w.u = 0x11223344u * (unsigned)scale;
    put(w.bytes[0] + (w.bytes[3] << 8) + ((u64)w.halves[1] << 16));
    put((u64)sum_bytes((const unsigned char *)greeting, (const unsigned char *)greeting + 7));
    memcpy(text, greeting, strlen(greeting) + 1);
    reverse(text);
    put((u64)text[0] | (u64)text[6] << 8 | (u64)memcmp(text, "!C eloV", 8) << 16);
    memset(text, 'z', 4);
    memmove(text + 1, text, 6);
    put((u64)text[0] | (u64)text[4] << 8 | (u64)text[6] << 16);
    {
        int first = counter();
        int second = counter();
        put((u64)first + (u64)second * 100);
    }
    {
        int *ptr = &out[1][0];
        put((u64)(ptr[4] - ptr[-1]));
        put((u64)(&out[2][2] - &out[0][0]));
    }
    return result_count;
}
