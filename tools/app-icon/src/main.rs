//! GPUI 0.2.2 と同じ2倍の画素・alpha変換で、SVGから表示用PNGを再生成する。
//! cargo run --release --manifest-path tools/app-icon/Cargo.toml -- assets/app-icon/rireki.svg assets/app-icon/rireki.png
use std::{env, fs, path::Path};

fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(args.len(), 3, "SVGと出力PNGのパスを指定してください");
    let bytes = fs::read(&args[1]).expect("SVGを読み込めません");
    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).expect("SVGを解析できません");
    let dimensions = tree.size();
    let width = (dimensions.width() * 2.0) as u32;
    let height = (dimensions.height() * 2.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).expect("画像サイズが不正です");
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
        &mut pixmap.as_mut(),
    );
    let mut rgba = pixmap.take();
    // GPUIのSVGローダーと同じf32除算・切り捨てで、premultiplied alphaを戻す。
    for pixel in rgba.as_chunks_mut::<4>().0 {
        if pixel[3] > 0 {
            let alpha = pixel[3] as f32 / 255.0;
            for value in &mut pixel[..3] {
                *value = (*value as f32 / alpha) as u8;
            }
        }
    }
    image::save_buffer_with_format(
        Path::new(&args[2]),
        &rgba,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .expect("PNGを保存できません");
    println!("{width}×{height}: {}", args[2]);
}
