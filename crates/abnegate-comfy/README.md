# abnegate-comfy

ComfyUI integration: image, video, and audio generation, upscaling, model
inventory, and LoRA training. The crate talks to a ComfyUI server over HTTP and
owns nothing else, with no web framework, database, or application state, so it
can be dropped into any project that needs image generation or wants to train a
LoRA. Training writes the dataset, captions any image left blank, runs the
packaged training graph on the configured server, then scores every checkpoint
the run produced and keeps the best one, and `video::extract` turns a clip into
that image set. The training graphs call custom nodes that ComfyUI does not ship;
`Config::contract` names them, and its defaults match the node pack this crate
was written against. A host that collects metrics installs `observe_requests`
once at startup; without it the crate records nothing.

## Features

- `saliency`: frames training crops on the subject U2-Net finds, through `abnegate-vision` on ONNX Runtime, when `Config::vision_model` points at the weights; without it a photo is cropped on its centre and a video frame on whatever moved.

## Usage

```sh
cargo add abnegate-comfy
cargo add tokio --features sync
```

```rust,no_run
use abnegate_comfy::{Client, Config, Error};
use tokio::sync::{broadcast, mpsc};

async fn lighthouse() -> Result<(), Error> {
    let client = Client::new(Config::from_env())?;
    let (_stop, mut cancel) = broadcast::channel(1);
    let (progress, _updates) = mpsc::unbounded_channel();

    let images = client
        .generate("a lighthouse in a storm", None, &mut cancel, progress)
        .await?;
    println!("{} images", images.len());
    Ok(())
}
```
