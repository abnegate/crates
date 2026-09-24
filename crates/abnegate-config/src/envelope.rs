mod location;
mod sealed;
mod segment;

use abnegate_secret::MasterKey;
use abnegate_secret::SecretValue;
use abnegate_secret::decrypt_value;
use abnegate_secret::encrypt_value;
use abnegate_secret::is_encrypted;
use toml::Value;

use crate::error::Error;

pub(crate) use crate::envelope::location::Location;
pub(crate) use crate::envelope::sealed::Sealed;
pub(crate) use crate::envelope::segment::Segment;

/// Find every closed `ENC[v<digits>:...]` envelope in `document`, decrypting
/// each in place when `key` is given and leaving it as it is when not.
///
/// An envelope of any version counts as sealed. One this release cannot open
/// fails with [`Error::Decrypt`] whose source is
/// [`abnegate_secret::Error::UnsupportedVersion`] when `key` is given, and is
/// kept as it was written when not.
pub(crate) fn unseal(document: &mut Value, key: Option<&MasterKey>) -> Result<Vec<Sealed>, Error> {
    let mut received = Vec::new();

    visit(document, &mut Location::default(), &mut |text, location| {
        if is_encrypted(text) {
            received.push((location.clone(), decrypt(text, location, key)?));
        }
        Ok(())
    })?;

    Ok(received
        .into_iter()
        .map(|(location, value)| Sealed::new(location, value, document))
        .collect())
}

/// Seal every string that holds a value which arrived sealed, wherever it now
/// sits, and every location such a value was edited in.
///
/// Without a key, a location that still holds its envelope is left alone and
/// one that would be written in the clear fails with
/// [`Error::SealedWithoutKey`]. A value that has gone from its location
/// fails with [`Error::SealedShapeChanged`] when fewer strings in the
/// document hold it than did on load, or when it was empty, since it cannot be
/// told apart from one that moved to a new key and was edited.
pub(crate) fn seal(
    document: &mut Value,
    sealed: &[Sealed],
    key: Option<&MasterKey>,
) -> Result<(), Error> {
    if let Some(lost) = sealed.iter().find(|value| value.is_lost(document)) {
        return Err(Error::SealedShapeChanged {
            field: lost.location().to_string(),
        });
    }

    let targets: Vec<Location> = sealed
        .iter()
        .flat_map(|value| value.targets(document))
        .collect();

    visit(document, &mut Location::default(), &mut |text, location| {
        if !is_encrypted(text) && sealed.iter().any(|value| value.matches(text)) {
            *text = encrypt(text, location, key)?;
        }
        Ok(())
    })?;

    for target in &targets {
        seal_at(document, target, key)?;
    }

    Ok(())
}

fn seal_at(
    document: &mut Value,
    location: &Location,
    key: Option<&MasterKey>,
) -> Result<(), Error> {
    match location.resolve_mut(document) {
        None => Ok(()),
        Some(Value::String(text)) if is_encrypted(text) => Ok(()),
        Some(Value::String(text)) => {
            *text = encrypt(text, location, key)?;
            Ok(())
        }
        Some(_) => Err(Error::SealedShapeChanged {
            field: location.to_string(),
        }),
    }
}

fn decrypt(
    text: &mut String,
    location: &Location,
    key: Option<&MasterKey>,
) -> Result<SecretValue, Error> {
    let Some(key) = key else {
        return Ok(SecretValue::new(text.as_str()));
    };

    let plaintext = decrypt_value(text, key).map_err(|source| Error::Decrypt {
        field: location.to_string(),
        source,
    })?;
    *text = plaintext.expose().to_string();

    Ok(plaintext)
}

fn encrypt(text: &str, location: &Location, key: Option<&MasterKey>) -> Result<String, Error> {
    let Some(key) = key else {
        return Err(Error::SealedWithoutKey {
            field: location.to_string(),
        });
    };

    encrypt_value(&SecretValue::new(text), key).map_err(|source| Error::Encrypt {
        field: location.to_string(),
        source,
    })
}

