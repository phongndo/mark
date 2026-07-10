# Python stress fixture with non-ASCII text: café λ🚀.

import re

ROUTE = re.compile(
    r"""^/api/(?P<name>[\w-]+)
    # verbose-regex comment that spans the raw triple-quoted string
    $""",
    re.VERBOSE,
)

DOC = """multi-line string
with café and "quotes"
and emoji 🚀
"""


def ratio(total: float, count: float) -> float:
    # regular comment before a division expression
    return total / count


class Greeter:
    def __init__(self, name: str = "λ") -> None:
        self.name = name

    def __str__(self) -> str:
        return f"{self.name}: {DOC.strip()} {ROUTE.match('/api/café') is not None}"
