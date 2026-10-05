use crate::subject::SaliencyMap;
use abnegate_vision::crop;
use abnegate_vision::crop::Region;
use abnegate_vision::crop::Target;
use abnegate_vision::gravity::Point;

/// Where the subject of one image sits, and the saliency map it was found on
/// when the model ran.
pub(crate) struct Located {
    pub(crate) focus: Point,
    pub(crate) map: Option<SaliencyMap>,
}

impl Located {
    /// The 8-bit subject mask of the crop of `region` rendered to `target`,
    /// from an image of oriented size `source`. `None` without a map.
    pub(crate) fn mask(
        &self,
        source: (u32, u32),
        region: Region,
        target: Target,
    ) -> Option<Vec<u8>> {
        let map = self.map.as_ref()?;
        crop::render_mask(&map.values, map.size, map.content, source, region, target)
            .inspect_err(|error| tracing::warn!(%error, "subject mask failed for one crop"))
            .ok()
    }
}
