# abnegate-config

Configuration for command line applications. `Config` is the application's own
settings type loaded from a TOML file, `Loader` chooses where that file lives and
which `MasterKey` unseals it, `abnegate_config::path` and
`abnegate_config::directory` derive the conventional locations from an
`Application` name, `EnvironmentFile` upserts keys in a `.env` file, and
`TokenStore` keeps credentials in the platform keyring. Any string in the file
written as an `ENC[v1:...]` envelope is decrypted on load when the loader is
given a master key, and `Config::save` seals those values again wherever they
now appear, refusing rather than write one in the clear. Every file this crate
writes is replaced atomically and is readable only by its owner.

## Locations

An `Application` keeps its files in the hidden directory `.{name}` under the
home directory, and its configuration in `config.toml` inside it.
Two environment variables override the location, where `<APPLICATION>` is the
name in upper case with every `-` replaced by `_`:

- `<APPLICATION>_CONFIG_DIRECTORY` moves the directory, and the file with it.
- `<APPLICATION>_CONFIG_PATH` names the file outright.

## Features

- `keyring`: `TokenStore` and `TokenMetadata`, a credential store backed by Keychain Services, the Windows Credential Manager, or the Secret Service.

## Usage

```sh
cargo add abnegate-config
cargo add serde --features derive
```

```rust,no_run
use abnegate_config::Application;
use abnegate_config::Error;
use abnegate_config::Loader;
use serde::Deserialize;
use serde::Serialize;

#[derive(Default, Deserialize, Serialize)]
struct Settings {
    model: String,
    host: Option<String>,
}

fn main() -> Result<(), Error> {
    let loader = Loader::new(&Application::new("example-cli")?)?;
    let mut config = loader.load_or_default::<Settings>()?;
    config.value_mut().model = "gpt-4o".to_string();
    config.save()
}
```
