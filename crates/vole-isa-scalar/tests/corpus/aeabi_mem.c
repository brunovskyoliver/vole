/* ARM EABI memory helpers that Clang calls for ARM32 struct copies. Weak, so
 * a runtime that provides them takes precedence. */
#include <stddef.h>

__attribute__((weak)) void __aeabi_memcpy(void *destination, const void *source, size_t count) {
    unsigned char *d = destination;
    const unsigned char *s = source;
    while (count--) *d++ = *s++;
}
__attribute__((weak)) void __aeabi_memcpy4(void *d, const void *s, size_t n) { __aeabi_memcpy(d, s, n); }
__attribute__((weak)) void __aeabi_memcpy8(void *d, const void *s, size_t n) { __aeabi_memcpy(d, s, n); }
__attribute__((weak)) void __aeabi_memset(void *destination, size_t count, int value) {
    unsigned char *d = destination;
    while (count--) *d++ = (unsigned char)value;
}
__attribute__((weak)) void __aeabi_memset4(void *d, size_t n, int v) { __aeabi_memset(d, n, v); }
__attribute__((weak)) void __aeabi_memset8(void *d, size_t n, int v) { __aeabi_memset(d, n, v); }
__attribute__((weak)) void __aeabi_memclr(void *d, size_t n) { __aeabi_memset(d, n, 0); }
__attribute__((weak)) void __aeabi_memclr4(void *d, size_t n) { __aeabi_memset(d, n, 0); }
__attribute__((weak)) void __aeabi_memclr8(void *d, size_t n) { __aeabi_memset(d, n, 0); }
