"""Entry point for the uv/pip-installed command. Search runs in Rust."""

import sys

from ._core import python_main


def main() -> None:
    python_main(sys.argv[1:])
