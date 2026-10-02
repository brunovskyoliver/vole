/* Arrays: rows of values stored next to each other in memory. */
#include <vole.h>

#define COUNT 6

int numbers[COUNT] = {31, 4, 15, 9, 26, 5};
int grid[2][3] = {{1, 2, 3}, {4, 5, 6}};

/* Bubble sort: repeatedly swap neighbours that are out of order. */
void sort(int values[], int count) {
    for (int pass = 0; pass < count - 1; pass++) {
        for (int i = 0; i < count - 1 - pass; i++) {
            if (values[i] > values[i + 1]) {
                int temporary = values[i];
                values[i] = values[i + 1];
                values[i + 1] = temporary;
            }
        }
    }
}

void print_all(const int values[], int count) {
    for (int i = 0; i < count; i++) {
        printf("%d ", values[i]);
    }
    putchar('\n');
}

int main(void) {
    print_all(numbers, COUNT);
    sort(numbers, COUNT);
    print_all(numbers, COUNT);

    /* A two-dimensional array is an array of rows. */
    int total = 0;
    for (int row = 0; row < 2; row++) {
        for (int column = 0; column < 3; column++) {
            total += grid[row][column];
        }
    }
    printf("grid total = %d\n", total);

    /* A string is an array of characters ending with a zero byte. */
    char word[] = "stack";
    int length = (int)strlen(word);
    for (int i = 0; i < length / 2; i++) {
        char temporary = word[i];
        word[i] = word[length - 1 - i];
        word[length - 1 - i] = temporary;
    }
    printf("reversed: %s (%d letters)\n", word, length);

    /* A local array lives on the stack. */
    int squares[5];
    for (int i = 0; i < 5; i++) {
        squares[i] = i * i;
    }
    print_all(squares, 5);
    return 0;
}
