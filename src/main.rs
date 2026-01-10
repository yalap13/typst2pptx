use anyhow::{Context, Result, anyhow};
use image::{ColorType, ImageEncoder, RgbaImage, codecs::png::PngEncoder};
use resvg::tiny_skia;
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
use typst::layout::{Abs, Frame, FrameItem, GroupItem, PagedDocument, Point, Size, Transform};
use typst::text::FontStyle;
use typst::visualize::{CurveItem, FixedStroke, Geometry, ImageKind, Paint};
use typst_render::render as render_page;
use typst_wrapper_world::TypstWrapperWorld;

/// Simple 2D affine transform represented as a 2x3 matrix.
#[derive(Clone, Copy, Debug)]
struct Affine {
    m11: f64,
    m12: f64,
    m21: f64,
    m22: f64,
    tx: f64,
    ty: f64,
}

impl Affine {
    fn identity() -> Self {
        Self {
            m11: 1.0,
            m12: 0.0,
            m21: 0.0,
            m22: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    fn from_typst(transform: typst::layout::Transform) -> Self {
        Self {
            m11: transform.sx.get(),
            m12: transform.kx.get(),
            m21: transform.ky.get(),
            m22: transform.sy.get(),
            tx: transform.tx.to_pt(),
            ty: transform.ty.to_pt(),
        }
    }

    fn translate(dx: f64, dy: f64) -> Self {
        Self {
            tx: dx,
            ty: dy,
            ..Self::identity()
        }
    }

    /// Compose this transform with `other` (apply `other` after `self`).
    fn mul(self, other: Self) -> Self {
        Self {
            m11: self.m11 * other.m11 + self.m12 * other.m21,
            m12: self.m11 * other.m12 + self.m12 * other.m22,
            m21: self.m21 * other.m11 + self.m22 * other.m21,
            m22: self.m21 * other.m12 + self.m22 * other.m22,
            tx: self.m11 * other.tx + self.m12 * other.ty + self.tx,
            ty: self.m21 * other.tx + self.m22 * other.ty + self.ty,
        }
    }

    fn apply_point(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.m11 * x + self.m12 * y + self.tx,
            self.m21 * x + self.m22 * y + self.ty,
        )
    }

    fn without_translation(self) -> Self {
        Self { tx: 0.0, ty: 0.0, ..self }
    }

    fn is_identity(&self) -> bool {
        (self.m11 - 1.0).abs() < 1e-9
            && self.m12.abs() < 1e-9
            && self.m21.abs() < 1e-9
            && (self.m22 - 1.0).abs() < 1e-9
            && self.tx.abs() < 1e-9
            && self.ty.abs() < 1e-9
    }

    /// Try to decompose into rotation (radians) and scales when there is no shear.
    fn decompose_rotation_scale(self) -> Option<(f64, f64, f64)> {
        // Column vectors of the linear part.
        let sx_vec = (self.m11, self.m21);
        let sy_vec = (self.m12, self.m22);

        // If the axes aren't orthogonal, treat as sheared.
        let dot = sx_vec.0 * sy_vec.0 + sx_vec.1 * sy_vec.1;
        if dot.abs() > 1e-6 {
            return None;
        }

        let scale_x = (sx_vec.0 * sx_vec.0 + sx_vec.1 * sx_vec.1).sqrt();
        let scale_y = (sy_vec.0 * sy_vec.0 + sy_vec.1 * sy_vec.1).sqrt();
        if scale_x.abs() < f64::EPSILON || scale_y.abs() < f64::EPSILON {
            return None;
        }

        let rotation = sx_vec.1.atan2(sx_vec.0);
        Some((rotation, scale_x, scale_y))
    }
}

fn affine_to_typst_transform(affine: Affine) -> Transform {
    Transform {
        sx: typst::layout::Ratio::new(affine.m11),
        kx: typst::layout::Ratio::new(affine.m12),
        ky: typst::layout::Ratio::new(affine.m21),
        sy: typst::layout::Ratio::new(affine.m22),
        tx: Abs::pt(affine.tx),
        ty: Abs::pt(affine.ty),
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

    fn add_item(&mut self, transform: Affine, item: &FrameItem) {
        let (x, y) = transform.apply_point(0.0, 0.0);
        let point = Point::new(Abs::pt(x), Abs::pt(y));
        let linear = transform.without_translation();

        let item_with_transform = if linear.is_identity() {
            item.clone()
        } else {
            let mut subframe = Frame::hard(Size::zero());
            subframe.push(Point::zero(), item.clone());
            let mut group = GroupItem::new(subframe);
            group.transform = affine_to_typst_transform(linear);
            FrameItem::Group(group)
        };

        self.frame.push(point, item_with_transform);

        if let Some(bounds) = item_bounds(transform, item) {
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

fn curve_bounds(curve: &typst::visualize::Curve, transform: Affine) -> Option<RectBounds> {
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut cursor = Point::zero();

    for item in &curve.0 {
        match item {
            CurveItem::Move(point) => {
                cursor = *point;
                points.push(transform.apply_point(point.x.to_pt(), point.y.to_pt()));
            }
            CurveItem::Line(point) => {
                points.push(transform.apply_point(point.x.to_pt(), point.y.to_pt()));
                cursor = *point;
            }
            CurveItem::Cubic(c1, c2, end) => {
                let approximated = sample_cubic_points(cursor, *c1, *c2, *end, 12);
                for (x, y) in approximated {
                    points.push(transform.apply_point(x, y));
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

fn item_bounds(transform: Affine, item: &FrameItem) -> Option<RectBounds> {
    match item {
        FrameItem::Text(text) => {
            let width = text.width().to_pt();
            let metrics = text.font.metrics();
            let ascender = metrics.ascender.at(text.size).to_pt();
            let descender = metrics.descender.at(text.size).to_pt();
            let height = ascender - descender;

            let corners = [
                (0.0, -ascender),
                (width, -ascender),
                (width, -ascender + height),
                (0.0, -ascender + height),
            ];
            let mut left = f64::INFINITY;
            let mut top = f64::INFINITY;
            let mut right = f64::NEG_INFINITY;
            let mut bottom = f64::NEG_INFINITY;
            for (x, y) in corners.iter().copied() {
                let (tx, ty) = transform.apply_point(x, y);
                left = left.min(tx);
                top = top.min(ty);
                right = right.max(tx);
                bottom = bottom.max(ty);
            }

            Some(RectBounds {
                left,
                top,
                right,
                bottom,
            })
        }
        FrameItem::Shape(shape, _) => match &shape.geometry {
            Geometry::Rect(size) => {
                let corners = [
                    transform.apply_point(0.0, 0.0),
                    transform.apply_point(size.x.to_pt(), 0.0),
                    transform.apply_point(size.x.to_pt(), size.y.to_pt()),
                    transform.apply_point(0.0, size.y.to_pt()),
                ];
                let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                    (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                    |(lx, ly, rx, by), (x, y)| {
                        (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                    },
                );
                Some(RectBounds {
                    left: min_x,
                    top: min_y,
                    right: max_x,
                    bottom: max_y,
                })
            }
            Geometry::Line(line) => {
                let (x1, y1) = transform.apply_point(0.0, 0.0);
                let (x2, y2) = transform.apply_point(line.x.to_pt(), line.y.to_pt());
                Some(RectBounds {
                    left: x1.min(x2),
                    top: y1.min(y2),
                    right: x1.max(x2),
                    bottom: y1.max(y2),
                })
            }
            Geometry::Curve(curve) => curve_bounds(curve, transform),
        },
        FrameItem::Image(_, size, _) => {
            let corners = [
                transform.apply_point(0.0, 0.0),
                transform.apply_point(size.x.to_pt(), 0.0),
                transform.apply_point(size.x.to_pt(), size.y.to_pt()),
                transform.apply_point(0.0, size.y.to_pt()),
            ];
            let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                |(lx, ly, rx, by), (x, y)| {
                    (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                },
            );
            Some(RectBounds {
                left: min_x,
                top: min_y,
                right: max_x,
                bottom: max_y,
            })
        }
        FrameItem::Link(_, size) => {
            let corners = [
                transform.apply_point(0.0, 0.0),
                transform.apply_point(size.x.to_pt(), 0.0),
                transform.apply_point(size.x.to_pt(), size.y.to_pt()),
                transform.apply_point(0.0, size.y.to_pt()),
            ];
            let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                |(lx, ly, rx, by), (x, y)| {
                    (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                },
            );
            Some(RectBounds {
                left: min_x,
                top: min_y,
                right: max_x,
                bottom: max_y,
            })
        }
        FrameItem::Group(_) | FrameItem::Tag(_) => None,
    }
}

fn collect_equations_from_frame(
    frame: &Frame,
    parent_transform: Affine,
    page_size: Size,
    page_index: usize,
    stack: &mut Vec<EquationBuilder>,
    captures: &mut Vec<EquationCapture>,
) {
    for (pos, item) in frame.items() {
        let translation = Affine::translate(pos.x.to_pt(), pos.y.to_pt());
        let item_transform = parent_transform.mul(translation);

        match item {
            FrameItem::Group(group) => {
                let transform = parent_transform
                    .mul(translation)
                    .mul(Affine::from_typst(group.transform));
                collect_equations_from_frame(
                    &group.frame,
                    transform,
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
                    active.add_item(item_transform, item);
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
            Affine::identity(),
            page.frame.size(),
            page_index,
            &mut stack,
            &mut captures,
        );
    }

    captures
}

fn crop_image_to_content(image: RgbaImage) -> (RgbaImage, u32, u32) {
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
        return (image, 0, 0);
    }

    let crop_width = max_x - min_x + 1;
    let crop_height = max_y - min_y + 1;
    let cropped =
        image::imageops::crop_imm(&image, min_x, min_y, crop_width, crop_height).to_image();

    (cropped, min_x, min_y)
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

        let (cropped, crop_min_x_px, crop_min_y_px) = crop_image_to_content(image);

        let pixel_per_pt_f64 = pixel_per_pt as f64;
        let bbox_min_x_px = (capture.bbox.min_x * pixel_per_pt_f64).floor() as i64;
        let bbox_min_y_px = (capture.bbox.min_y * pixel_per_pt_f64).floor() as i64;
        let bbox_max_x_px = (capture.bbox.max_x * pixel_per_pt_f64).ceil() as i64;
        let bbox_max_y_px = (capture.bbox.max_y * pixel_per_pt_f64).ceil() as i64;

        let bbox_width_px = (bbox_max_x_px - bbox_min_x_px).max(1) as u32;
        let bbox_height_px = (bbox_max_y_px - bbox_min_y_px).max(1) as u32;

        let mut padded = RgbaImage::new(bbox_width_px, bbox_height_px);
        let paste_x = (crop_min_x_px as i64 - bbox_min_x_px).max(0) as u32;
        let paste_y = (crop_min_y_px as i64 - bbox_min_y_px).max(0) as u32;
        image::imageops::overlay(&mut padded, &cropped, paste_x.into(), paste_y.into());

        let file_name = format!("equation_page{}_{}.png", capture.page_index + 1, index + 1);
        let path = output_dir.join(file_name);
        let file = fs::File::create(&path)?;
        let encoder = PngEncoder::new(file);
        encoder.write_image(
            padded.as_raw(),
            padded.width(),
            padded.height(),
            ColorType::Rgba8.into(),
        )?;

        let left_pt = capture.bbox.min_x;
        let top_pt = capture.bbox.min_y;
        let width_pt = padded.width() as f64 / pixel_per_pt_f64;
        let height_pt = padded.height() as f64 / pixel_per_pt_f64;

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
    parent_transform: Affine,
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
        let translation = Affine::translate(pos.x.to_pt(), pos.y.to_pt());
        let item_transform = parent_transform.mul(translation);
        let equation_active = !equation_stack.is_empty();

        match item {
            FrameItem::Group(group) => {
                // Recurse with accumulated transform
                let transform = parent_transform
                    .mul(translation)
                    .mul(Affine::from_typst(group.transform));
                walk_frame(
                    &group.frame,
                    transform,
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

                let raw_width = text.width().to_pt();
                let metrics = text.font.metrics();
                let ascender = metrics.ascender.at(text.size).to_pt();
                let descender = metrics.descender.at(text.size).to_pt();
                let raw_height = ascender - descender;

                // Try to map rotation/scale; otherwise fall back to axis-aligned bounds.
                let (left_pt, top_pt, width_pt, height_pt, rotation_deg, scale_x, scale_y) =
                    if let Some((rotation, scale_x, scale_y)) =
                        item_transform.decompose_rotation_scale()
                    {
                        let center_local = (raw_width / 2.0, -ascender + raw_height / 2.0);
                        let (center_x, center_y) =
                            item_transform.apply_point(center_local.0, center_local.1);
                        (
                            center_x - (raw_width * scale_x) / 2.0,
                            center_y - (raw_height * scale_y) / 2.0,
                            raw_width * scale_x,
                            raw_height * scale_y,
                            Some(rotation.to_degrees()),
                            scale_x,
                            scale_y,
                        )
                    } else {
                        let corners = [
                            item_transform.apply_point(0.0, -ascender),
                            item_transform.apply_point(raw_width, -ascender),
                            item_transform.apply_point(raw_width, -ascender + raw_height),
                            item_transform.apply_point(0.0, -ascender + raw_height),
                        ];
                        let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                            (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                            |(lx, ly, rx, by), (x, y)| {
                                (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                            },
                        );
                        (min_x, min_y, max_x - min_x, max_y - min_y, None, 1.0, 1.0)
                    };

                let left = pt.call1((left_pt,))?;
                let top = pt.call1((top_pt,))?;
                let width = pt.call1((width_pt,))?;
                let height = pt.call1((height_pt,))?;
                let textbox = shapes.call_method1("add_textbox", (left, top, width, height))?;
                if let Some(rotation_deg) = rotation_deg {
                    textbox.setattr("rotation", rotation_deg)?;
                }
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
                // Use uniform scale to approximate text scaling; python-pptx cannot apply non-uniform text scale.
                let avg_scale = (scale_x.abs() + scale_y.abs()) / 2.0;
                font.setattr("size", pt.call1((text.size.to_pt() * avg_scale,))?)?;

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

                    let corners = [
                        item_transform.apply_point(0.0, 0.0),
                        item_transform.apply_point(size.x.to_pt(), 0.0),
                        item_transform.apply_point(size.x.to_pt(), size.y.to_pt()),
                        item_transform.apply_point(0.0, size.y.to_pt()),
                    ];
                    let start_x = pt.call1((corners[0].0,))?;
                    let start_y = pt.call1((corners[0].1,))?;
                    let builder = shapes.call_method1("build_freeform", (start_x, start_y))?;
                    let mut pending: Vec<(f64, f64)> = Vec::new();
                    for corner in corners.iter().skip(1) {
                        pending.push(*corner);
                    }
                    add_line_segments(py, &pt, &builder, &pending, true)?;
                    let rect_shape = builder.call_method0("convert_to_shape")?;
                    disable_shadow(&rect_shape)?;

                    apply_fill(&rect_shape, &shape.fill, rgb_color)?;
                    apply_stroke(&rect_shape, shape.stroke.as_ref(), pt, rgb_color)?;
                }
                Geometry::Line(line) => {
                    let (x1, y1) = item_transform.apply_point(0.0, 0.0);
                    let (x2, y2) = item_transform.apply_point(line.x.to_pt(), line.y.to_pt());
                    let begin_x = pt.call1((x1,))?;
                    let begin_y = pt.call1((y1,))?;
                    let end_x = pt.call1((x2,))?;
                    let end_y = pt.call1((y2,))?;

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

                    let start_global =
                        item_transform.apply_point(initial_cursor.x.to_pt(), initial_cursor.y.to_pt());
                    let start_x = pt.call1((start_global.0,))?;
                    let start_y = pt.call1((start_global.1,))?;
                    let builder = shapes.call_method1("build_freeform", (start_x, start_y))?;
                    let mut cursor = initial_cursor;
                    let mut pending: Vec<(f64, f64)> = Vec::new();

                    for item in &curve.0 {
                        match item {
                            CurveItem::Move(point) => {
                                add_line_segments(py, &pt, &builder, &pending, false)?;
                                pending.clear();

                                let move_global =
                                    item_transform.apply_point(point.x.to_pt(), point.y.to_pt());
                                builder.call_method1(
                                    "move_to",
                                    (
                                        pt.call1((move_global.0,))?,
                                        pt.call1((move_global.1,))?,
                                    ),
                                )?;
                                cursor = *point;
                            }
                            CurveItem::Line(point) => {
                                pending.push((
                                    item_transform.apply_point(point.x.to_pt(), point.y.to_pt()).0,
                                    item_transform.apply_point(point.x.to_pt(), point.y.to_pt()).1,
                                ));
                                cursor = *point;
                            }
                            CurveItem::Cubic(c1, c2, end) => {
                                let approximated = sample_cubic_points(cursor, *c1, *c2, *end, 12);

                                for (x, y) in approximated {
                                    let (tx, ty) = item_transform.apply_point(x, y);
                                    pending.push((tx, ty));
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

                    let raw_width = size.x.to_pt();
                    let raw_height = size.y.to_pt();
                    let (left_pt, top_pt, width_pt, height_pt, rotation_deg) =
                        if let Some((rotation, scale_x, scale_y)) =
                            item_transform.decompose_rotation_scale()
                        {
                            let (center_x, center_y) = item_transform
                                .apply_point(raw_width / 2.0, raw_height / 2.0);
                            (
                                center_x - (raw_width * scale_x) / 2.0,
                                center_y - (raw_height * scale_y) / 2.0,
                                raw_width * scale_x,
                                raw_height * scale_y,
                                Some(rotation.to_degrees()),
                            )
                        } else {
                            let corners = [
                                item_transform.apply_point(0.0, 0.0),
                                item_transform.apply_point(raw_width, 0.0),
                                item_transform.apply_point(raw_width, raw_height),
                                item_transform.apply_point(0.0, raw_height),
                            ];
                            let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                                (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                                |(lx, ly, rx, by), (x, y)| {
                                    (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                                },
                            );
                            (min_x, min_y, max_x - min_x, max_y - min_y, None)
                        };

                    let width = pt.call1((width_pt,))?;
                    let height = pt.call1((height_pt,))?;
                    let left = pt.call1((left_pt,))?;
                    let top = pt.call1((top_pt,))?;

                    let picture =
                        shapes.call_method1("add_picture", (buffer, left, top, width, height))?;
                    if let Some(rotation_deg) = rotation_deg {
                        picture.setattr("rotation", rotation_deg)?;
                    }
                }
                ImageKind::Svg(svg) => {
                    // Render the SVG into a PNG buffer sized to the Typst layout box.
                    // Double the raster resolution while keeping slide size unchanged.
                    let scale_factor = 2.0;
                    let to_px = |pt: f64| ((pt * 96.0 / 72.0 * scale_factor).max(1.0).ceil()) as u32;
                    let width_pt = size.x.to_pt();
                    let height_pt = size.y.to_pt();
                    let width_px = to_px(width_pt);
                    let height_px = to_px(height_pt);

                    let mut pixmap = match tiny_skia::Pixmap::new(width_px, height_px) {
                        Some(pixmap) => pixmap,
                        None => {
                            eprintln!(
                                "SVG could not allocate pixmap at {}x{}; skipping.",
                                width_px, height_px
                            );
                            continue;
                        }
                    };

                    let tree = svg.tree();
                    let scale = tiny_skia::Transform::from_scale(
                        width_px as f32 / tree.size().width(),
                        height_px as f32 / tree.size().height(),
                    );
                    resvg::render(tree, scale, &mut pixmap.as_mut());

                    let png_bytes = pixmap
                        .encode_png()
                        .map_err(|err| PyRuntimeError::new_err(format!("SVG encode failed: {err}")))?;

                    let image_bytes = PyBytes::new(py, &png_bytes);
                    let buffer = io.getattr("BytesIO")?.call1((image_bytes,))?;

                    let (left_pt, top_pt, width_pt, height_pt, rotation_deg) =
                        if let Some((rotation, scale_x, scale_y)) =
                            item_transform.decompose_rotation_scale()
                        {
                            let (center_x, center_y) = item_transform.apply_point(
                                width_pt / 2.0,
                                height_pt / 2.0,
                            );
                            (
                                center_x - (width_pt * scale_x) / 2.0,
                                center_y - (height_pt * scale_y) / 2.0,
                                width_pt * scale_x,
                                height_pt * scale_y,
                                Some(rotation.to_degrees()),
                            )
                        } else {
                            let corners = [
                                item_transform.apply_point(0.0, 0.0),
                                item_transform.apply_point(width_pt, 0.0),
                                item_transform.apply_point(width_pt, height_pt),
                                item_transform.apply_point(0.0, height_pt),
                            ];
                            let (min_x, min_y, max_x, max_y) = corners.iter().fold(
                                (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
                                |(lx, ly, rx, by), (x, y)| {
                                    (lx.min(*x), ly.min(*y), rx.max(*x), by.max(*y))
                                },
                            );
                            (min_x, min_y, max_x - min_x, max_y - min_y, None)
                        };
                    let width = pt.call1((width_pt,))?;
                    let height = pt.call1((height_pt,))?;
                    let left = pt.call1((left_pt,))?;
                    let top = pt.call1((top_pt,))?;

                    let picture =
                        shapes.call_method1("add_picture", (buffer, left, top, width, height))?;
                    if let Some(rotation_deg) = rotation_deg {
                        picture.setattr("rotation", rotation_deg)?;
                    }
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
                Affine::identity(),
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

    let world = TypstWrapperWorld::new("src".to_owned(), content);

    let document: PagedDocument = typst::compile(&world)
        .output
        .expect("Typst compilation failed");

    let equation_captures = collect_equations(&document);
    let equation_images =
        render_equations_to_png(&document, &equation_captures, Path::new("equations"))
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;

    walk_paged_document(document, &equation_images)
}
