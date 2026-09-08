//! Pure-Rust runtime UASTC transcoding. No encoder or native C++ dependency.
use basisu::{DecodeFlags, SourceFormat, TargetFormat, Transcoder};
use bevy::{
    asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader},
    image::{CompressedImageFormatSupport, CompressedImageFormats},
    prelude::*,
    render::render_resource::{AstcBlock, AstcChannel, Extent3d, TextureDimension, TextureFormat},
};

pub struct UastcPlugin;
impl Plugin for UastcPlugin {
    fn build(&self, _app: &mut App) {}
    fn finish(&self, app: &mut App) {
        let formats = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .map_or(CompressedImageFormats::NONE, |support| support.0);
        app.register_asset_loader(UastcLoader { formats });
    }
}

#[derive(TypePath)]
struct UastcLoader {
    formats: CompressedImageFormats,
}

#[derive(Debug, thiserror::Error)]
pub enum UastcError {
    #[error("failed to read UASTC texture: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid UASTC KTX2 texture: {0}")]
    Invalid(String),
}
impl AssetLoader for UastcLoader {
    type Asset = Image;
    type Settings = ();
    type Error = UastcError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<Image, UastcError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        println!("loaded uastc");
        decode(&bytes, self.formats)
    }
    fn extensions(&self) -> &[&str] {
        &["uastc.ktx2"]
    }
}

/// Transcode a 2D texture to a format supported by the device.
pub fn decode(bytes: &[u8], formats: CompressedImageFormats) -> Result<Image, UastcError> {
    let invalid = |error: basisu::Error| UastcError::Invalid(format!("{error:?}"));
    if !bytes.starts_with(b"\xabKTX 20\xbb\r\n\x1a\n") {
        return Err(UastcError::Invalid("expected KTX2 container".into()));
    }
    let texture = Transcoder::new(bytes).map_err(invalid)?;
    if texture.source_format() != SourceFormat::UastcLdr
        || texture.layer_count() > 1
        || texture.face_count() != 1
        || texture.is_video()
    {
        return Err(UastcError::Invalid(
            "only single-layer 2D UASTC LDR textures are supported".into(),
        ));
    }
    let (width, height) = texture.base_dimensions();
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(UastcError::Invalid(
            "texture dimensions exceed 16384".into(),
        ));
    }
    let srgb = bytes
        .get(48..52)
        .and_then(|offset| <[u8; 4]>::try_from(offset).ok())
        .and_then(|offset| bytes.get(u32::from_le_bytes(offset) as usize + 14))
        .copied()
        == Some(2);
    let aligned = width % 4 == 0 && height % 4 == 0;
    let (target, format) = if aligned && formats.contains(CompressedImageFormats::ASTC_LDR) {
        (
            TargetFormat::Astc4x4Rgba,
            TextureFormat::Astc {
                block: AstcBlock::B4x4,
                channel: if srgb {
                    AstcChannel::UnormSrgb
                } else {
                    AstcChannel::Unorm
                },
            },
        )
    } else if aligned && formats.contains(CompressedImageFormats::BC) {
        (
            TargetFormat::Bc7Rgba,
            if srgb {
                TextureFormat::Bc7RgbaUnormSrgb
            } else {
                TextureFormat::Bc7RgbaUnorm
            },
        )
    } else {
        (
            TargetFormat::Rgba32,
            if srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            },
        )
    };
    let mut data = Vec::new();
    for level in 0..texture.level_count() {
        data.extend(
            texture
                .transcode(level, target, DecodeFlags::NONE)
                .map_err(invalid)?,
        );
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.mip_level_count = texture.level_count();
    image.data = Some(data);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_input_returns_error() {
        for bytes in [
            &[][..],
            &b"not a texture"[..],
            &b"\xabKTX 20\xbb\r\n\x1a\n"[..],
        ] {
            assert!(decode(bytes, CompressedImageFormats::NONE).is_err());
        }
    }
}
