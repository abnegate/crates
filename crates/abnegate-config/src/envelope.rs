mod location;
mod sealed;
mod segment;

use abnegate_secret::MasterKey;
use abnegate_secret::SecretValue;
use abnegate_secret::decrypt_value;
use abnegate_secret::encrypt_value;
use abnegate_secret::is_encrypted;
use toml::Value;

use crate::error::ConfigError;

pub(crate) use crate::envelope::location::Location;
pub(crate) use crate::envelope::sealed::Sealed;
pub(crate) use crate::envelope::segment::Segment;

/// Find every `ENC[v1:...]` envelope in `document`, decrypting each in place
/// when `key` is given and leaving it as it is when not.
pub(crate) fn unseal(
    document: &mut Value,
    key: Option<&MasterKey>,
) -> Result<Vec<Sealed>, ConfigError> {
    let mut sealed = Vec::new();
    walk(document, key, &mut Location::default(), &mut sealed)?;
    Ok(sealed)
}

/// Seal every location that holds a value which arrived sealed.
///
/// Without a key, a location that still holds its envelope is left alone and
/// one that would be written in the clear fails with
/// [`ConfigError::SealedWithoutKey`].
pub(crate) fn seal(
    document: &mut Value,
    sealed: &[Sealed],
    key: Option<&MasterKey>,
) -> Result<(), ConfigError> {
    let targets: Vec<Location> = sealed
        .iter()
        .flat_map(|value| value.targets(document))
        .collect();

    for target in &targets {
        seal_at(document, target, key)?;
    }

    Ok(())
}

fn seal_at(
    document: &mut Value,
    location: &Location,
    key: Option<&MasterKey>,
) -> Result<(), ConfigError> {
    let text = match location.resolve_mut(document) {
        None => return Ok(()),
        Some(Value::String(text)) => text,
        Some(_) => {
            return Err(ConfigError::SealedShapeChanged {
                field: location.to_string(),
            });
        }
    };

    if is_encrypted(text) {
        return Ok(());
    }

    let Some(key) = key else {
        return Err(ConfigError::SealedWithoutKey {
            field: location.to_string(),
        });
    };

    *text = encrypt_value(&SecretValue::new(text.as_str()), key).map_err(|source| {
        ConfigError::Encrypt {
            field: location.to_string(),
            source,
        }
    })?;

    Ok(())
}

