# abnegate-config

Configuration for command line applications. `Config` is the application's own
settings type loaded from a TOML file, `Loader` chooses where that file lives and
which `MasterKey` unseals it, `config_path` derives the conventional location
from an `Application` name, `EnvironmentFile` upserts keys in a `.env` file, and
`TokenStore` keeps credentials in the platform keyring. Any string in the file
written as an `ENC[v1:...]` envelope is decrypted on load when the loader is
given a master key, and `Config::save` seals those values again wherever they
now appear, refusing rather than write one in the clear. Every file this crate
writes is replaced atomically and is readable only by its owner.

## Features

- `keyring`: `TokenStore` and `TokenMetadata`, a credential store backed by Keychain Services, the Windows Credential Manager, or the Secret Service.

## Usage

```sh
cargo add abnegate-config
cargo add serde --features derive
```

```rust,no_run
use abnegate_config::{Application, ConfigError, Loader};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
struct Settings {
    model: String,
    host: Option<String>,
}

fn main() -> Result<(), ConfigError> {
    let loader = Loader::new(&Application::new("example-cli")?)?;
    let mut config = loader.load_or_default::<Settings>()?;
    config.value_mut().model = "gpt-4o".to_string();
    config.save()
}
```
