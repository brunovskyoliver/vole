/* Vole teaching runtime: output, a printf subset, memory helpers and the
 * integer-division support routines Clang expects on 32-bit guests.
 *
 * Always compiled at -O0. On 32-bit guests the code avoids '/' and '%' so that
 * it never needs the division helpers it defines; it uses shift-and-subtract.
 */
#include <stdarg.h>
#include <stddef.h>
#include <vole.h>

typedef unsigned long long vole_u64;

/* C library names are weak, so a document may define its own strlen or memcpy.
 * The runtime itself only calls its private helpers. */
#define VOLE_WEAK __attribute__((weak))

static size_t vole_strlen(const char *text) {
    size_t length = 0;
    while (text[length]) {
        length++;
    }
    return length;
}

/* ---- Teaching system calls ---------------------------------------------- */

static void vole_write_byte(const char *byte) {
#if defined(__aarch64__)
    __asm__ volatile("mov x0, #1\n\tmov x1, %0\n\tmov x2, #1\n\tmov x8, #64\n\tsvc #0"
                     :
                     : "r"(byte)
                     : "x0", "x1", "x2", "x8", "memory");
#elif defined(__arm__)
    __asm__ volatile("mov r0, #1\n\tmov r1, %0\n\tmov r2, #1\n\tmov r7, #4\n\tsvc #0"
                     :
                     : "r"(byte)
                     : "r0", "r1", "r2", "r7", "memory");
#elif defined(__x86_64__)
    __asm__ volatile("movq %0, %%rsi\n\tmovl $1, %%eax\n\tmovl $1, %%edi\n\tmovl $1, %%edx\n\tsyscall"
                     :
                     : "r"(byte)
                     : "rax", "rdi", "rsi", "rdx", "rcx", "r11", "memory");
#elif defined(__i386__)
    __asm__ volatile("movl %0, %%ecx\n\tmovl $4, %%eax\n\tmovl $1, %%ebx\n\tmovl $1, %%edx\n\tint $0x80"
                     :
                     : "r"(byte)
                     : "eax", "ebx", "ecx", "edx", "memory");
#else
#error "Unsupported Vole target"
#endif
}

_Noreturn void vole_exit(int status) {
    long value = status;
#if defined(__aarch64__)
    __asm__ volatile("mov x0, %0\n\tmov x8, #93\n\tsvc #0" : : "r"(value) : "x0", "x8", "memory");
#elif defined(__arm__)
    __asm__ volatile("mov r0, %0\n\tmov r7, #1\n\tsvc #0" : : "r"(value) : "r0", "r7", "memory");
#elif defined(__x86_64__)
    __asm__ volatile("movq %0, %%rdi\n\tmovl $60, %%eax\n\tsyscall"
                     :
                     : "r"(value)
                     : "rax", "rdi", "rcx", "r11", "memory");
#elif defined(__i386__)
    __asm__ volatile("movl %0, %%ebx\n\tmovl $1, %%eax\n\tint $0x80" : : "r"(value) : "eax", "ebx", "memory");
#endif
    for (;;) {
    }
}

/* ---- Division without hardware support ---------------------------------- */

/* Division by zero yields quotient 0 and leaves the numerator as remainder,
 * like the ARM64 UDIV instruction. */
static vole_u64 vole_udivmod64(vole_u64 numerator, vole_u64 denominator, vole_u64 *remainder) {
    vole_u64 quotient = 0;
    vole_u64 rest = 0;
    int bit;
    if (denominator == 0) {
        if (remainder) {
            *remainder = numerator;
        }
        return 0;
    }
    for (bit = 63; bit >= 0; bit--) {
        rest = (rest << 1) | ((numerator >> bit) & 1);
        if (rest >= denominator) {
            rest -= denominator;
            quotient |= (vole_u64)1 << bit;
        }
    }
    if (remainder) {
        *remainder = rest;
    }
    return quotient;
}

static unsigned vole_udivmod32(unsigned numerator, unsigned denominator, unsigned *remainder) {
    unsigned quotient = 0;
    unsigned rest = 0;
    int bit;
    if (denominator == 0) {
        if (remainder) {
            *remainder = numerator;
        }
        return 0;
    }
    for (bit = 31; bit >= 0; bit--) {
        rest = (rest << 1) | ((numerator >> bit) & 1);
        if (rest >= denominator) {
            rest -= denominator;
            quotient |= 1u << bit;
        }
    }
    if (remainder) {
        *remainder = rest;
    }
    return quotient;
}

