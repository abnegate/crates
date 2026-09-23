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
    #[default]
    Relevance,
    #[serde(alias = "downloads_desc")]
    DownloadsDescending,
    #[serde(alias = "downloads_asc")]
    DownloadsAscending,
    #[serde(alias = "name_asc")]
    NameAscending,
    #[serde(alias = "name_desc")]
    NameDescending,
    #[serde(alias = "size_asc")]
    SizeAscending,
    #[serde(alias = "size_desc")]
    SizeDescending,
    #[serde(alias = "params_asc")]
    ParametersAscending,
    #[serde(alias = "params_desc")]
    ParametersDescending,
    #[serde(alias = "updated_desc")]
    UpdatedDescending,
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
