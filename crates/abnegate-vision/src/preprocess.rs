//! Fits a decoded raster into the model's input tensor.

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::decode::{Layout, Orientation, Raster};
use crate::gravity::Rectangle;

mod error;

pub use crate::preprocess::error::PreprocessError;

/// ImageNet normalization, pre-folded into `value * scale + bias` per channel.
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STANDARD_DEVIATION: [f32; 3] = [0.229, 0.224, 0.225];

/// Reusable scratch for one preprocessing pipeline. Sizing the buffers once at
/// construction keeps steady-state analysis free of heap traffic.
pub struct Preprocessor {
    resizer: Resizer,
    resized: Vec<u8>,
    tensor: Vec<f32>,
    width: u32,
    height: u32,
}

impl Preprocessor {
    pub fn new(width: u32, height: u32) -> Self {
        let pixels = width as usize * height as usize;
        Self {
            resizer: Resizer::new(),
            resized: vec![0; pixels * Layout::Rgba.channels()],
            tensor: vec![0.0; pixels * 3],
            width,
            height,
        }
    }

    /// The normalized RGB NCHW tensor produced by the last [`Self::prepare`].
    pub fn tensor(&self) -> &[f32] {
        &self.tensor
    }

    /// Fits the raster inside the tensor without changing its aspect ratio,
    /// applying EXIF orientation and ImageNet normalization on the way. Unused
    /// tensor pixels stay neutral zero padding. Returns the rectangle the image
    /// content occupies.
    pub fn prepare(&mut self, raster: &Raster) -> Result<Rectangle, PreprocessError> {
        if self.width == 0 || self.height == 0 {
            return Err(PreprocessError::Dimensions);
        }
        if raster.width == 0 || raster.height == 0 {
            return Err(PreprocessError::Source);
        }

        let (source_width, source_height) = raster.oriented_size();
        let scale = f64::from(self.width) / f64::from(source_width);
        let scale = scale.min(f64::from(self.height) / f64::from(source_height));
        let fitted_width = fit(source_width, scale, self.width);
        let fitted_height = fit(source_height, scale, self.height);
        let offset_x = (self.width - fitted_width) / 2;
        let offset_y = (self.height - fitted_height) / 2;

        let (resized_width, resized_height) = if raster.orientation.swaps_axes() {
            (fitted_height, fitted_width)
        } else {
            (fitted_width, fitted_height)
        };
        self.resize(raster, resized_width, resized_height)?;

        self.tensor.fill(0.0);
        self.write_tensor(
            raster.layout,
            raster.orientation,
            (resized_width, resized_height),
            (fitted_width, fitted_height),
            (offset_x, offset_y),
        );

        Ok(Rectangle::new(
            offset_x as i32,
            offset_y as i32,
            (offset_x + fitted_width) as i32,
            (offset_y + fitted_height) as i32,
        ))
    }

    fn resize(&mut self, raster: &Raster, width: u32, height: u32) -> Result<(), PreprocessError> {
        let pixel_type = if raster.layout == Layout::Rgb {
            PixelType::U8x3
        } else {
            PixelType::U8x4
        };
        let source = ImageRef::new(raster.width, raster.height, &raster.pixels, pixel_type)?;
        let channels = raster.layout.channels();
        let target = &mut self.resized[..width as usize * height as usize * channels];
        let mut destination = Image::from_slice_u8(width, height, target, pixel_type)?;

        // Parity with the Go reference: its imaging.Linear over unpremultiplied NRGBA.
        let options = ResizeOptions::new()
            .resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))
            .use_alpha(false);
        self.resizer.resize(&source, &mut destination, &options)?;
        Ok(())
    }

    fn write_tensor(
        &mut self,
        layout: Layout,
        orientation: Orientation,
        resized: (u32, u32),
        fitted: (u32, u32),
        offset: (u32, u32),
    ) {
        let plane = self.width as usize * self.height as usize;
        let (red, rest) = self.tensor.split_at_mut(plane);
        let (green, blue) = rest.split_at_mut(plane);
        let planes = [red, green, blue];

        let channels = layout.channels();
        let opaque = layout == Layout::Rgb;
        let sample_scale = if opaque { 1.0 / 255.0 } else { 1.0 / 65535.0 };
        let scale: [f32; 3] =
            std::array::from_fn(|channel| sample_scale / STANDARD_DEVIATION[channel]);
        let bias: [f32; 3] =
            std::array::from_fn(|channel| -MEAN[channel] / STANDARD_DEVIATION[channel]);

        let (fitted_width, fitted_height) = (fitted.0 as usize, fitted.1 as usize);
        let stride = resized.0 as usize * channels;
        let width = self.width as usize;

        for y in 0..fitted_height {
            let row = (y + offset.1 as usize) * width + offset.0 as usize;
            if orientation == Orientation::Normal {
                let source = &self.resized[y * stride..y * stride + fitted_width * channels];
                for (x, pixel) in source.chunks_exact(channels).enumerate() {
                    let alpha = if opaque { 1 } else { u32::from(pixel[3]) };
                    for channel in 0..3 {
                        let value = sample(pixel[channel], alpha, opaque);
                        planes[channel][row + x] = value * scale[channel] + bias[channel];
                    }
                }
                continue;
            }
            for x in 0..fitted_width {
                let (source_x, source_y) = map_pixel(orientation, x as u32, y as u32, resized);
                let base = source_y as usize * stride + source_x as usize * channels;
                let pixel = &self.resized[base..base + channels];
                let alpha = if opaque { 1 } else { u32::from(pixel[3]) };
                for channel in 0..3 {
                    let value = sample(pixel[channel], alpha, opaque);
                    planes[channel][row + x] = value * scale[channel] + bias[channel];
                }
            }
        }
    }
}

