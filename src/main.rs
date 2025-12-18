use std::fs;

use typst::layout::{Frame, FrameItem, PagedDocument};
use typst_as_library::TypstWrapperWorld;

fn walk_frame(frame: Frame) {
    let items_iter = frame.items();
    for item in items_iter {
        let point = item.0;
        match item.1.to_owned() {
            FrameItem::Group(group_item) => {
                walk_frame(group_item.frame);
            }
            FrameItem::Text(text_item) => {}
            FrameItem::Shape(shape, span) => {}
            FrameItem::Image(image, axes, span) => {}
            FrameItem::Link(destination, axes) => {}
            FrameItem::Tag(tag) => {}
        }
    }
}

fn walk_paged_document(paged_doc: PagedDocument) {
    let pages = paged_doc.pages;
    for page in pages {
        walk_frame(page.frame)
    }
}

fn main() {
    let content = fs::read_to_string("src/source.typ").expect("Unable to read file.");
    let world = TypstWrapperWorld::new("src/".to_owned(), content);
    let document: PagedDocument = typst::compile(&world)
        .output
        .expect("Error compiling typst");

    walk_paged_document(document);
}
