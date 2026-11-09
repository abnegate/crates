# abnegate-secret

Credential handling. `SecretValue` holds a credential without letting it reach a
log line by accident: it zeroizes on drop and redacts itself in `Debug`.
`encrypt_value` wraps one in an AES-256-GCM `ENC[v1:...]` envelope for storage,
`redact` scrubs credentials out of text on its way back to a model,
`contains_credential` tells whether a value meant to be a plain name carries
one, `conceal` removes one known key however a provider echoes it back, and
`sanitize` scrubs text that has been through a terminal.

## Features

- `encryption`, on by default: `encrypt_value`, `decrypt_value`, `decrypt_legacy_value` for values sealed before the envelope existed, and the `MasterKey` they seal under, with the AES-256-GCM, random source and key file dependencies they need.
- `derivation`: `MasterKey::derive`, which stretches a passphrase into a key with Argon2id, and the `KeyDerivation` that names its domain. Implies `encryption`.
- `sqlx`: `Encode`, `Decode` and `Type` for `SecretValue` over every database that stores a `String`.

The `sqlx` feature implements sqlx 0.9's traits and chooses no driver, runtime
or TLS stack; enable those on your own `sqlx` 0.9 dependency. Another sqlx
minor is a separate crate and does not see these implementations. To only
redact and sanitize, turn default features off and skip the cipher:

```sh
cargo add abnegate-secret --no-default-features
```

## Usage

```sh
cargo add abnegate-secret
```

```rust
use abnegate_secret::{Error, MasterKey, SecretValue, decrypt_value, encrypt_value, redact};

fn main() -> Result<(), Error> {
    let key = MasterKey::generate()?;
    let password = SecretValue::new("hunter2");

    let stored = encrypt_value(&password, &key)?;
    assert_eq!(decrypt_value(&stored, &key)?, password);

    println!("{}", redact("login failed: password=hunter2"));
    Ok(())
}
```
