#include <stdio.h>

#define JOIN(a, b) a ## b
#define MESSAGE "café λ"
#if defined(DEBUG) && DEBUG
#  define TRACE(fmt, ...) fprintf(stderr, fmt "\n", __VA_ARGS__)
#else
#  define TRACE(fmt, ...) ((void)0)
#endif

/* C stress fixture.
 * Multi-line comment for begin/end continuation with non-ASCII text: λ🚀.
 */
static const char *text = "line one\nline two with \"quotes\"";

int main(void) {
    int total = 42;
    int count = 6;
    printf("%s %s %d\n", MESSAGE, text, total / count);
    return 0;
}
