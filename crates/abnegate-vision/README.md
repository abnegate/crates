# abnegate-vision

Subject-aware image cropping. It finds the visual subject of an image and frames
a crop on it, so a pipeline keeps the subject instead of whatever happened to be
in the middle of the frame. Detection reproduces
[autogravity](https://github.com/appwrite/autogravity), running U2-Net through
ONNX Runtime behind the `saliency` feature; decoding JPEG, PNG and WebP, crop
planning and rendering are always available, so a caller that already knows
where the subject is can crop without the model. The crop, the downscale and any
EXIF rotation happen in one resampling pass over the source, so no full-size
intermediate is ever built.

## Features

- `saliency`: `Analyzer` and the `saliency` module, subject detection with U2-Net over ONNX Runtime. The roughly 168 MiB `u2net.onnx` export is not vendored. The build downloads ONNX Runtime's prebuilt binaries; to load a runtime you ship instead, enable `ort/load-dynamic` in your own manifest.

## Usage

```sh
cargo add abnegate-vision
```

```rust,no_run
use abnegate_vision::{Error, Point, Rendered, Target, crop, decode};

fn frame(data: &[u8]) -> Result<Rendered, Error> {
    let raster = decode::decode(data)?;
    let focus = Point { x: 0.5, y: 0.33 };
    let region = crop::plan(raster.oriented_size(), Target::square(1024), focus)?;
    Ok(crop::render(&raster, region, Target::square(1024))?)
}
```

With `saliency`, `Analyzer::open` loads the model and `Analyzer::crop` finds the
focus itself.
