use std::fmt::Write;

use abnegate_secret::{MasterKey, SecretValue, decrypt_value, encrypt_value, is_encrypted};
use toml::Value;

use crate::error::ConfigError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Segment {
    Key(String),
    Index(usize),
}

pub(crate) type Location = Vec<Segment>;

pub(crate) fn unseal(document: &mut Value, key: &MasterKey) -> Result<Vec<Location>, ConfigError> {
    let mut sealed = Vec::new();
    let mut location = Location::new();
    walk(document, key, &mut location, &mut sealed)?;
    Ok(sealed)
}

pub(crate) fn seal(
    document: &mut Value,
    locations: &[Location],
    key: &MasterKey,
) -> Result<(), ConfigError> {
    for location in locations {
        let Some(Value::String(text)) = resolve(document, location) else {
            continue;
        };
        if is_encrypted(text) {
            continue;
        }

        *text = encrypt_value(&SecretValue::new(text.as_str()), key).map_err(|source| {
            ConfigError::Encrypt {
                field: describe(location),
                source,
            }
        })?;
    }

    Ok(())
}

fn walk(
    value: &mut Value,
    key: &MasterKey,
    location: &mut Location,
    sealed: &mut Vec<Location>,
) -> Result<(), ConfigError> {
    match value {
        Value::String(text) if is_encrypted(text) => {
            let plaintext = decrypt_value(text, key).map_err(|source| ConfigError::Decrypt {
                field: describe(location),
                source,
            })?;
            *text = plaintext.expose().to_string();
            sealed.push(location.clone());
        }
        Value::Table(table) => {
            for (name, child) in table.iter_mut() {
                location.push(Segment::Key(name.clone()));
                walk(child, key, location, sealed)?;
                location.pop();
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter_mut().enumerate() {
                location.push(Segment::Index(index));
                walk(child, key, location, sealed)?;
                location.pop();
            }
        }
        _ => {}
    }

    Ok(())
}

fn resolve<'a>(document: &'a mut Value, location: &Location) -> Option<&'a mut Value> {
    let mut current = document;

    for segment in location {
        current = match segment {
            Segment::Key(name) => current.as_table_mut()?.get_mut(name)?,
            Segment::Index(index) => current.as_array_mut()?.get_mut(*index)?,
        };
    }

    Some(current)
}

fn describe(location: &[Segment]) -> String {
    let mut description = String::new();

    for segment in location {
        match segment {
            Segment::Key(name) => {
                if !description.is_empty() {
                    description.push('.');
                }
                description.push_str(name);
            }
            Segment::Index(index) => {
                let _ = write!(description, "[{index}]");
            }
        }
    }

    description
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed_document(key: &MasterKey) -> Value {
        let token = encrypt_value(&SecretValue::new("hunter2"), key).unwrap();
        toml::from_str(&format!(
            r#"
            model = "gpt-4o"
            password = "{token}"

            [database]
            password = "{token}"
            hosts = ["one", "{token}"]
            "#
        ))
        .unwrap()
    }

    #[test]
    fn every_sealed_string_is_decrypted_wherever_it_sits() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);

        let locations = unseal(&mut document, &key).unwrap();

        assert_eq!(locations.len(), 3);
        assert_eq!(document["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["hosts"][1].as_str(), Some("hunter2"));
    }

    #[test]
    fn plain_strings_are_left_alone() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);

        unseal(&mut document, &key).unwrap();

        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
        assert_eq!(document["database"]["hosts"][0].as_str(), Some("one"));
    }

    #[test]
    fn a_document_without_envelopes_seals_nothing() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();

        assert!(unseal(&mut document, &key).unwrap().is_empty());
    }

    #[test]
    fn the_wrong_key_names_the_field_it_could_not_decrypt() {
        let token = encrypt_value(&SecretValue::new("hunter2"), &MasterKey::generate()).unwrap();
        let mut document: Value =
            toml::from_str(&format!("[database]\npassword = \"{token}\"\n")).unwrap();

        let error = unseal(&mut document, &MasterKey::generate()).unwrap_err();

        assert!(
            matches!(&error, ConfigError::Decrypt { field, .. } if field == "database.password"),
            "{error:?}"
        );
    }

    #[test]
    fn an_index_names_the_element_it_could_not_decrypt() {
        let token = encrypt_value(&SecretValue::new("hunter2"), &MasterKey::generate()).unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\"]\n")).unwrap();

        let error = unseal(&mut document, &MasterKey::generate()).unwrap_err();

        assert!(
            matches!(&error, ConfigError::Decrypt { field, .. } if field == "hosts[1]"),
            "{error:?}"
        );
    }

    #[test]
    fn resealing_restores_every_location_that_arrived_sealed() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let locations = unseal(&mut document, &key).unwrap();

        seal(&mut document, &locations, &key).unwrap();

        assert!(is_encrypted(document["password"].as_str().unwrap()));
        assert!(is_encrypted(
            document["database"]["hosts"][1].as_str().unwrap()
        ));
        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
        assert_eq!(document["database"]["hosts"][0].as_str(), Some("one"));
    }

    #[test]
    fn resealing_survives_a_field_that_has_since_been_removed() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();
        let locations = vec![vec![Segment::Key("password".to_string())]];

        seal(&mut document, &locations, &key).unwrap();

        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
    }

    #[test]
    fn an_already_sealed_value_is_not_sealed_twice() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let envelope = document["password"].as_str().unwrap().to_string();
        let locations = vec![vec![Segment::Key("password".to_string())]];

        seal(&mut document, &locations, &key).unwrap();

        assert_eq!(document["password"].as_str(), Some(envelope.as_str()));
    }

    #[test]
    fn a_location_reads_as_a_dotted_path() {
        assert_eq!(describe(&[]), "");
        assert_eq!(
            describe(&[
                Segment::Key("database".to_string()),
                Segment::Key("password".to_string())
            ]),
            "database.password"
        );
        assert_eq!(
            describe(&[Segment::Key("hosts".to_string()), Segment::Index(2)]),
            "hosts[2]"
        );
    }
}
