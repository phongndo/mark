#include <iostream>
#include <string>

#define MARK_STRESS(name) void name()
#if __cplusplus >= 202002L
#  define HAS_MODERN_CPP 1
#else
#  define HAS_MODERN_CPP 0
#endif

/* C++ stress fixture with non-ASCII text: café λ🚀. */
static const std::string pattern = R"regex(^/api/([\w-]+)/(?:"quoted")$)regex";
static const std::string html = R"HTML(<div data-title="λ🚀">
  <span>{{ value }}</span>
</div>)HTML";

MARK_STRESS(run) {
    auto ratio = 42 / 7 / 2;
    std::cout << pattern << html << ratio << '\n';
}