fn walk(
    value: &mut Value,
    key: Option<&MasterKey>,
    location: &mut Location,
    sealed: &mut Vec<Sealed>,
) -> Result<(), ConfigError> {
    match value {
        Value::String(text) if is_encrypted(text) => {
            let received = match key {
                Some(key) => {
                    let plaintext =
                        decrypt_value(text, key).map_err(|source| ConfigError::Decrypt {
                            field: location.to_string(),
                            source,
                        })?;
                    *text = plaintext.expose().to_string();
                    plaintext
                }
                None => SecretValue::new(text.as_str()),
            };
            sealed.push(Sealed::new(location.clone(), received));
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

    fn password() -> Location {
        Location::from(vec![Segment::Key("password".to_string())])
    }

    fn is_sealed(value: &Value) -> bool {
        value.as_str().is_some_and(is_encrypted)
    }

    #[test]
    fn every_sealed_string_is_decrypted_wherever_it_sits() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);

        let sealed = unseal(&mut document, Some(&key)).unwrap();

        assert_eq!(sealed.len(), 3);
        assert_eq!(document["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["hosts"][1].as_str(), Some("hunter2"));
    }

    #[test]
    fn plain_strings_are_left_alone() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);

        unseal(&mut document, Some(&key)).unwrap();

        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
        assert_eq!(document["database"]["hosts"][0].as_str(), Some("one"));
    }

    #[test]
    fn without_a_key_envelopes_are_found_and_left_sealed() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let original = document.clone();

        let sealed = unseal(&mut document, None).unwrap();

        assert_eq!(sealed.len(), 3);
        assert_eq!(document, original);
    }

    #[test]
    fn a_document_without_envelopes_seals_nothing() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();

        assert!(unseal(&mut document, Some(&key)).unwrap().is_empty());
    }

    #[test]
    fn the_wrong_key_names_the_field_it_could_not_decrypt() {
        let token = encrypt_value(&SecretValue::new("hunter2"), &MasterKey::generate()).unwrap();
        let mut document: Value =
            toml::from_str(&format!("[database]\npassword = \"{token}\"\n")).unwrap();

        let error = unseal(&mut document, Some(&MasterKey::generate())).unwrap_err();

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

        let error = unseal(&mut document, Some(&MasterKey::generate())).unwrap_err();

        assert!(
            matches!(&error, ConfigError::Decrypt { field, .. } if field == "hosts[1]"),
            "{error:?}"
        );
    }

    #[test]
    fn resealing_restores_every_location_that_arrived_sealed() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert!(is_sealed(&document["password"]));
        assert!(is_sealed(&document["database"]["password"]));
        assert!(is_sealed(&document["database"]["hosts"][1]));
        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
        assert_eq!(document["database"]["hosts"][0].as_str(), Some("one"));
    }

    #[test]
    fn resealing_survives_a_field_that_has_since_been_removed() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();
        let sealed = [Sealed::new(password(), SecretValue::new("hunter2"))];

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
    }

    #[test]
    fn an_already_sealed_value_is_not_sealed_twice() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let envelope = document["password"].as_str().unwrap().to_string();
        let sealed = [Sealed::new(password(), SecretValue::new("hunter2"))];

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(document["password"].as_str(), Some(envelope.as_str()));
    }

    #[test]
    fn a_shifted_array_seals_the_secret_and_not_its_new_neighbour() {
        let key = MasterKey::generate();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\", \"three\"]\n")).unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["hosts"].as_array_mut().unwrap().remove(0);
        seal(&mut document, &sealed, Some(&key)).unwrap();

        let hosts = document["hosts"].as_array().unwrap();
        assert!(is_sealed(&hosts[0]), "{hosts:?}");
        assert_eq!(hosts[1].as_str(), Some("three"));
        assert_eq!(
            decrypt_value(hosts[0].as_str().unwrap(), &key)
                .unwrap()
                .expose(),
            "hunter2"
        );
    }

    #[test]
    fn a_shifted_array_of_tables_follows_the_secret() {
        let key = MasterKey::generate();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value = toml::from_str(&format!(
            "[[servers]]\npassword = \"plain\"\n\n[[servers]]\npassword = \"{token}\"\n"
        ))
        .unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["servers"].as_array_mut().unwrap().remove(0);
        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert!(is_sealed(&document["servers"][0]["password"]));
    }

    #[test]
    fn a_sealed_field_that_is_no_longer_a_string_is_refused() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("[password]\nvalue = \"hunter2\"\n").unwrap();
        let sealed = [Sealed::new(password(), SecretValue::new("hunter2"))];

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(&error, ConfigError::SealedShapeChanged { field } if field == "password"),
            "{error:?}"
        );
    }

    #[test]
    fn a_lost_array_secret_whose_key_path_holds_other_values_is_refused() {
        let key = MasterKey::generate();
        let mut document: Value = toml::from_str("hosts = [1, 2]").unwrap();
        let sealed = [Sealed::new(
            Location::from(vec![Segment::Key("hosts".to_string()), Segment::Index(0)]),
            SecretValue::new("hunter2"),
        )];

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(error, ConfigError::SealedShapeChanged { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn without_a_key_an_untouched_envelope_is_written_as_it_was() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let original = document.clone();
        let sealed = unseal(&mut document, None).unwrap();

        seal(&mut document, &sealed, None).unwrap();

        assert_eq!(document, original);
    }

    #[test]
    fn without_a_key_a_replaced_envelope_is_refused() {
        let key = MasterKey::generate();
        let mut document = sealed_document(&key);
        let sealed = unseal(&mut document, None).unwrap();

        document["database"]["password"] = Value::String("plaintext".to_string());
        let error = seal(&mut document, &sealed, None).unwrap_err();

        assert!(
            matches!(&error, ConfigError::SealedWithoutKey { field } if field == "database.password"),
            "{error:?}"
        );
    }
}
