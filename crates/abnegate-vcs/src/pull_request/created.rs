/// Created PR result
#[derive(Debug, Clone)]
pub struct CreatedPr {
    pub url: String,
    pub number: i64,
    pub state: String,
}
