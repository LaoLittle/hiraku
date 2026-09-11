//! Read back the engine canvas before a modal covers it. No window or filesystem API.
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};

#[derive(Resource, Default)]
pub struct SavePreview {
    pub png: Vec<u8>,
    pub capture: Option<Entity>,
    pub waiting_frames: u32,
}

pub fn capture(commands: &mut Commands, canvas: &crate::HirakuCanvas, preview: &mut SavePreview) {
    preview.png.clear();
    preview.waiting_frames = 0;
    let entity = commands
        .spawn(Screenshot::image(canvas.image.clone()))
        .observe(
            |event: On<ScreenshotCaptured>, mut preview: ResMut<SavePreview>| {
                if preview.capture != Some(event.entity) {
                    return;
                }
                preview.capture = None;
                match encode(&event.image) {
                    Ok(png) => preview.png = png,
                    Err(error) => warn!("failed to capture save thumbnail: {error}"),
                }
            },
        )
        .id();
    preview.capture = Some(entity);
}

fn encode(image: &Image) -> Result<Vec<u8>, String> {
    let dynamic = image
        .clone()
        .try_into_dynamic()
        .map_err(|error| error.to_string())?;
    let resized = dynamic.thumbnail(480, 270).to_rgb8();
    let mut output = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(output.into_inner())
}

pub fn load_image(server: &AssetServer, path: &str) -> Handle<Image> {
    load_image_with(server, path, |slot| {
        crate::storage::load_save_thumbnail(slot).map_err(|error| error.to_string())
    })
}

fn load_image_with(
    server: &AssetServer,
    path: &str,
    read_preview: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> Handle<Image> {
    let Some(slot) = path.strip_prefix("save-thumbnail://") else {
        return crate::texture::load_static_image(server, path);
    };
    let decoded = read_preview(slot).and_then(|png| {
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .map_err(|error| error.to_string())
    });
    match decoded {
        Ok(image) => server.add(Image::from_dynamic(
            image,
            true,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        )),
        Err(error) => {
            warn!("failed to load thumbnail for `{slot}`: {error}");
            server.add(Image::from_dynamic(
                image::DynamicImage::new_rgba8(1, 1),
                true,
                default(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_png_returns_transparent_asset_without_panicking() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let server = app.world().resource::<AssetServer>();
        let handle = load_image_with(server, "save-thumbnail://bob", |_| Ok(vec![0xff]));
        assert!(handle.path().is_none());
        app.update();
        let assets = app.world().resource::<Assets<Image>>();
        let image = assets.get(&handle).expect("fallback asset");
        assert_eq!(image.data.as_deref(), Some(&[0, 0, 0, 0][..]));
    }
    #[test]
    fn thumbnail_is_an_in_memory_asset_not_a_named_asset_source() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let server = app.world().resource::<AssetServer>();
        let source = Image::from_dynamic(image::DynamicImage::new_rgb8(16, 9), true, default());
        let png = encode(&source).expect("encode synthetic preview");
        let mut read = false;
        let handle = load_image_with(server, "save-thumbnail://alice", |slot| {
            assert_eq!(slot, "alice");
            read = true;
            Ok(png)
        });
        assert!(read);
        assert!(
            handle.path().is_none(),
            "preview must not be sent to an asset source"
        );
    }
    #[test]
    fn thumbnail_is_bounded_and_png_encoded() {
        let source =
            Image::from_dynamic(image::DynamicImage::new_rgb8(1920, 1080), true, default());
        let png = encode(&source).expect("encode thumbnail");
        let image = image::load_from_memory(&png).expect("decode thumbnail");
        assert_eq!((image.width(), image.height()), (480, 270));
    }
}
