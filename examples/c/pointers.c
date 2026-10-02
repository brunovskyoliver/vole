/* Pointers hold memory addresses. Watch the Memory view while stepping. */
#include <vole.h>

struct point {
    int x;
    int y;
};

void swap(int *a, int *b) {
    int temporary = *a;
    *a = *b;
    *b = temporary;
}

/* A pointer to a struct lets a function change its fields. */
void move(struct point *p, int dx, int dy) {
    p->x += dx;
    p->y += dy;
}

/* Walk a string with a pointer until the terminating zero byte. */
int count_letter(const char *text, char letter) {
    int count = 0;
    for (const char *p = text; *p != '\0'; p++) {
        if (*p == letter) {
            count++;
        }
    }
    return count;
}

int main(void) {
    int a = 1;
    int b = 2;
    int *pa = &a;

    *pa = 10; /* writes to a through the pointer */
    printf("a = %d, b = %d\n", a, b);
    swap(&a, &b);
    printf("after swap: a = %d, b = %d\n", a, b);

    /* Pointer arithmetic moves by whole elements. */
    int values[4] = {5, 10, 15, 20};
    int *p = values;
    printf("*p = %d, *(p + 2) = %d, p[3] = %d\n", *p, *(p + 2), p[3]);
    printf("elements between: %d\n", (int)(&values[3] - &values[0]));

    struct point spot = {3, 4};
    move(&spot, 2, -1);
    printf("spot = (%d, %d)\n", spot.x, spot.y);

    printf("letters 's' in \"mississippi\": %d\n", count_letter("mississippi", 's'));
    printf("a pointer is %d bytes on this machine\n", (int)sizeof(pa));
    return 0;
}
