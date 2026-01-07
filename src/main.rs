use pyo3::{prelude::*, types::{PyBytes, PyList}};
use std::{env, fs};

use typst::layout::{Frame, FrameItem, PagedDocument, Point};
use typst::text::FontStyle;
use typst::visualize::{CurveItem, Geometry, ImageKind, Paint};
use typst_as_library::TypstWrapperWorld;

/// Simple translation-only transform accumulator (should become full rotation/scale later?)
#[derive(Clone, Copy, Debug)]
struct Offset {
    x: f64,
    y: f64,
}

impl Offset {
    fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    fn add(self, dx: f64, dy: f64) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
        }
    }
}

fn paint_to_rgba(paint: &Paint) -> Option<[u8; 4]> {
    match paint {
        Paint::Solid(color) => Some(color.to_rgb().to_vec4_u8()),
        Paint::Gradient(_) | Paint::Tiling(_) => None,
    }
}

fn disable_shadow(shape: &Bound<'_, PyAny>) -> PyResult<()> {
    shape.getattr("shadow")?.setattr("inherit", false)?;
    Ok(())
}

fn add_line_segments<'py>(
    py: Python<'py>,
    pt: &Bound<'py, PyAny>,
    builder: &Bound<'py, PyAny>,
    vertices: &[(f64, f64)],
    close: bool,
) -> PyResult<()> {
    if vertices.is_empty() && !close {
        return Ok(());
    }

    let py_vertices = PyList::empty(py);
    for (x, y) in vertices {
        py_vertices.append((pt.call1((*x,))?, pt.call1((*y,))?))?;
    }

    builder.call_method1("add_line_segments", (py_vertices, close))?;
    Ok(())
}

fn sample_cubic_points(
    start: Point,
    c1: Point,
    c2: Point,
    end: Point,
    steps: usize,
) -> Vec<(f64, f64)> {
    let mut points = Vec::with_capacity(steps);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let omt = 1.0 - t;
        let omt2 = omt * omt;
        let t2 = t * t;

        let x = omt2 * omt * start.x.to_pt()
            + 3.0 * omt2 * t * c1.x.to_pt()
            + 3.0 * omt * t2 * c2.x.to_pt()
            + t2 * t * end.x.to_pt();
        let y = omt2 * omt * start.y.to_pt()
            + 3.0 * omt2 * t * c1.y.to_pt()
            + 3.0 * omt * t2 * c2.y.to_pt()
            + t2 * t * end.y.to_pt();

        points.push((x, y));
    }

    points
}

