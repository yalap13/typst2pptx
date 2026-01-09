use anyhow::{anyhow, Context, Result};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder, RgbaImage};
use pyo3::{
    exceptions::PyRuntimeError,
    prelude::*,
    types::{PyBytes, PyList},
};
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

mod typst_wrapper_world;

use typst::foundations::Smart;
use typst::introspection::{Location, Tag};
use typst::layout::{Abs, Frame, FrameItem, PagedDocument, Point, Size};
use typst::text::FontStyle;
use typst::visualize::{CurveItem, FixedStroke, Geometry, ImageKind, Paint};
use typst_render::render as render_page;
use typst_wrapper_world::TypstWrapperWorld;

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

#[derive(Clone, Copy, Debug)]
struct BoundingBox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    initialized: bool,
}

impl BoundingBox {
    fn new() -> Self {
        Self {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
            initialized: false,
        }
    }

    fn include_bounds(&mut self, left: f64, top: f64, right: f64, bottom: f64) {
        if !self.initialized {
            self.min_x = left;
            self.min_y = top;
            self.max_x = right;
            self.max_y = bottom;
            self.initialized = true;
            return;
        }

        self.min_x = self.min_x.min(left);
        self.min_y = self.min_y.min(top);
        self.max_x = self.max_x.max(right);
        self.max_y = self.max_y.max(bottom);
    }

    fn is_valid(&self) -> bool {
        self.initialized
    }
}

#[derive(Clone, Copy, Debug)]
struct RectBounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

#[derive(Clone)]
struct EquationBuilder {
    location: Location,
    frame: Frame,
    bbox: BoundingBox,
}

impl EquationBuilder {
    fn new(location: Location, page_size: Size) -> Self {
        Self {
            location,
            frame: Frame::hard(page_size),
            bbox: BoundingBox::new(),
        }
    }

    fn add_item(&mut self, abs_offset: Offset, item: &FrameItem) {
        let point = Point::new(Abs::pt(abs_offset.x), Abs::pt(abs_offset.y));
        self.frame.push(point, item.clone());

        if let Some(bounds) = item_bounds(abs_offset, item) {
            self.bbox
                .include_bounds(bounds.left, bounds.top, bounds.right, bounds.bottom);
        }
    }
}

#[derive(Clone)]
struct EquationCapture {
    page_index: usize,
    frame: Frame,
    bbox: BoundingBox,
}

struct EquationPng {
    page_index: usize,
    left_pt: f64,
    top_pt: f64,
    width_pt: f64,
    height_pt: f64,
    path: PathBuf,
}

fn paint_to_rgba(paint: &Paint) -> Option<[u8; 4]> {
    match paint {
        Paint::Solid(color) => Some(color.to_vec4_u8()),
        Paint::Gradient(_) | Paint::Tiling(_) => None,
    }
}

fn apply_fill<'py>(
    shape: &Bound<'py, PyAny>,
    fill: &Option<Paint>,
    rgb_color: &Bound<'py, PyAny>,
) -> PyResult<()> {
    if let Some([r, g, b, _a]) = fill.as_ref().and_then(paint_to_rgba) {
        let fill = shape.getattr("fill")?;
        fill.call_method0("solid")?;
        fill.getattr("fore_color")?
            .setattr("rgb", rgb_color.call1((r, g, b))?)?;
    } else {
        shape.getattr("fill")?.call_method0("background")?;
    }
    Ok(())
}

fn apply_stroke<'py>(
    shape: &Bound<'py, PyAny>,
    stroke: Option<&FixedStroke>,
    pt: &Bound<'py, PyAny>,
    rgb_color: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let line = shape.getattr("line")?;
    if let Some(stroke) = stroke {
        if let Some([r, g, b, _a]) = paint_to_rgba(&stroke.paint) {
            line.getattr("color")?
                .setattr("rgb", rgb_color.call1((r, g, b))?)?;
            line.setattr("width", pt.call1((stroke.thickness.to_pt(),))?)?;
            return Ok(());
        }
    }

    line.getattr("fill")?.call_method0("background")?;
    line.setattr("width", pt.call1((0,))?)?;
    Ok(())
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

