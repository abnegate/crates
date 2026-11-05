use serde::Deserialize;

use crate::modality::vendor::openai_compatible::item::Item;
use crate::provider::ProviderError;

#[derive(Debug, Deserialize)]
pub(super) struct Response {
    data: Vec<Item>,
}

impl Response {
    pub(super) fn into_embeddings(self, inputs: usize) -> Result<Vec<Vec<f32>>, ProviderError> {
        if self.data.len() != inputs {
            return Err(ProviderError::parse(format!(
                "{} embeddings came back for {inputs} inputs",
                self.data.len()
            )));
        }

        let mut embeddings: Vec<Option<Vec<f32>>> =
            std::iter::repeat_with(|| None).take(inputs).collect();
        for item in self.data {
            let slot = embeddings.get_mut(item.index).ok_or_else(|| {
                ProviderError::parse(format!(
                    "embedding index {} is outside the {inputs} inputs",
                    item.index
                ))
            })?;
            if slot.replace(item.embedding).is_some() {
                return Err(ProviderError::parse(format!(
                    "embedding index {} came back twice",
                    item.index
                )));
            }
        }

        embeddings
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| ProviderError::parse("an input has no embedding"))
    }
}
