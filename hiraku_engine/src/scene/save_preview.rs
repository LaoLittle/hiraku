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
    let Some(slot) = path.strip_prefix("save-thumbnail://") else {
        return server.load(path.to_owned());
    };
    let decoded = crate::storage::load_save_data(slot)
        .map_err(|error| error.to_string())
        .and_then(|data| {
            image::load_from_memory_with_format(&data.thumbnail_png, image::ImageFormat::Png)
                .map_err(|error| error.to_string())
        });
    match decoded {
        Ok(image) => server.add(Image::from_dynamic(image, true, default())),
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
    fn thumbnail_is_bounded_and_png_encoded() {
        let source =
            Image::from_dynamic(image::DynamicImage::new_rgb8(1920, 1080), true, default());
        let png = encode(&source).expect("encode thumbnail");
        let image = image::load_from_memory(&png).expect("decode thumbnail");
        assert_eq!((image.width(), image.height()), (480, 270));
    }
}
