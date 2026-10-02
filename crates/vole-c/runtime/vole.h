/* Vole teaching runtime for freestanding C programs.
 *
 * There is no operating system, heap, input or file system. Output goes to the
 * simulator's console through the teaching write call, one byte at a time.
 */
#ifndef VOLE_H
#define VOLE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Text output */
void vole_putc(char c);
void vole_print(const char *text);   /* no newline */
void vole_println(const char *text); /* adds a newline */
void vole_print_int(long value);
void vole_print_uint(unsigned long value);
void vole_print_hex(unsigned long value); /* 0x prefix, lowercase */

/* Stop the machine; the status stays in the exit-call argument register. */
_Noreturn void vole_exit(int status);

/* Small C library subset */
int putchar(int c);
int puts(const char *text); /* adds a newline */
/* Supports %d %i %u %x %X %o %c %s %p %%, the - and 0 flags, a width,
 * and the hh h l ll z length modifiers. */
int printf(const char *format, ...) __attribute__((format(printf, 1, 2)));
void *memset(void *destination, int value, size_t count);
void *memcpy(void *destination, const void *source, size_t count);
void *memmove(void *destination, const void *source, size_t count);
int memcmp(const void *left, const void *right, size_t count);
size_t strlen(const char *text);

#ifdef __cplusplus
}
#endif

#endif
