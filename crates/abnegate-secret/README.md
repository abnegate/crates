# abnegate-secret

Credential handling. `SecretValue` holds a credential without letting it reach a
log line by accident: it zeroizes on drop and redacts itself in `Debug`.
`encrypt_value` wraps one in an AES-256-GCM `ENC[v1:...]` envelope for storage,
`redact` scrubs credentials out of text on its way back to a model, and
`sanitize` does the same to text that has been through a terminal.

## Features

- `sqlx`: `Encode`, `Decode` and `Type` for `SecretValue` over every database that stores a `String`.
- `rusqlite`: `ToSql` and `FromSql` for `SecretValue`, the same for SQLite.

Neither feature chooses a driver, runtime, TLS stack or SQLite build; enable
those on your own `sqlx` or `rusqlite` dependency.

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
