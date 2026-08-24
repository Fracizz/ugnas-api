"""Compatibility shim. Prefer: from ugnas import Ugnas"""

from ugnas.api import Ugnas
from ugnas.cli import main

__all__ = ["Ugnas", "main"]

if __name__ == "__main__":
    raise SystemExit(main())
