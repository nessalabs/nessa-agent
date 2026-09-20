//! Fit one image file to limits given on the command line.
//!
//! ```text
//! cargo run -p nessa-images --example fit -- photo.heic fitted 3750000 2000
//! ```
//!
//! writes `fitted.jpg` or `fitted.png`. The library itself reads no files; this
//! example does the reading and writing around it.
use nessa_images::{normalize, Encoding, Limits};
use std::{env, error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let [input, output, max_bytes, max_long_edge_px] = arguments.as_slice() else {
        return Err("usage: fit <input> <output-stem> <max-bytes> <max-long-edge-px>".into());
    };
    let limits = Limits::new(
        vec![Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp],
        max_bytes.parse()?,
        max_long_edge_px.parse()?,
    )?;
    let original = fs::read(input)?;
    let fitted = normalize(&original, &limits)?;
    let extension = match fitted.encoding {
        Encoding::Png => "png",
        Encoding::Jpeg => "jpg",
        Encoding::Gif => "gif",
        Encoding::Webp => "webp",
    };
    let path = format!("{output}.{extension}");
    fs::write(&path, &fitted.bytes)?;
    println!(
        "{} bytes -> {} bytes, {}x{} {}, {}: {path}",
        original.len(),
        fitted.bytes.len(),
        fitted.width,
        fitted.height,
        fitted.encoding.media_type(),
        if fitted.changed {
            "re-encoded"
        } else {
            "unchanged"
        },
    );
    Ok(())
}
