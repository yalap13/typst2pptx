typst2pptx
==========

Convert Typst documents into PowerPoint (`.pptx`) files. It walks the Typst layout tree and recreates text, shapes, images, and equations in PowerPoint using `python-pptx`, rasterizing only when PowerPoint cannot represent a transform faithfully.

Requirements
------------
- Python 3.12+
- Rust toolchain (for the PyO3 extension)
- `uv` for the suggested install path

We are currently working to make this tool available on PyPI to avoid the need for Rust at install time.

Installation (user)
-------------------
Clone the repo, then install the tool into your environment with `uv`:

```
git clone https://github.com/…/typst2pptx.git
cd typst2pptx
uv tool install .
```

That builds the native extension with `maturin` behind the scenes and installs a `typst2pptx` CLI.

Usage
-----
Basic CLI:

```
typst2pptx path/to/doc.typ
```

Options:
- `-o/--output`: override output path (defaults to `<sourcename>.pptx`).
- `--equations-dir PATH`: keep rendered equation PNGs under `PATH` instead of a temp dir; when omitted, they are written to a temp `typst2pptx-equations-<timestamp>-<pid>` folder and cleaned up afterward.

Notes:
- Typst packages are fetched as needed; set `CACHE_DIRECTORY` to control where they are cached.
- Hyperlinks are currently dropped (`FrameItem::Link` is ignored); most geometry, rotation, and scaling are preserved, with rasterization used for skewed text/images or complex shapes.
- There is a sample Typst source at `src/source.typ`; try `typst2pptx src/source.typ -o my_presentation.pptx`.

How it works (short version)
----------------------------
- Compiles the Typst document, walks each page frame, and maps elements into PowerPoint shapes.
- Equations are isolated, rendered to PNG at 300 DPI, and reinserted at their original coordinates.
- When PowerPoint cannot express a transform (e.g., non-uniform scaling or skew), the affected content is rasterized to preserve layout.
- Text stays editable where possible (e.g. when not skewed or scaled non-uniformly).

Contributing
------------
- Make your changes, then rerun `maturin develop` after touching Rust code so the Python module is rebuilt.
