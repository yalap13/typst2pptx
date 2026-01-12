typst2pptx
==========

Convert Typst documents into PowerPoint (`.pptx`) files. It walks the Typst layout tree and recreates text, shapes, images, and equations in PowerPoint using `python-pptx`, rasterizing only when PowerPoint cannot represent a transform faithfully.

Requirements
------------
- Python 3.12+
- Rust toolchain (for the PyO3 extension)
- `uv` for the suggested installation method

We are currently working to make this tool available on PyPI to avoid the need for Rust at install time.

Installation (user)
-------------------
Clone the repo, then install the tool using `maturin` and `uv` (this installs the tool globally):

```
git clone https://github.com/yalap13/typst2pptx.git
cd typst2pptx
uv sync
maturin develop
uv tool install .
```

Usage
-----
Basic CLI:

```
typst2pptx path/to/doc.typ
```

Options:
- `-o/--output PATH`: override output path (defaults to `<sourcename>.pptx`).
- `--equations-dir PATH`: keep rendered equation PNGs under `PATH` instead of a temporary directory. By default, they are written to a temp `typst2pptx-equations-<timestamp>-<pid>` folder and cleaned up afterward.

Notes:
- Typst packages are fetched as needed. 
- There is a sample Typst source at `src/source.typ` with its image dependencies. You can try `typst2pptx src/source.typ -o my_presentation.pptx`.
- Skewed and non-uniformly scaled text/images are rasterized.
- Text stays editable where possible (e.g. when not skewed or scaled non-uniformly).
- Currently missing: support for gradients (anything else?).