static long long vole_divmod64(long long numerator, long long denominator, long long *remainder) {
    int negative_quotient = (numerator < 0) != (denominator < 0);
    int negative_remainder = numerator < 0;
    vole_u64 n = numerator < 0 ? 0 - (vole_u64)numerator : (vole_u64)numerator;
    vole_u64 d = denominator < 0 ? 0 - (vole_u64)denominator : (vole_u64)denominator;
    vole_u64 rest;
    vole_u64 quotient = vole_udivmod64(n, d, &rest);
    if (remainder) {
        *remainder = (long long)(negative_remainder ? 0 - rest : rest);
    }
    return (long long)(negative_quotient ? 0 - quotient : quotient);
}

static int vole_divmod32(int numerator, int denominator, int *remainder) {
    int negative_quotient = (numerator < 0) != (denominator < 0);
    int negative_remainder = numerator < 0;
    unsigned n = numerator < 0 ? 0u - (unsigned)numerator : (unsigned)numerator;
    unsigned d = denominator < 0 ? 0u - (unsigned)denominator : (unsigned)denominator;
    unsigned rest;
    unsigned quotient = vole_udivmod32(n, d, &rest);
    if (remainder) {
        *remainder = (int)(negative_remainder ? 0u - rest : rest);
    }
    return (int)(negative_quotient ? 0u - quotient : quotient);
}

#if defined(__arm__)
/* ARM run-time ABI helpers. A 64-bit return value occupies r0 (low) and r1,
 * which is exactly the {quotient, remainder} pair the *divmod helpers return. */
int __aeabi_idiv(int numerator, int denominator) {
    return vole_divmod32(numerator, denominator, 0);
}

unsigned __aeabi_uidiv(unsigned numerator, unsigned denominator) {
    return vole_udivmod32(numerator, denominator, 0);
}

vole_u64 __aeabi_idivmod(int numerator, int denominator) {
    int remainder;
    unsigned quotient = (unsigned)vole_divmod32(numerator, denominator, &remainder);
    return ((vole_u64)(unsigned)remainder << 32) | quotient;
}

vole_u64 __aeabi_uidivmod(unsigned numerator, unsigned denominator) {
    unsigned remainder;
    unsigned quotient = vole_udivmod32(numerator, denominator, &remainder);
    return ((vole_u64)remainder << 32) | quotient;
}

/* Called by the __aeabi_ldivmod/__aeabi_uldivmod shims in crt0, which return
 * the quotient in r0:r1 and the remainder in r2:r3. */
long long __vole_ldivmod(long long numerator, long long denominator, long long *remainder) {
    return vole_divmod64(numerator, denominator, remainder);
}

vole_u64 __vole_uldivmod(vole_u64 numerator, vole_u64 denominator, vole_u64 *remainder) {
    return vole_udivmod64(numerator, denominator, remainder);
}
#endif

#if defined(__i386__)
/* libgcc-compatible 64-bit division helpers for IA-32. */
long long __divdi3(long long numerator, long long denominator) {
    return vole_divmod64(numerator, denominator, 0);
}

long long __moddi3(long long numerator, long long denominator) {
    long long remainder;
    vole_divmod64(numerator, denominator, &remainder);
    return remainder;
}

vole_u64 __udivdi3(vole_u64 numerator, vole_u64 denominator) {
    return vole_udivmod64(numerator, denominator, 0);
}

vole_u64 __umoddi3(vole_u64 numerator, vole_u64 denominator) {
    vole_u64 remainder;
    vole_udivmod64(numerator, denominator, &remainder);
    return remainder;
}
#endif

/* ---- Text output --------------------------------------------------------- */

void vole_putc(char c) {
    vole_write_byte(&c);
}

void vole_print(const char *text) {
    while (*text) {
        vole_putc(*text++);
    }
}

void vole_println(const char *text) {
    vole_print(text);
    vole_putc('\n');
}

/* Writes the digits of value in base (2..16) to buffer, least significant
 * first, and returns how many were written. */
