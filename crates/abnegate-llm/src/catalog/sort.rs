use serde::Deserialize;
use serde::Serialize;

/// How to order browse results.
///
/// Serialised in snake case; the abbreviated names an earlier release wrote,
/// such as `params_asc`, are still read.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelSort {
    /// No reordering: models come as the catalogue ranks them, which for
    /// Hugging Face is by downloads. The default.
    #[default]
    Relevance,
    /// Most downloaded first.
    #[serde(alias = "downloads_desc")]
    DownloadsDescending,
    /// Least downloaded first.
    #[serde(alias = "downloads_asc")]
    DownloadsAscending,
    /// A to Z, ignoring case.
    #[serde(alias = "name_asc")]
    NameAscending,
    /// Z to A, ignoring case.
    #[serde(alias = "name_desc")]
    NameDescending,
    /// Smallest download first.
    #[serde(alias = "size_asc")]
    SizeAscending,
    /// Largest download first.
    #[serde(alias = "size_desc")]
    SizeDescending,
    /// Fewest parameters first.
    #[serde(alias = "params_asc")]
    ParametersAscending,
    /// Most parameters first.
    #[serde(alias = "params_desc")]
    ParametersDescending,
    /// Most recently updated first.
    #[serde(alias = "updated_desc")]
    UpdatedDescending,
    /// Least recently updated first.
    #[serde(alias = "updated_asc")]
    UpdatedAscending,
}

#[cfg(test)]
mod tests {
    use super::ModelSort;

    #[test]
    fn a_sort_is_written_in_full_and_read_in_either_spelling() {
        assert_eq!(
            serde_json::to_string(&ModelSort::ParametersAscending).unwrap(),
            "\"parameters_ascending\""
        );
        for (name, expected) in [
            ("parameters_ascending", ModelSort::ParametersAscending),
            ("params_asc", ModelSort::ParametersAscending),
            ("name_desc", ModelSort::NameDescending),
            ("downloads_descending", ModelSort::DownloadsDescending),
        ] {
            assert_eq!(
                serde_json::from_str::<ModelSort>(&format!("\"{name}\"")).unwrap(),
                expected
            );
        }
    }
}
