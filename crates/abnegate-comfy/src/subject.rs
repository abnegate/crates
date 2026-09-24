//! Where the subject of a training image is.
//!
//! Training images are cropped square, and a crop taken on faith puts whatever
//! happened to be in the middle of the frame at the middle of the dataset. So
//! the subject is located first, through `abnegate-vision`'s port of autogravity:
//! U2-Net segments what looks like a subject, and the crop is framed on the
//! centre of mass of that.
//!
//! The model is 168 MiB and is not vendored, so every entry point here degrades
//! rather than fails. Without it a photo is framed on its centre and a video
//! frame on whatever moved, which is what they did before autogravity was
//! wired in at all.

mod error;
#[cfg(feature = "saliency")]
mod models;

pub use error::Error;

use crate::config::Config;
use abnegate_vision::crop::{self, Rendered, Target};
use abnegate_vision::gravity::Point;
use abnegate_vision::{Raster, decode};

/// The centre of the frame, and the answer whenever nothing better is known.
pub const CENTRE: Point = Point { x: 0.5, y: 0.5 };

/// Locates training subjects against one shared model.
#[derive(Clone)]
pub struct Subject {
    #[cfg(feature = "saliency")]
    analyzer: Option<std::sync::Arc<abnegate_vision::Analyzer>>,
}

impl Subject {
    /// The analyzer for the configured model, loaded on first use.
    ///
    /// One model is 168 MiB of resident ONNX Runtime arena, so each one is
    /// loaded once for the process, off the async runtime, and shared by every
    /// caller after that. A model that fails to load is tried again next time.
    #[cfg(feature = "saliency")]
    pub async fn shared(config: &Config) -> Self {
        static ANALYZERS: std::sync::LazyLock<models::Models<abnegate_vision::Analyzer>> =
            std::sync::LazyLock::new(models::Models::default);

        let Some(path) = config.vision_model.clone() else {
            return Self::none();
        };
        Self {
            analyzer: ANALYZERS
                .load(path, |path| abnegate_vision::Analyzer::open(path))
                .await,
        }
    }

    #[cfg(not(feature = "saliency"))]
    pub async fn shared(_config: &Config) -> Self {
        Self {}
    }

    /// An instance that always falls back, for callers that want the geometry
    /// without the model.
    pub fn none() -> Self {
        Self {
            #[cfg(feature = "saliency")]
            analyzer: None,
        }
    }

    #[cfg(feature = "saliency")]
    pub fn available(&self) -> bool {
        self.analyzer.is_some()
    }

    #[cfg(not(feature = "saliency"))]
    pub fn available(&self) -> bool {
        false
    }

    /// Where the subject of one image sits, or `fallback` when the model
    /// cannot say.
    pub fn focus(&self, raster: &Raster, fallback: Point) -> Point {
        self.weighted(raster, &[], fallback)
    }

    /// The same, with a caller's own map biasing the model's.
    ///
    /// `bias` is a square grid over the image, and a cell's weight is doubled
    /// where the bias peaks. It is enough to pick the moving subject out of a
    /// group and never enough to invent one where the model saw none, which is
    /// what a video frame wants: saliency says what looks like a subject,
    /// motion says which one is being filmed.
    pub fn weighted(&self, raster: &Raster, bias: &[f32], fallback: Point) -> Point {
        #[cfg(feature = "saliency")]
        if let Some(analyzer) = &self.analyzer {
            let located = analyzer.saliency(raster, |map, content| centre(map, content, bias));
            match located {
                Ok(Some(point)) => return point,
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "subject detection failed for one image"),
            }
        }
        let _ = (raster, bias);
        fallback
    }

    /// Decodes an image and renders the square crop framed on its subject.
    pub fn crop(&self, data: &[u8], side: u32) -> Result<Rendered, Error> {
        let raster = decode::decode(data)?;
        self.render(&raster, side, self.focus(&raster, CENTRE))
    }

    /// Renders one crop at an already-decided focus, so a pair of images that
    /// have to stay aligned can share one.
    pub fn render(&self, raster: &Raster, side: u32, focus: Point) -> Result<Rendered, Error> {
        let target = Target::square(side);
        let region = crop::plan(raster.oriented_size(), target, focus)?;
        Ok(crop::render(raster, region, target)?)
    }
}