/// Walk a Typst frame tree and place items with absolute coordinates
fn walk_frame<'py>(
    frame: &Frame,
    parent_offset: Offset,
    slide: &Bound<'py, PyAny>,
    py: Python<'py>,
    pt: &Bound<'py, PyAny>,
    rgb_color: &Bound<'py, PyAny>,
    mso_auto_shape: &Bound<'py, PyAny>,
    mso_connector: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let shapes = slide.getattr("shapes")?;
    let io = py.import("io")?;

    for (pos, item) in frame.items() {
        let local_offset = Offset {
            x: pos.x.to_pt(),
            y: pos.y.to_pt(),
        };

        let abs_offset = parent_offset.add(local_offset.x, local_offset.y);

        match item {
            FrameItem::Group(group) => {
                // Recurse with accumulated offset
                let transform = group.transform;
                let translated_offset = abs_offset.add(
                    transform.tx.to_pt(),
                    transform.ty.to_pt(),
                );
                walk_frame(
                    &group.frame,
                    translated_offset,
                    slide,
                    py,
                    pt,
                    rgb_color,
                    mso_auto_shape,
                    mso_connector,
                )?;
            }

            FrameItem::Text(text) => {
                let content = text.text.to_string();
                if content.trim().is_empty() {
                    continue;
                }

                let width = pt.call1((text.width().to_pt(),))?;
                let metrics = text.font.metrics();
                let ascender = metrics.ascender.at(text.size).to_pt();
                let descender = metrics.descender.at(text.size).to_pt();
                let height = pt.call1((ascender - descender,))?;
                let left = pt.call1((abs_offset.x,))?;
                let top = pt.call1((abs_offset.y - ascender,))?;
                let textbox = shapes.call_method1("add_textbox", (left, top, width, height))?;
                disable_shadow(&textbox)?;

                let text_frame = textbox.getattr("text_frame")?;
                text_frame.setattr("margin_left", pt.call1((0,))?)?;
                text_frame.setattr("margin_top", pt.call1((0,))?)?;
                text_frame.setattr("margin_right", pt.call1((0,))?)?;
                text_frame.setattr("margin_bottom", pt.call1((0,))?)?;
                text_frame.call_method0("clear")?;

                let p = text_frame.getattr("paragraphs")?.get_item(0)?;
                let run = p.call_method0("add_run")?;
                run.setattr("text", content)?;

                let font = run.getattr("font")?;
                let font_info = text.font.info();
                font.setattr("name", font_info.family.as_str())?;
                font.setattr("size", pt.call1((text.size.to_pt(),))?)?;

                if let Some([r, g, b, _a]) = paint_to_rgba(&text.fill) {
                    font.getattr("color")?
                        .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                }

                let variant = &font_info.variant;
                font.setattr("bold", variant.weight.to_number() >= 700)?;
                font.setattr(
                    "italic",
                    matches!(variant.style, FontStyle::Italic | FontStyle::Oblique),
                )?;
            }

            FrameItem::Shape(shape, _span) => match &shape.geometry {
                Geometry::Rect(size) => {
                    let width = pt.call1((size.x.to_pt(),))?;
                    let height = pt.call1((size.y.to_pt(),))?;
                    let left = pt.call1((abs_offset.x,))?;
                    let top = pt.call1((abs_offset.y,))?;

                    let rect = shapes.call_method1(
                        "add_shape",
                        (
                            mso_auto_shape.getattr("RECTANGLE")?,
                            left,
                            top,
                            width,
                            height,
                        ),
                    )?;
                    disable_shadow(&rect)?;

                    if let Some(fill_paint) = &shape.fill {
                        if let Some([r, g, b, _a]) = paint_to_rgba(fill_paint) {
                            let fill = rect.getattr("fill")?;
                            fill.call_method0("solid")?;
                            fill.getattr("fore_color")?
                                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                        }
                    } else {
                        rect.getattr("fill")?.call_method0("background")?;
                    }

                    if let Some(stroke) = &shape.stroke {
                        if let Some([r, g, b, _a]) = paint_to_rgba(&stroke.paint) {
                            let line = rect.getattr("line")?;
                            line.getattr("color")?
                                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                            line.setattr("width", pt.call1((stroke.thickness.to_pt(),))?)?;
                        }
                    } else {
                        let line = rect.getattr("line")?;
                        line.getattr("fill")?.call_method0("background")?;
                        line.setattr("width", pt.call1((0,))?)?;
                    }
                }
                Geometry::Line(line) => {
                    let begin_x = pt.call1((abs_offset.x,))?;
                    let begin_y = pt.call1((abs_offset.y,))?;
                    let end_x = pt.call1((abs_offset.x + line.x.to_pt(),))?;
                    let end_y = pt.call1((abs_offset.y + line.y.to_pt(),))?;

                    let line_shape = shapes.call_method1(
                        "add_connector",
                        (
                            mso_connector.getattr("STRAIGHT")?,
                            begin_x,
                            begin_y,
                            end_x,
                            end_y,
                        ),
                    )?;
                    disable_shadow(&line_shape)?;

                    if let Some(stroke) = &shape.stroke {
                        if let Some([r, g, b, _a]) = paint_to_rgba(&stroke.paint) {
                            let line = line_shape.getattr("line")?;
                            line.getattr("color")?
                                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                            line.setattr("width", pt.call1((stroke.thickness.to_pt(),))?)?;
                        }
                    } else {
                        let line = line_shape.getattr("line")?;
                        line.getattr("fill")?.call_method0("background")?;
                        line.setattr("width", pt.call1((0,))?)?;
                    }
                }
                Geometry::Curve(curve) => {
                    if curve.0.is_empty() {
                        continue;
                    }

                    let initial_cursor = match curve.0.first() {
                        Some(CurveItem::Move(p)) => *p,
                        _ => Point::zero(),
                    };

                    let start_x = pt.call1((abs_offset.x + initial_cursor.x.to_pt(),))?;
                    let start_y = pt.call1((abs_offset.y + initial_cursor.y.to_pt(),))?;
                    let builder = shapes.call_method1("build_freeform", (start_x, start_y))?;
                    let mut cursor = initial_cursor;
                    let mut pending: Vec<(f64, f64)> = Vec::new();

                    for item in &curve.0 {
                        match item {
                            CurveItem::Move(point) => {
                                add_line_segments(py, &pt, &builder, &pending, false)?;
                                pending.clear();

                                builder.call_method1(
                                    "move_to",
                                    (
                                        pt.call1((abs_offset.x + point.x.to_pt(),))?,
                                        pt.call1((abs_offset.y + point.y.to_pt(),))?,
                                    ),
                                )?;
                                cursor = *point;
                            }
                            CurveItem::Line(point) => {
                                pending.push((
                                    abs_offset.x + point.x.to_pt(),
                                    abs_offset.y + point.y.to_pt(),
                                ));
                                cursor = *point;
                            }
                            CurveItem::Cubic(c1, c2, end) => {
                                let approximated =
                                    sample_cubic_points(cursor, *c1, *c2, *end, 12);

                                for (x, y) in approximated {
                                    pending.push((abs_offset.x + x, abs_offset.y + y));
                                }
                                cursor = *end;
                            }
                            CurveItem::Close => {
                                add_line_segments(py, &pt, &builder, &pending, true)?;
                                pending.clear();
                            }
                        }
                    }

                    add_line_segments(py, &pt, &builder, &pending, false)?;

                    let pptx_shape = builder.call_method0("convert_to_shape")?;
                    disable_shadow(&pptx_shape)?;

                    if let Some(fill_paint) = &shape.fill {
                        if let Some([r, g, b, _a]) = paint_to_rgba(fill_paint) {
                            let fill = pptx_shape.getattr("fill")?;
                            fill.call_method0("solid")?;
                            fill.getattr("fore_color")?
                                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                        }
                    } else {
                        pptx_shape.getattr("fill")?.call_method0("background")?;
                    }

                    if let Some(stroke) = &shape.stroke {
                        if let Some([r, g, b, _a]) = paint_to_rgba(&stroke.paint) {
                            let line = pptx_shape.getattr("line")?;
                            line.getattr("color")?
                                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
                            line.setattr("width", pt.call1((stroke.thickness.to_pt(),))?)?;
                        }
                    } else {
                        let line = pptx_shape.getattr("line")?;
                        line.getattr("fill")?.call_method0("background")?;
                        line.setattr("width", pt.call1((0,))?)?;
                    }
                }
            },
            FrameItem::Image(image, size, _span) => match image.kind() {
                ImageKind::Raster(raster) => {
                    let raw = raster.data();
                    let image_bytes = PyBytes::new(py, raw.as_slice());
                    let buffer = io.getattr("BytesIO")?.call1((image_bytes,))?;

                    let width = pt.call1((size.x.to_pt(),))?;
                    let height = pt.call1((size.y.to_pt(),))?;
                    let left = pt.call1((abs_offset.x,))?;
                    let top = pt.call1((abs_offset.y,))?;

                    shapes.call_method1("add_picture", (buffer, left, top, width, height))?;
                }
                ImageKind::Svg(_) => {
                    eprintln!("SVG images are not yet supported in PPTX export; skipping.");
                }
            },
            FrameItem::Link(..) => {}
            FrameItem::Tag(..) => {}
        }
    }

    Ok(())
}