static int vole_digits(char *buffer, vole_u64 value, unsigned base, int upper) {
    const char *symbols = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    int count = 0;
    do {
        unsigned digit;
#if defined(__aarch64__) || defined(__x86_64__)
        /* 64-bit guests divide in hardware. */
        digit = (unsigned)(value % base);
        value = value / base;
#else
        if ((value >> 32) == 0) {
            value = vole_udivmod32((unsigned)value, base, &digit);
        } else {
            vole_u64 rest;
            value = vole_udivmod64(value, base, &rest);
            digit = (unsigned)rest;
        }
#endif
        buffer[count++] = symbols[digit];
    } while (value != 0);
    return count;
}

void vole_print_uint(unsigned long value) {
    char buffer[24];
    int count = vole_digits(buffer, value, 10, 0);
    while (count > 0) {
        vole_putc(buffer[--count]);
    }
}

void vole_print_int(long value) {
    unsigned long magnitude = (unsigned long)value;
    if (value < 0) {
        vole_putc('-');
        magnitude = 0ul - magnitude;
    }
    vole_print_uint(magnitude);
}

void vole_print_hex(unsigned long value) {
    char buffer[24];
    int count = vole_digits(buffer, value, 16, 0);
    vole_print("0x");
    while (count > 0) {
        vole_putc(buffer[--count]);
    }
}

VOLE_WEAK int putchar(int c) {
    vole_putc((char)c);
    return (unsigned char)c;
}

VOLE_WEAK int puts(const char *text) {
    vole_println(text);
    return 1;
}

static int vole_pad(char fill, int count) {
    int written = 0;
    while (written < count) {
        vole_putc(fill);
        written++;
    }
    return written;
}

/* Prints prefix (sign or 0x) and digits with the requested width/flags. */
static int vole_field(const char *prefix, const char *text, int length, int reversed, int width,
                      int left, int zero) {
    int prefix_length = (int)vole_strlen(prefix);
    int padding = width - prefix_length - length;
    int written = 0;
    int index;
    if (padding < 0) {
        padding = 0;
    }
    if (!left && !zero) {
        written += vole_pad(' ', padding);
    }
    vole_print(prefix);
    written += prefix_length;
    if (!left && zero) {
        written += vole_pad('0', padding);
    }
    for (index = 0; index < length; index++) {
        vole_putc(reversed ? text[length - 1 - index] : text[index]);
    }
    written += length;
    if (left) {
        written += vole_pad(' ', padding);
    }
    return written;
}

VOLE_WEAK int printf(const char *format, ...) {
    va_list arguments;
    int written = 0;
    va_start(arguments, format);
    while (*format) {
        char buffer[24];
        int left = 0;
        int zero = 0;
        int width = 0;
        int size = 0; /* -2 hh, -1 h, 0 int, 1 l, 2 ll, 3 z */
        vole_u64 value;
        int negative = 0;
        char conversion;
        if (*format != '%') {
            vole_putc(*format++);
            written++;
            continue;
        }
        format++;
        for (;; format++) {
            if (*format == '-') {
                left = 1;
            } else if (*format == '0') {
                zero = 1;
            } else {
                break;
            }
        }
        while (*format >= '0' && *format <= '9') {
            width = width * 10 + (*format++ - '0');
        }
        if (*format == 'h') {
            size = -1;
            if (*++format == 'h') {
                size = -2;
                format++;
            }
        } else if (*format == 'l') {
            size = 1;
            if (*++format == 'l') {
                size = 2;
                format++;
            }
        } else if (*format == 'z') {
            size = 3;
            format++;
        }
        conversion = *format;
        if (conversion == '\0') {
            break;
        }
        format++;
        switch (conversion) {
        case 'd':
        case 'i': {
            long long number;
            if (size == 2) {
                number = va_arg(arguments, long long);
            } else if (size == 1) {
                number = va_arg(arguments, long);
            } else if (size == 3) {
                number = (long)va_arg(arguments, size_t);
            } else {
                number = va_arg(arguments, int);
                if (size == -1) {
                    number = (short)number;
                } else if (size == -2) {
                    number = (signed char)number;
                }
            }
            negative = number < 0;
            value = negative ? 0 - (vole_u64)number : (vole_u64)number;
            written += vole_field(negative ? "-" : "", buffer, vole_digits(buffer, value, 10, 0), 1,
                                  width, left, zero);
            break;
        }
        case 'u':
        case 'x':
        case 'X':
        case 'o': {
            unsigned base = conversion == 'u' ? 10 : conversion == 'o' ? 8 : 16;
            if (size == 2) {
                value = va_arg(arguments, unsigned long long);
            } else if (size == 1) {
                value = va_arg(arguments, unsigned long);
            } else if (size == 3) {
                value = va_arg(arguments, size_t);
            } else {
                value = va_arg(arguments, unsigned);
                if (size == -1) {
                    value = (unsigned short)value;
                } else if (size == -2) {
                    value = (unsigned char)value;
                }
            }
            written += vole_field("", buffer, vole_digits(buffer, value, base, conversion == 'X'),
                                  1, width, left, zero);
            break;
        }
        case 'p':
            value = (unsigned long)va_arg(arguments, void *);
            written += vole_field("0x", buffer, vole_digits(buffer, value, 16, 0), 1, width, left,
                                  zero);
            break;
        case 'c':
            buffer[0] = (char)va_arg(arguments, int);
            written += vole_field("", buffer, 1, 0, width, left, 0);
            break;
        case 's': {
            const char *text = va_arg(arguments, const char *);
            if (!text) {
                text = "(null)";
            }
            written += vole_field("", text, (int)vole_strlen(text), 0, width, left, 0);
            break;
        }
        case '%':
            vole_putc('%');
            written++;
            break;
        default:
            /* Unsupported conversion: print it unchanged. */
            vole_putc('%');
            vole_putc(conversion);
            written += 2;
            break;
        }
    }
    va_end(arguments);
    return written;
}

