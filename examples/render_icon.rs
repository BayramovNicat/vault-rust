//! Renders assets/icon.svg to a PNG: `cargo run --example render_icon -- <size> <out.png>`.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let size: u32 = args[1].parse().expect("size");
    let svg = std::fs::read("assets/icon.svg").expect("assets/icon.svg");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).expect("parse svg");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.save_png(&args[2]).expect("write png");
}