fn walk_paged_document(paged_doc: PagedDocument) -> PyResult<()> {
    Python::attach(|py| {
        let pptx = py.import("pptx")?;
        let util = py.import("pptx.util")?;
        let pt = util.getattr("Pt")?;
        let color_mod = py.import("pptx.dml.color")?;
        let rgb_color = color_mod.getattr("RGBColor")?;
        let shapes_enum = py.import("pptx.enum.shapes")?;
        let mso_auto_shape = shapes_enum.getattr("MSO_AUTO_SHAPE_TYPE")?;
        let mso_connector = shapes_enum.getattr("MSO_CONNECTOR_TYPE")?;

        let presentation = pptx.getattr("Presentation")?.call0()?;
        let slides = presentation.getattr("slides")?;
        let layouts = presentation.getattr("slide_layouts")?;
        let blank_layout = layouts.get_item(6)?;

        // Set slide size from Typst page dimensions
        let first_page = &paged_doc.pages[0];
        let width_pt = first_page.frame.width().to_pt();
        let height_pt = first_page.frame.height().to_pt();

        presentation.setattr("slide_width", pt.call1((width_pt,))?)?;
        presentation.setattr("slide_height", pt.call1((height_pt,))?)?;

        // Create slides and render content
        for page in &paged_doc.pages {
            // println!("{:?}", page.frame);
            let slide = slides.call_method1("add_slide", (blank_layout.clone(),))?;
            walk_frame(
                &page.frame,
                Offset::zero(),
                &slide,
                py,
                &pt,
                &rgb_color,
                &mso_auto_shape,
                &mso_connector,
            )?;
        }

        presentation.call_method1("save", ("my_presentation.pptx",))?;
        Ok(())
    })
}

fn main() -> PyResult<()> {
    // Temporary explicit Python environment setup
    unsafe {
        env::set_var(
            "PYTHONHOME",
            "/Users/coug8874/.local/share/uv/python/cpython-3.13.7-macos-aarch64-none",
        );
        env::set_var(
            "PYTHONPATH",
            "/Users/coug8874/code/pyo3-test/.venv/lib/python3.13/site-packages",
        );
    }

    let content = fs::read_to_string("src/source.typ").expect("Unable to read Typst source file.");

    let world = TypstWrapperWorld::new("src/".to_owned(), content);

    let document: PagedDocument = typst::compile(&world)
        .output
        .expect("Typst compilation failed");

    walk_paged_document(document)
}