fn visit<Visitor>(
    value: &mut Value,
    location: &mut Location,
    visitor: &mut Visitor,
) -> Result<(), Error>
where
    Visitor: FnMut(&mut String, &Location) -> Result<(), Error>,
{
    match value {
        Value::String(text) => visitor(text, location)?,
        Value::Table(table) => {
            for (name, child) in table.iter_mut() {
                location.push(Segment::Key(name.clone()));
                visit(child, location, visitor)?;
                location.pop();
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter_mut().enumerate() {
                location.push(Segment::Index(index));
                visit(child, location, visitor)?;
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

    fn loaded(location: Location, content: &str) -> Sealed {
        Sealed::new(
            location,
            SecretValue::new("hunter2"),
            &toml::from_str(content).unwrap(),
        )
    }

    fn is_sealed(value: &Value) -> bool {
        value.as_str().is_some_and(is_encrypted)
    }

    fn opened(value: &Value, key: &MasterKey) -> String {
        assert!(is_sealed(value), "{value} was written in the clear");
        decrypt_value(value.as_str().unwrap(), key)
            .unwrap()
            .expose()
            .to_string()
    }

    fn unsealed(content: &str, key: Option<&MasterKey>) -> Vec<Sealed> {
        unseal(&mut toml::from_str(content).unwrap(), key).unwrap()
    }

    fn shape_changed_at(error: &Error, expected: &str) -> bool {
        matches!(error, Error::SealedShapeChanged { field } if field == expected)
    }

    #[test]
    fn every_sealed_string_is_decrypted_wherever_it_sits() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);

        let sealed = unseal(&mut document, Some(&key)).unwrap();

        assert_eq!(sealed.len(), 3);
        assert_eq!(document["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["password"].as_str(), Some("hunter2"));
        assert_eq!(document["database"]["hosts"][1].as_str(), Some("hunter2"));
    }

    #[test]
    fn plain_strings_are_left_alone() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);

        unseal(&mut document, Some(&key)).unwrap();

        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
        assert_eq!(document["database"]["hosts"][0].as_str(), Some("one"));
    }

    #[test]
    fn without_a_key_envelopes_are_found_and_left_sealed() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);
        let original = document.clone();

        let sealed = unseal(&mut document, None).unwrap();

        assert_eq!(sealed.len(), 3);
        assert_eq!(document, original);
    }

    #[test]
    fn a_document_without_envelopes_seals_nothing() {
        let key = MasterKey::generate().unwrap();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();

        assert!(unseal(&mut document, Some(&key)).unwrap().is_empty());
    }

    #[test]
    fn the_wrong_key_names_the_field_it_could_not_decrypt() {
        let token = encrypt_value(
            &SecretValue::new("hunter2"),
            &MasterKey::generate().unwrap(),
        )
        .unwrap();
        let mut document: Value =
            toml::from_str(&format!("[database]\npassword = \"{token}\"\n")).unwrap();

        let error = unseal(&mut document, Some(&MasterKey::generate().unwrap())).unwrap_err();

        assert!(
            matches!(&error, Error::Decrypt { field, .. } if field == "database.password"),
            "{error:?}"
        );
    }

    #[test]
    fn an_index_names_the_element_it_could_not_decrypt() {
        let token = encrypt_value(
            &SecretValue::new("hunter2"),
            &MasterKey::generate().unwrap(),
        )
        .unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\"]\n")).unwrap();

        let error = unseal(&mut document, Some(&MasterKey::generate().unwrap())).unwrap_err();

        assert!(
            matches!(&error, Error::Decrypt { field, .. } if field == "hosts[1]"),
            "{error:?}"
        );
    }

    #[test]
    fn resealing_restores_every_location_that_arrived_sealed() {
        let key = MasterKey::generate().unwrap();
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
    fn a_sealed_field_that_has_gone_with_its_value_is_refused() {
        let key = MasterKey::generate().unwrap();
        let mut document: Value = toml::from_str("model = \"gpt-4o\"").unwrap();
        let sealed = [loaded(password(), "password = \"hunter2\"")];

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "password"),
            "{error:?}"
        );
    }

    #[test]
    fn a_value_read_under_a_new_key_is_sealed_under_that_key() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("model = \"gpt-4o\"\napi_key = \"{token}\"\n"),
            Some(&key),
        );
        let mut document: Value =
            toml::from_str("model = \"gpt-4o\"\ntoken = \"hunter2\"\n").unwrap();

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert!(is_sealed(&document["token"]), "{document}");
        assert_eq!(opened(&document["token"], &key), "hunter2");
        assert_eq!(document["model"].as_str(), Some("gpt-4o"));
    }

    #[test]
    fn a_value_under_a_renamed_map_key_is_sealed_under_the_new_one() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("[profiles.default]\ntoken = \"{token}\"\n"),
            Some(&key),
        );
        let mut document: Value = toml::from_str("[profiles.work]\ntoken = \"hunter2\"\n").unwrap();

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert!(
            is_sealed(&document["profiles"]["work"]["token"]),
            "{document}"
        );
    }

    #[test]
    fn every_plain_copy_of_a_sealed_value_is_sealed() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(&format!("password = \"{token}\"\n"), Some(&key));
        let mut document: Value =
            toml::from_str("password = \"hunter2\"\nbackup = \"hunter2\"\nhint = \"hunter\"\n")
                .unwrap();

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert!(is_sealed(&document["password"]), "{document}");
        assert!(is_sealed(&document["backup"]), "{document}");
        assert_eq!(document["hint"].as_str(), Some("hunter"));
    }

    #[test]
    fn a_value_that_moved_to_a_new_key_and_was_edited_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(&format!("api_key = \"{token}\"\n"), Some(&key));
        let mut document: Value = toml::from_str("token = \"correct-horse\"\n").unwrap();

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "api_key"),
            "{error:?}"
        );
        assert_eq!(document["token"].as_str(), Some("correct-horse"));
    }

    #[test]
    fn a_value_renamed_and_edited_beside_a_sealed_copy_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("api_key = \"{token}\"\nbackup = \"{token}\"\n"),
            Some(&key),
        );
        let mut document: Value =
            toml::from_str("token = \"correct-horse\"\nbackup = \"hunter2\"\n").unwrap();

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(shape_changed_at(&error, "api_key"), "{error:?}");
    }

    #[test]
    fn a_value_renamed_and_edited_beside_a_plain_copy_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("api_key = \"{token}\"\nhint = \"hunter2\"\n"),
            Some(&key),
        );
        let mut document: Value =
            toml::from_str("token = \"correct-horse\"\nhint = \"hunter2\"\n").unwrap();

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(shape_changed_at(&error, "api_key"), "{error:?}");
    }

    #[test]
    fn without_a_key_a_value_renamed_and_edited_beside_its_envelope_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("api_key = \"{token}\"\nbackup = \"{token}\"\n"),
            None,
        );
        let mut document: Value = toml::from_str(&format!(
            "token = \"correct-horse\"\nbackup = \"{token}\"\n"
        ))
        .unwrap();

        let error = seal(&mut document, &sealed, None).unwrap_err();

        assert!(shape_changed_at(&error, "api_key"), "{error:?}");
    }

    #[test]
    fn a_value_renamed_beside_a_sealed_copy_is_sealed_in_both_places() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(
            &format!("api_key = \"{token}\"\nbackup = \"{token}\"\n"),
            Some(&key),
        );
        let mut document: Value =
            toml::from_str("token = \"hunter2\"\nbackup = \"hunter2\"\n").unwrap();

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(opened(&document["token"], &key), "hunter2");
        assert_eq!(opened(&document["backup"], &key), "hunter2");
    }

    #[test]
    fn an_empty_value_renamed_and_edited_beside_an_empty_string_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new(""), &key).unwrap();
        let sealed = unsealed(
            &format!("api_key = \"{token}\"\nproxy = \"\"\n"),
            Some(&key),
        );
        let mut document: Value =
            toml::from_str("token = \"correct-horse\"\nproxy = \"\"\n").unwrap();

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(shape_changed_at(&error, "api_key"), "{error:?}");
    }

    #[test]
    fn an_empty_value_that_gained_an_empty_neighbour_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new(""), &key).unwrap();
        let sealed = unsealed(&format!("api_key = \"{token}\"\n"), Some(&key));
        let mut document: Value =
            toml::from_str("token = \"correct-horse\"\nproxy = \"\"\n").unwrap();

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(shape_changed_at(&error, "api_key"), "{error:?}");
    }

    #[test]
    fn an_empty_value_is_sealed_where_it_sits_and_nowhere_else() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new(""), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("password = \"{token}\"\nproxy = \"\"\n")).unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["password"] = Value::String("correct-horse".to_string());
        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(opened(&document["password"], &key), "correct-horse");
        assert_eq!(document["proxy"].as_str(), Some(""));
    }

    #[test]
    fn without_a_key_an_envelope_under_a_new_key_is_written_as_it_was() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let sealed = unsealed(&format!("api_key = \"{token}\"\n"), None);
        let mut document: Value = toml::from_str(&format!("token = \"{token}\"\n")).unwrap();

        seal(&mut document, &sealed, None).unwrap();

        assert_eq!(document["token"].as_str(), Some(token.as_str()));
    }

    #[test]
    fn rotating_one_of_two_entries_that_shared_a_secret_keeps_both_sealed() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value = toml::from_str(&format!(
            "[[servers]]\npassword = \"{token}\"\n\n[[servers]]\npassword = \"{token}\"\n"
        ))
        .unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["servers"][1]["password"] = Value::String("correct-horse".to_string());
        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(opened(&document["servers"][0]["password"], &key), "hunter2");
        assert_eq!(
            opened(&document["servers"][1]["password"], &key),
            "correct-horse"
        );
    }

    #[test]
    fn an_already_sealed_value_is_not_sealed_twice() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);
        let envelope = document["password"].as_str().unwrap().to_string();
        let sealed = [loaded(password(), "password = \"hunter2\"")];

        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(document["password"].as_str(), Some(envelope.as_str()));
    }

    #[test]
    fn a_shifted_array_seals_the_secret_and_not_its_new_neighbour() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\", \"three\"]\n")).unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["hosts"].as_array_mut().unwrap().remove(0);
        seal(&mut document, &sealed, Some(&key)).unwrap();

        let hosts = document["hosts"].as_array().unwrap();
        assert!(is_sealed(&hosts[0]), "{hosts:?}");
        assert_eq!(hosts[1].as_str(), Some("three"));
        assert_eq!(opened(&hosts[0], &key), "hunter2");
    }

    #[test]
    fn a_shifted_array_of_tables_follows_the_secret() {
        let key = MasterKey::generate().unwrap();
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
    fn an_element_inserted_before_a_secret_is_sealed_with_it() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\"]\n")).unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["hosts"]
            .as_array_mut()
            .unwrap()
            .insert(0, Value::String("zero".to_string()));
        seal(&mut document, &sealed, Some(&key)).unwrap();

        let hosts: Vec<String> = document["hosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|host| opened(host, &key))
            .collect();
        assert_eq!(hosts, ["zero", "one", "hunter2"]);
    }

    #[test]
    fn a_copied_entry_whose_original_was_rotated_keeps_both_sealed() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("[[servers]]\npassword = \"{token}\"\n")).unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        let copy = document["servers"][0].clone();
        document["servers"].as_array_mut().unwrap().push(copy);
        document["servers"][0]["password"] = Value::String("correct-horse".to_string());
        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(
            opened(&document["servers"][0]["password"], &key),
            "correct-horse"
        );
        assert_eq!(opened(&document["servers"][1]["password"], &key), "hunter2");
    }

    #[test]
    fn a_secret_rotated_while_a_neighbour_took_its_old_value_is_sealed() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value = toml::from_str(&format!(
            "[[servers]]\npassword = \"first\"\n\n[[servers]]\npassword = \"{token}\"\n\n[[servers]]\npassword = \"third\"\n"
        ))
        .unwrap();
        let sealed = unseal(&mut document, Some(&key)).unwrap();

        document["servers"].as_array_mut().unwrap().remove(2);
        document["servers"][0]["password"] = Value::String("hunter2".to_string());
        document["servers"][1]["password"] = Value::String("correct-horse".to_string());
        seal(&mut document, &sealed, Some(&key)).unwrap();

        assert_eq!(opened(&document["servers"][0]["password"], &key), "hunter2");
        assert_eq!(
            opened(&document["servers"][1]["password"], &key),
            "correct-horse"
        );
    }

    #[test]
    fn without_a_key_growing_an_array_that_holds_an_envelope_is_refused() {
        let key = MasterKey::generate().unwrap();
        let token = encrypt_value(&SecretValue::new("hunter2"), &key).unwrap();
        let mut document: Value =
            toml::from_str(&format!("hosts = [\"one\", \"{token}\"]\n")).unwrap();
        let sealed = unseal(&mut document, None).unwrap();

        document["hosts"]
            .as_array_mut()
            .unwrap()
            .insert(0, Value::String("zero".to_string()));
        let error = seal(&mut document, &sealed, None).unwrap_err();

        assert!(
            matches!(&error, Error::SealedWithoutKey { field } if field == "hosts[0]"),
            "{error:?}"
        );
    }

    #[test]
    fn a_sealed_field_that_is_no_longer_a_string_is_refused() {
        let key = MasterKey::generate().unwrap();
        let mut document: Value = toml::from_str("[password]\nvalue = \"hunter2\"\n").unwrap();
        let sealed = [loaded(password(), "password = \"hunter2\"")];

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(&error, Error::SealedShapeChanged { field } if field == "password"),
            "{error:?}"
        );
    }

    #[test]
    fn a_lost_array_secret_whose_key_path_holds_other_values_is_refused() {
        let key = MasterKey::generate().unwrap();
        let mut document: Value = toml::from_str("hosts = [1, 2]").unwrap();
        let sealed = [loaded(
            Location::from(vec![Segment::Key("hosts".to_string()), Segment::Index(0)]),
            "hosts = [\"hunter2\", \"one\"]",
        )];

        let error = seal(&mut document, &sealed, Some(&key)).unwrap_err();

        assert!(
            matches!(error, Error::SealedShapeChanged { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn without_a_key_an_untouched_envelope_is_written_as_it_was() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);
        let original = document.clone();
        let sealed = unseal(&mut document, None).unwrap();

        seal(&mut document, &sealed, None).unwrap();

        assert_eq!(document, original);
    }

    #[test]
    fn without_a_key_a_replaced_envelope_is_refused() {
        let key = MasterKey::generate().unwrap();
        let mut document = sealed_document(&key);
        let sealed = unseal(&mut document, None).unwrap();

        document["database"]["password"] = Value::String("plaintext".to_string());
        let error = seal(&mut document, &sealed, None).unwrap_err();

        assert!(
            matches!(&error, Error::SealedWithoutKey { field } if field == "database.password"),
            "{error:?}"
        );
    }
}