/// Bias below this share of its peak is background, not the subject. Ignoring
/// it keeps a large still background from outweighing a small moving subject.
#[cfg(feature = "saliency")]
const FLOOR: f32 = 0.55;

/// The centre of mass of the saliency map inside `content`, with `bias`
/// stretched over it. `None` when the model found nothing to weigh.
#[cfg(feature = "saliency")]
fn centre(
    map: &[f32],
    content: abnegate_vision::gravity::Rectangle,
    bias: &[f32],
) -> Option<Point> {
    use abnegate_vision::saliency::{INPUT_HEIGHT, INPUT_WIDTH};

    let peak = bias.iter().copied().fold(0.0f32, f32::max);
    let side = (bias.len() as f64).sqrt() as usize;
    let weighted: Vec<f32> = if peak <= 0.0 || side * side != bias.len() {
        map.to_vec()
    } else {
        let floor = peak * FLOOR;
        (0..map.len())
            .map(|index| {
                let x = (index % INPUT_WIDTH as usize) as i32;
                let y = (index / INPUT_WIDTH as usize) as i32;
                if x < content.left || x >= content.right || y < content.top || y >= content.bottom
                {
                    return 0.0;
                }
                let column = ((x - content.left) as usize * side) / content.width().max(1) as usize;
                let row = ((y - content.top) as usize * side) / content.height().max(1) as usize;
                let moved = bias[row.min(side - 1) * side + column.min(side - 1)];
                map[index] * (1.0 + if moved >= floor { moved / peak } else { 0.0 })
            })
            .collect()
    };
    let (point, confidence) = abnegate_vision::gravity::from_saliency_region(
        &weighted,
        INPUT_WIDTH,
        INPUT_HEIGHT,
        content,
    )
    .ok()?;
    (confidence > 0.0).then_some(point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use abnegate_vision::decode::Layout;

    #[test]
    fn without_a_model_every_image_keeps_its_fallback() {
        let subject = Subject::none();
        assert!(!subject.available());
        let raster = Raster::new(4, 4, Layout::Rgb, vec![128; 4 * 4 * 3]);
        let elsewhere = Point { x: 0.2, y: 0.8 };
        assert_eq!(subject.focus(&raster, elsewhere), elsewhere);
        assert_eq!(subject.weighted(&raster, &[1.0; 16], elsewhere), elsewhere);
    }

    fn encoded(width: u32, height: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![90u8; (width * height * 3) as usize],
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_crop_without_a_model_is_still_the_square_the_trainer_asked_for() {
        let rendered = Subject::none().crop(&encoded(16, 9), 8).unwrap();
        assert_eq!((rendered.width, rendered.height), (8, 8));
    }

    #[test]
    fn something_that_is_not_an_image_cannot_be_cropped() {
        let error = Subject::none().crop(b"not an image", 8).unwrap_err();
        assert!(matches!(error, Error::Decode(_)), "{error}");
    }

    #[test]
    fn a_zero_sided_crop_is_refused_rather_than_rendered() {
        let raster = Raster::new(4, 4, Layout::Rgb, vec![128; 4 * 4 * 3]);
        let error = Subject::none().render(&raster, 0, CENTRE).unwrap_err();
        assert!(matches!(error, Error::Crop(_)), "{error}");
    }

    #[test]
    fn a_crop_is_the_square_the_trainer_asked_for() {
        let raster = Raster::new(8, 4, Layout::Rgb, vec![64; 8 * 4 * 3]);
        let rendered = Subject::none().render(&raster, 4, CENTRE).unwrap();
        assert_eq!((rendered.width, rendered.height), (4, 4));
        assert_eq!(rendered.pixels.len(), 4 * 4 * 3);
    }
}
