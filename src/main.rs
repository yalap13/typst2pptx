use std::fs;

use typst::layout::PagedDocument;
use typst_as_library::TypstWrapperWorld;

fn main() {
    let content = fs::read_to_string("src/source.typ").expect("Unable to read file.");
    let world = TypstWrapperWorld::new("src/".to_owned(), content);
    let document: PagedDocument = typst::compile(&world)
        .output
        .expect("Error compiling typst");
    let pages = document.pages;
    let page = pages.get(3).unwrap().to_owned();
    let frame = page.frame;
    let items_iter = frame.items();

    for elem in items_iter {
        let point = elem.0;
        let item = elem.1.to_owned();
        println!("{}, {}, {:?}", point.x.to_pt(), point.y.to_pt(), item)
    }
}