/// Premultiplies alpha against black with the integer math the Go reference
/// performs when it reads NRGBA through `color.Color.RGBA()`.
#[inline(always)]
fn sample(component: u8, alpha: u32, opaque: bool) -> f32 {
    if opaque {
        f32::from(component)
    } else {
        ((u32::from(component) * 257) * alpha / 255) as f32
    }
}

/// Maps a coordinate in the oriented image back to the stored pixel buffer.
#[inline(always)]
pub(crate) fn map_pixel(orientation: Orientation, x: u32, y: u32, size: (u32, u32)) -> (u32, u32) {
    let (width, height) = size;
    match orientation {
        Orientation::Normal => (x, y),
        Orientation::FlipHorizontal => (width - 1 - x, y),
        Orientation::Rotate180 => (width - 1 - x, height - 1 - y),
        Orientation::FlipVertical => (x, height - 1 - y),
        Orientation::Transpose => (y, x),
        Orientation::Rotate90 => (y, height - 1 - x),
        Orientation::Transverse => (width - 1 - y, height - 1 - x),
        Orientation::Rotate270 => (width - 1 - y, x),
    }
}

fn fit(source: u32, scale: f64, limit: u32) -> u32 {
    let scaled = (f64::from(source) * scale).round() as i64;
    scaled.clamp(1, i64::from(limit)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(width: u32, height: u32, layout: Layout, orientation: Orientation) -> Raster {
        let pixels = vec![200u8; (width * height) as usize * layout.channels()];
        Raster::new(width, height, layout, pixels).with_orientation(orientation)
    }

    #[test]
    fn produces_normalized_nchw() {
        let mut preprocessor = Preprocessor::new(1, 1);
        let source = Raster::new(1, 1, Layout::Rgb, vec![255, 128, 0]);
        let content = preprocessor.prepare(&source).unwrap();

        assert_eq!(content, Rectangle::new(0, 0, 1, 1));
        let want = [
            (1.0 - 0.485) / 0.229,
            (128.0 / 255.0 - 0.456) / 0.224,
            (0.0 - 0.406) / 0.225,
        ];
        for (got, want) in preprocessor.tensor().iter().zip(want) {
            assert!((got - want).abs() < 1e-4, "got {got}, want {want}");
        }
    }

    #[test]
    fn preserves_aspect_ratio_with_neutral_padding() {
        for (width, height, want) in [
            (4, 2, Rectangle::new(0, 1, 4, 3)),
            (2, 4, Rectangle::new(1, 0, 3, 4)),
            (4, 4, Rectangle::new(0, 0, 4, 4)),
        ] {
            let mut preprocessor = Preprocessor::new(4, 4);
            let source = raster(width, height, Layout::Rgb, Orientation::Normal);
            let content = preprocessor.prepare(&source).unwrap();
            assert_eq!(content, want);

            for y in 0..4i32 {
                for x in 0..4i32 {
                    let inside = x >= content.left
                        && x < content.right
                        && y >= content.top
                        && y < content.bottom;
                    for channel in 0..3 {
                        let value = preprocessor.tensor()[channel * 16 + (y * 4 + x) as usize];
                        assert_eq!(inside, value != 0.0, "at ({x}, {y}) channel {channel}");
                    }
                }
            }
        }
    }

    #[test]
    fn orientation_swaps_the_content_rectangle() {
        let mut preprocessor = Preprocessor::new(4, 4);
        let source = raster(4, 2, Layout::Rgb, Orientation::Rotate90);
        assert_eq!(
            preprocessor.prepare(&source).unwrap(),
            Rectangle::new(1, 0, 3, 4)
        );
    }

    #[test]
    fn rejects_empty_images() {
        let mut preprocessor = Preprocessor::new(4, 4);
        let source = raster(0, 0, Layout::Rgb, Orientation::Normal);
        assert!(matches!(
            preprocessor.prepare(&source),
            Err(PreprocessError::Source)
        ));
    }
}
