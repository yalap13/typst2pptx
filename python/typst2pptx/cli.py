from __future__ import annotations

import argparse
from pathlib import Path
from typing import Iterable, Optional

from .typst2pptx import typst_to_pptx


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="typst2pptx",
        description="Render a Typst document into a PowerPoint file using python-pptx.",
    )
    parser.add_argument(
        "source",
        type=Path,
        help="Path to the Typst source file to convert.",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        help="Where to write the generated PPTX (defaults to SOURCE with a .pptx extension).",
    )
    parser.add_argument(
        "--equations-dir",
        type=Path,
        help="Keep rendered equation PNGs in this directory (by default they are stored in a temporary directory and removed).",
    )
    return parser


def main(argv: Optional[Iterable[str]] = None) -> None:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if not args.source.is_file():
        parser.error(f"Typst source '{args.source}' does not exist or is not a file.")

    output_path = args.output or args.source.with_suffix(".pptx")
    equations_dir = str(args.equations_dir) if args.equations_dir else None

    try:
        typst_to_pptx(str(args.source), str(output_path), equations_dir)
    except Exception as exc:
        parser.exit(1, f"error: {exc}\n")


if __name__ == "__main__":
    main()
