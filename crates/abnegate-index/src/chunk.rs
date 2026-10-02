//! Overlapping windows of source text.

/// Overlapping window length, in characters.
pub const WINDOW: usize = 4000;

/// Characters reused from the previous window.
pub const OVERLAP: usize = 400;

/// Windows shorter than this are dropped.
pub const SMALLEST: usize = 15;

/// Overlapping windows of source text, split on line boundaries when possible.
pub fn chunks(blob: &str) -> Vec<String> {
    let blob = blob.trim();
    if blob.len() < SMALLEST {
        return Vec::new();
    }
    if blob.len() <= WINDOW {
        return vec![blob.to_string()];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < blob.len() {
        let raw_end = start.saturating_add(WINDOW).min(blob.len());
        let mut end = floor_char(blob, raw_end);
        if end < blob.len()
            && let Some(relative) = blob[start..end].rfind('\n')
        {
            end = start + relative + 1;
        }
        if end <= start {
            end = ceil_char(blob, start.saturating_add(1).min(blob.len()));
        }
        let piece = blob[start..end].trim();
        if piece.len() >= SMALLEST {
            out.push(piece.to_string());
        }
        if end >= blob.len() {
            break;
        }
        let next = floor_char(blob, end.saturating_sub(OVERLAP));
        start = if next > start { next } else { end };
    }
    out
}

fn floor_char(blob: &str, mut index: usize) -> usize {
    if index >= blob.len() {
        return blob.len();
    }
    while index > 0 && !blob.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char(blob: &str, mut index: usize) -> usize {
    if index >= blob.len() {
        return blob.len();
    }
    while index < blob.len() && !blob.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_file_is_one_chunk() {
        let blob = "class API::BulkImports < Grape::API\n  get ':id/entities' do\n  end\nend\n";
        let pieces = chunks(blob);
        assert_eq!(pieces.len(), 1);
        assert!(pieces[0].contains("get ':id/entities'"));
    }

    #[test]
    fn short_blob_is_dropped() {
        assert!(chunks("fn x() {}").is_empty());
    }

    #[test]
    fn overlapping_windows_cover_a_long_blob() {
        let blob = "alpha\n".repeat(WINDOW / 3);
        let pieces = chunks(&blob);
        assert!(pieces.len() >= 2);
        let last = pieces.last().unwrap();
        assert!(blob.contains(last));
        assert!(pieces[0].len() >= SMALLEST);
    }

    #[test]
    fn windows_split_on_character_boundaries() {
        let blob = "é".repeat(WINDOW + 50);
        let pieces = chunks(&blob);
        assert!(pieces.len() >= 2);
        for piece in &pieces {
            assert!(piece.is_char_boundary(piece.len()));
        }
    }
}