fn bounds_from_points(points: &[(f64, f64)]) -> Option<RectBounds> {
    if points.is_empty() {
        return None;
    }

    let mut left = f64::INFINITY;
    let mut top = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut bottom = f64::NEG_INFINITY;

    for (x, y) in points {
        left = left.min(*x);
        top = top.min(*y);
        right = right.max(*x);
        bottom = bottom.max(*y);
    }

    Some(RectBounds {
        left,
        top,
        right,
        bottom,
    })
}

fn curve_bounds(curve: &typst::visualize::Curve, abs_offset: Offset) -> Option<RectBounds> {
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut cursor = Point::zero();

    for item in &curve.0 {
        match item {
            CurveItem::Move(point) => {
                cursor = *point;
                points.push((
                    abs_offset.x + point.x.to_pt(),
                    abs_offset.y + point.y.to_pt(),
                ));
            }
            CurveItem::Line(point) => {
                points.push((
                    abs_offset.x + point.x.to_pt(),
                    abs_offset.y + point.y.to_pt(),
                ));
                cursor = *point;
            }
            CurveItem::Cubic(c1, c2, end) => {
                let approximated = sample_cubic_points(cursor, *c1, *c2, *end, 12);
                for (x, y) in approximated {
                    points.push((abs_offset.x + x, abs_offset.y + y));
                }
                cursor = *end;
            }
            CurveItem::Close => {}
        }
    }

    bounds_from_points(&points)
}

fn equation_start_location(tag: &Tag) -> Option<Location> {
    if let Tag::Start(elem) = tag {
        if elem.elem().name() == "equation" {
            return elem.location();
        }
    }
    None
}

fn equation_end_location(tag: &Tag) -> Option<Location> {
    if let Tag::End(location, _) = tag {
        return Some(*location);
    }
    None
}

fn item_bounds(abs_offset: Offset, item: &FrameItem) -> Option<RectBounds> {
    match item {
        FrameItem::Text(text) => {
            let width = text.width().to_pt();
            let metrics = text.font.metrics();
            let ascender = metrics.ascender.at(text.size).to_pt();
            let descender = metrics.descender.at(text.size).to_pt();
            let height = ascender - descender;
            Some(RectBounds {
                left: abs_offset.x,
                top: abs_offset.y - ascender,
                right: abs_offset.x + width,
                bottom: abs_offset.y - ascender + height,
            })
        }
        FrameItem::Shape(shape, _) => match &shape.geometry {
            Geometry::Rect(size) => Some(RectBounds {
                left: abs_offset.x,
                top: abs_offset.y,
                right: abs_offset.x + size.x.to_pt(),
                bottom: abs_offset.y + size.y.to_pt(),
            }),
            Geometry::Line(line) => {
                let x2 = abs_offset.x + line.x.to_pt();
                let y2 = abs_offset.y + line.y.to_pt();
                Some(RectBounds {
                    left: abs_offset.x.min(x2),
                    top: abs_offset.y.min(y2),
                    right: abs_offset.x.max(x2),
                    bottom: abs_offset.y.max(y2),
                })
            }
            Geometry::Curve(curve) => curve_bounds(curve, abs_offset),
        },
        FrameItem::Image(_, size, _) => Some(RectBounds {
            left: abs_offset.x,
            top: abs_offset.y,
            right: abs_offset.x + size.x.to_pt(),
            bottom: abs_offset.y + size.y.to_pt(),
        }),
        FrameItem::Link(_, size) => Some(RectBounds {
            left: abs_offset.x,
            top: abs_offset.y,
            right: abs_offset.x + size.x.to_pt(),
            bottom: abs_offset.y + size.y.to_pt(),
        }),
        FrameItem::Group(_) | FrameItem::Tag(_) => None,
    }
}

