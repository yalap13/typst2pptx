use pyo3::prelude::*;
use std::{env, fs};

use typst::layout::{Frame, FrameItem, PagedDocument};
use typst_as_library::TypstWrapperWorld;

fn walk_frame<'py>(frame: Frame, slide: &Bound<'py, PyAny>) -> PyResult<()> {
    let items_iter = frame.items();
    for item in items_iter {
        let point = item.0;
        match item.1.to_owned() {
            FrameItem::Group(group_item) => {
                walk_frame(group_item.frame, slide)?;
            }
            FrameItem::Text(text_item) => {}
            FrameItem::Shape(shape, span) => {}
            FrameItem::Image(image, axes, span) => {}
            FrameItem::Link(destination, axes) => {}
            FrameItem::Tag(tag) => {}
        }
    }
    Ok(())
}

fn walk_paged_document(paged_doc: PagedDocument) -> PyResult<()> {
    Python::attach(|py| {
        let pptx = py.import("pptx")?;
        let util = py.import("pptx.util")?;
        let inches = util.getattr("Inches")?;
        let pts = util.getattr("Pt")?;

        let presentation = pptx.getattr("Presentation")?.call0()?;
        let slides = presentation.getattr("slides")?;
        let layouts = presentation.getattr("slide_layouts")?;
        let blank_layout = layouts.get_item(6)?;

        // Set slide size
        // Take the first page as reference (Typst pages are uniform)
        let first_page = &paged_doc.pages[0];
        let width_pt = first_page.frame.width().to_pt();
        let height_pt = first_page.frame.height().to_pt();

        presentation.setattr("slide_width", pts.call1((width_pt,))?)?;
        presentation.setattr("slide_height", pts.call1((height_pt,))?)?;

        let pages = paged_doc.pages;
        for page in pages {
            let slide = slides.call_method1("add_slide", (blank_layout.clone(),))?;
            walk_frame(page.frame, &slide)?;
        }

        presentation.call_method1("save", ("my_presentation.pptx",))?;
        Ok(())
    })
}

fn main() -> PyResult<()> {
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
    let content = fs::read_to_string("src/source.typ").expect("Unable to read file.");
    let world = TypstWrapperWorld::new("src/".to_owned(), content);
    let document: PagedDocument = typst::compile(&world)
        .output
        .expect("Error compiling typst");

    // print!("{:?}", document);

    walk_paged_document(document)
}
