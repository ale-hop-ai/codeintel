"""Default arguments that the def_sig character scan cannot read (#74).

Its own file, and one SCIP has no document for, deliberately: growing a file
that `index.scip` covers re-attributes its module-scope references (#75).
"""


def retry(delay=1 << 4, loud=True) -> bool:
    """A shift in a default argument: `<<` is not two open brackets."""
    return loud and delay > 0


def halve(delay=8 >> 1, loud=True) -> bool:
    """The other direction: `>>` is not two closers either."""
    return loud and delay > 0


def pick(first=1 < 2, second=2) -> int:
    """A bare comparison, which no character scan can tell from a generic."""
    return second if first else 0
