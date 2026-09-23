use abnegate_comfy::Config;
use abnegate_comfy::client::{Client, Error, SourceImage, SourceVideo};

#[test]
fn empty_sources_are_rejected_before_upload() {
    assert!(matches!(
        SourceImage::new(Vec::new(), "image/png"),
        Err(Error::Configuration("source image is empty or too large"))
    ));
    assert!(matches!(
        SourceVideo::new(Vec::new(), "video/webm"),
        Err(Error::Configuration("source video is empty or too large"))
    ));
}

#[test]
fn source_types_are_canonicalized_and_unknown_types_are_rejected() {
    let image = SourceImage::new(vec![1], " IMAGE/WEBP ").unwrap();
    assert_eq!(image.mime, "image/webp");
    assert!(image.filename.ends_with(".webp"));

    let video = SourceVideo::new(vec![1], " VIDEO/MP4 ").unwrap();
    assert_eq!(video.mime, "video/mp4");
    assert!(video.filename.ends_with(".mp4"));

    assert!(matches!(
        SourceImage::new(vec![1], "image/gif"),
        Err(Error::Configuration("source image type is not supported"))
    ));
    assert!(matches!(
        SourceVideo::new(vec![1], "video/quicktime"),
        Err(Error::Configuration("source video type is not supported"))
    ));
}

#[test]
fn invalid_client_configuration_is_rejected_before_catalog_or_network_access() {
    assert!(matches!(
        Client::new(Config {
            base_url: "   ".to_string(),
            ..Config::default()
        }),
        Err(Error::Configuration("COMFYUI_BASE_URL is empty"))
    ));
    assert!(matches!(
        Client::new(Config {
            checkpoint: "../model.safetensors".to_string(),
            ..Config::default()
        }),
        Err(Error::Configuration(
            "COMFYUI_CHECKPOINT must be a checkpoint filename"
        ))
    ));
}