/* ---- Memory and strings -------------------------------------------------- */

VOLE_WEAK void *memset(void *destination, int value, size_t count) {
    unsigned char *bytes = destination;
    while (count--) {
        *bytes++ = (unsigned char)value;
    }
    return destination;
}

VOLE_WEAK void *memcpy(void *destination, const void *source, size_t count) {
    unsigned char *to = destination;
    const unsigned char *from = source;
    while (count--) {
        *to++ = *from++;
    }
    return destination;
}

VOLE_WEAK void *memmove(void *destination, const void *source, size_t count) {
    unsigned char *to = destination;
    const unsigned char *from = source;
    if (to < from) {
        while (count--) {
            *to++ = *from++;
        }
    } else if (to > from) {
        while (count--) {
            to[count] = from[count];
        }
    }
    return destination;
}

#if defined(__arm__)
/* ARM run-time ABI memory helpers. Clang calls these for struct copies and
 * zero-initialized aggregates. Note the (destination, count, value) order. */
void __aeabi_memcpy(void *destination, const void *source, size_t count) {
    memcpy(destination, source, count);
}
void __aeabi_memcpy4(void *destination, const void *source, size_t count) {
    memcpy(destination, source, count);
}
void __aeabi_memcpy8(void *destination, const void *source, size_t count) {
    memcpy(destination, source, count);
}
void __aeabi_memmove(void *destination, const void *source, size_t count) {
    memmove(destination, source, count);
}
void __aeabi_memmove4(void *destination, const void *source, size_t count) {
    memmove(destination, source, count);
}
void __aeabi_memmove8(void *destination, const void *source, size_t count) {
    memmove(destination, source, count);
}
void __aeabi_memset(void *destination, size_t count, int value) {
    memset(destination, value, count);
}
void __aeabi_memset4(void *destination, size_t count, int value) {
    memset(destination, value, count);
}
void __aeabi_memset8(void *destination, size_t count, int value) {
    memset(destination, value, count);
}
void __aeabi_memclr(void *destination, size_t count) {
    memset(destination, 0, count);
}
void __aeabi_memclr4(void *destination, size_t count) {
    memset(destination, 0, count);
}
void __aeabi_memclr8(void *destination, size_t count) {
    memset(destination, 0, count);
}
#endif


VOLE_WEAK int memcmp(const void *left, const void *right, size_t count) {
    const unsigned char *a = left;
    const unsigned char *b = right;
    size_t index;
    for (index = 0; index < count; index++) {
        if (a[index] != b[index]) {
            return a[index] < b[index] ? -1 : 1;
        }
    }
    return 0;
}

VOLE_WEAK size_t strlen(const char *text) {
    return vole_strlen(text);
}
