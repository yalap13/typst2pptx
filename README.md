# typst2pptx

Convert Typst documents into PowerPoint (`.pptx`) files. It walks the Typst layout tree and recreates text, shapes, images, and equations in PowerPoint using `python-pptx`, rasterizing only when PowerPoint cannot represent a transform faithfully.

## Installation and Usage
### With `uv` (recommended) :
```
uv tool install typst2pptx
```
Can also be installed using `pip` or `pipx`.

### From source :
#### Requirements :
- Python 3.12+
- Rust toolchain (for the PyO3 extension)
- `uv` for the suggested installation method

Clone the repo, then install the tool using `maturin` and `uv` (this installs the tool globally):

```
git clone https://github.com/yalap13/typst2pptx.git
cd typst2pptx
uv sync
maturin develop
uv tool install .
```

## Usage
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
- Linear gradients work, but radial/conic gradients are not currently supported.

## Supported features

“Editable” means native PowerPoint text or shapes. “Rasterized” means the content is preserved as a PNG image.

| Feature | Support | Output and limitations |
| --- | --- | --- |
| Typst pages and slide dimensions | Supported | Each page becomes a slide; all slides use the first page's dimensions. |
| Text | Editable | Preserves font family, size, bold, italic, solid color, and placement. Fonts must be available in the application viewing the presentation. |
| Inline raw code | Editable | Preserves the monospace font and corrects baseline placement using font metrics. |
| Subscripts and superscripts | Supported | Synthesized scripts stay editable; special script glyphs supplied by the font are rasterized. |
| Bulleted and numbered lists | Supported | Markers and text are positioned individually; they are not native PowerPoint lists. |
| Inline and block equations | Rasterized | Rendered at 300 dpi and placed at Typst's layout coordinates. Equation images can be retained with `--equations-dir`. |
| Rectangles, lines, arrows, and paths | Editable | Exported as PowerPoint shapes and connectors; cubic curves are approximated with line segments. |
| Solid fills and page backgrounds | Supported | Preserves RGB color and transparency, including `color.transparentize()`. |
| Linear gradient fills | Supported | Preserves stop colors, transparency, positions, and angle. |
| Radial and conic gradient fills | Partial | Falls back to the first stop's color and transparency. |
| Tiling fills | Unsupported | No fill is exported. |
| Shape outlines | Partial | Preserves solid color, transparency, and thickness; dash patterns, caps, joins, and gradient or tiling strokes are not preserved. |
| Raster images | Supported | Embedded as pictures; transforms that require rasterization are rendered at 300 dpi. |
| SVG and PDF images | Rasterized | Rendered at 300 dpi and embedded as pictures. |
| Rotation and scaling | Supported | Represented natively where possible; skewed or non-uniformly scaled text and skewed images are rasterized. |
| Web hyperlinks | Supported | Clickable shapes overlay the link area. |
| Internal document links | Supported | Resolved destinations link to the corresponding slide. |
| Shape shadows | Disabled | Exported text, geometry, and pictures use flat styling without inherited theme shadows. |
| Layout containers, grids, and tables | Partial | Preserves their laid-out text and geometry; does not recreate native PowerPoint containers, groups, or tables. |
| Group clipping | Unsupported | Group clipping paths are not applied during conversion. |
| Typst packages and presentation themes | Supported | Compiled through Typst; packages are fetched as needed. Theme content follows the feature limitations above. |
| Touying pauses and overlays | Static slides | Each compiled Typst page becomes a separate slide; no PowerPoint animations are generated. |