fn collect_equations_from_frame(
    frame: &Frame,
    parent_offset: Offset,
    page_size: Size,
    page_index: usize,
    stack: &mut Vec<EquationBuilder>,
    captures: &mut Vec<EquationCapture>,
) {
    for (pos, item) in frame.items() {
        let local_offset = Offset {
            x: pos.x.to_pt(),
            y: pos.y.to_pt(),
        };
        let abs_offset = parent_offset.add(local_offset.x, local_offset.y);

        match item {
            FrameItem::Group(group) => {
                let transform = group.transform;
                let translated_offset = abs_offset.add(transform.tx.to_pt(), transform.ty.to_pt());
                collect_equations_from_frame(
                    &group.frame,
                    translated_offset,
                    page_size,
                    page_index,
                    stack,
                    captures,
                );
            }
            FrameItem::Tag(tag) => match tag {
                _ => {
                    if let Some(loc) = equation_start_location(tag) {
                        stack.push(EquationBuilder::new(loc, page_size));
                    } else if let Some(location) = equation_end_location(tag) {
                        if let Some(idx) = stack.iter().rposition(|eq| eq.location == location) {
                            let builder = stack.remove(idx);
                            if builder.bbox.is_valid() {
                                captures.push(EquationCapture {
                                    page_index,
                                    frame: builder.frame,
                                    bbox: builder.bbox,
                                });
                            }
                        }
                    }
                }
            },
            _ => {
                if let Some(active) = stack.last_mut() {
                    active.add_item(abs_offset, item);
                }
            }
        }
    }
}

fn collect_equations(paged_doc: &PagedDocument) -> Vec<EquationCapture> {
    let mut captures = Vec::new();

    for (page_index, page) in paged_doc.pages.iter().enumerate() {
        let mut stack: Vec<EquationBuilder> = Vec::new();
        collect_equations_from_frame(
            &page.frame,
            Offset::zero(),
            page.frame.size(),
            page_index,
            &mut stack,
            &mut captures,
        );
    }

    captures
}

fn crop_image_to_content(image: RgbaImage) -> RgbaImage {
    let width = image.width();
    let height = image.height();

    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;

    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] != 0 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            found = true;
        }
    }

    if !found {
        return image;
    }

    let crop_width = max_x - min_x + 1;
    let crop_height = max_y - min_y + 1;
    let cropped =
        image::imageops::crop_imm(&image, min_x, min_y, crop_width, crop_height).to_image();

    cropped
}

fn render_equations_to_png(
    paged_doc: &PagedDocument,
    captures: &[EquationCapture],
    output_dir: &Path,
) -> Result<Vec<EquationPng>> {
    fs::create_dir_all(output_dir).context("failed to create equation output directory")?;

    let dpi: f32 = 300.0;
    let pixel_per_pt: f32 = dpi / 72.0;

    let mut rendered = Vec::new();

    for (index, capture) in captures.iter().enumerate() {
        let mut page = paged_doc.pages[capture.page_index].clone();
        page.frame = capture.frame.clone();
        page.fill = Smart::Custom(None);

        let pixmap = render_page(&page, pixel_per_pt);

        let image = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec())
            .ok_or_else(|| anyhow!("failed to build RGBA image for equation {}", index + 1))?;

        let cropped = crop_image_to_content(image);

        let file_name = format!("equation_page{}_{}.png", capture.page_index + 1, index + 1);
        let path = output_dir.join(file_name);
        let file = fs::File::create(&path)?;
        let encoder = PngEncoder::new(file);
        encoder.write_image(
            cropped.as_raw(),
            cropped.width(),
            cropped.height(),
            ColorType::Rgba8.into(),
        )?;

        let left_pt = capture.bbox.min_x;
        let top_pt = capture.bbox.min_y;
        let width_pt = cropped.width() as f64 / pixel_per_pt as f64;
        let height_pt = cropped.height() as f64 / pixel_per_pt as f64;

        rendered.push(EquationPng {
            page_index: capture.page_index,
            left_pt,
            top_pt,
            width_pt,
            height_pt,
            path,
        });
    }

    Ok(rendered)
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
    equation_stack: &mut Vec<Location>,
) -> PyResult<()> {
    let shapes = slide.getattr("shapes")?;
    let io = py.import("io")?;

    for (pos, item) in frame.items() {
        let local_offset = Offset {
            x: pos.x.to_pt(),
            y: pos.y.to_pt(),
        };

        let abs_offset = parent_offset.add(local_offset.x, local_offset.y);
        let equation_active = !equation_stack.is_empty();

        match item {
            FrameItem::Group(group) => {
                // Recurse with accumulated offset
                let transform = group.transform;
                let translated_offset = abs_offset.add(transform.tx.to_pt(), transform.ty.to_pt());
                walk_frame(
                    &group.frame,
                    translated_offset,
                    slide,
                    py,
                    pt,
                    rgb_color,
                    mso_auto_shape,
                    mso_connector,
                    equation_stack,
                )?;
            }
            FrameItem::Tag(tag) => {
                if let Some(loc) = equation_start_location(tag) {
                    equation_stack.push(loc);
                } else if let Some(location) = equation_end_location(tag) {
                    if equation_stack.last() == Some(&location) {
                        equation_stack.pop();
                    }
                }
            }
            _ if equation_active => continue,

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
                    if !equation_stack.is_empty() {
                        continue;
                    }

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

                    apply_fill(&rect, &shape.fill, rgb_color)?;
                    apply_stroke(&rect, shape.stroke.as_ref(), pt, rgb_color)?;
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

                    apply_stroke(&line_shape, shape.stroke.as_ref(), pt, rgb_color)?;
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
                                let approximated = sample_cubic_points(cursor, *c1, *c2, *end, 12);

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

                    apply_fill(&pptx_shape, &shape.fill, rgb_color)?;
                    apply_stroke(&pptx_shape, shape.stroke.as_ref(), pt, rgb_color)?;
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
        }
    }

    Ok(())
}

fn walk_paged_document(paged_doc: PagedDocument, equations: &[EquationPng]) -> PyResult<()> {
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

        let mut equations_by_page: HashMap<usize, Vec<&EquationPng>> = HashMap::new();
        for equation in equations {
            equations_by_page
                .entry(equation.page_index)
                .or_default()
                .push(equation);
        }

        // Set slide size from Typst page dimensions
        let first_page = &paged_doc.pages[0];
        let width_pt = first_page.frame.width().to_pt();
        let height_pt = first_page.frame.height().to_pt();

        presentation.setattr("slide_width", pt.call1((width_pt,))?)?;
        presentation.setattr("slide_height", pt.call1((height_pt,))?)?;

        // Create slides and render content
        for (page_index, page) in paged_doc.pages.iter().enumerate() {
            let slide = slides.call_method1("add_slide", (blank_layout.clone(),))?;
            let mut equation_stack: Vec<Location> = Vec::new();
            walk_frame(
                &page.frame,
                Offset::zero(),
                &slide,
                py,
                &pt,
                &rgb_color,
                &mso_auto_shape,
                &mso_connector,
                &mut equation_stack,
            )?;

            if let Some(page_equations) = equations_by_page.get(&page_index) {
                let shapes = slide.getattr("shapes")?;
                for eq in page_equations {
                    let left = pt.call1((eq.left_pt,))?;
                    let top = pt.call1((eq.top_pt,))?;
                    let width = pt.call1((eq.width_pt,))?;
                    let height = pt.call1((eq.height_pt,))?;
                    shapes.call_method1(
                        "add_picture",
                        (eq.path.to_string_lossy().as_ref(), left, top, width, height),
                    )?;
                }
            }
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

    let equation_captures = collect_equations(&document);
    let equation_images = render_equations_to_png(&document, &equation_captures, Path::new("equations"))
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;

    walk_paged_document(document, &equation_images)
}
