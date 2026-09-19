//! Per-character, transient packing of loose part images. Save state keeps
//! source parts, never generated atlas coordinates or GPU asset handles.
use super::source::AtlasSource;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::{image::TextureAtlasBuilder, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceId {
    Render(AssetId<Image>),
    Cpu(AssetId<AtlasSource>),
}
impl From<AssetId<Image>> for SourceId {
    fn from(id: AssetId<Image>) -> Self {
        Self::Render(id)
    }
}
impl SourceId {
    pub fn get<'a>(
        self,
        images: &'a Assets<Image>,
        sources: Option<&'a Assets<AtlasSource>>,
    ) -> Option<&'a Image> {
        match self {
            Self::Render(id) => images.get(id),
            Self::Cpu(id) => sources?.get(id).map(|s| &s.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Region {
    pub image: SourceId,
    pub rect: URect,
}

#[derive(Default)]
pub(super) struct PackedAtlas {
    image: Option<Handle<Image>>,
    regions: Vec<Region>,
    rects: Vec<URect>,
    dirty: bool,
}

impl PackedAtlas {
    pub fn invalidate(&mut self, changed: &[AssetId<Image>]) {
        self.dirty |= self
            .regions
            .iter()
            .any(|r| matches!(r.image, SourceId::Render(id) if changed.contains(&id)));
    }
    pub fn invalidate_sources(&mut self, changed: &[AssetId<AtlasSource>]) {
        self.dirty |= self
            .regions
            .iter()
            .any(|r| matches!(r.image, SourceId::Cpu(id) if changed.contains(&id)));
    }
    #[cfg(test)]
    pub fn resolve(
        &mut self,
        requested: &[Region],
        images: &mut Assets<Image>,
        limit: u32,
    ) -> Result<(Handle<Image>, Vec<URect>), String> {
        self.resolve_with_sources(requested, images, limit, None)
    }

    pub fn resolve_with_sources(
        &mut self,
        requested: &[Region],
        images: &mut Assets<Image>,
        limit: u32,
        sources: Option<&Assets<AtlasSource>>,
    ) -> Result<(Handle<Image>, Vec<URect>), String> {
        let cached = !self.dirty
            && self.image.as_ref().is_some_and(|h| images.contains(h))
            && requested.iter().all(|r| self.regions.contains(r));
        if !cached {
            // Keep resident expressions for reuse, but never hold their source
            // handles alive merely because they once appeared in this atlas.
            let mut regions = self
                .regions
                .iter()
                .copied()
                .filter(|r| r.image.get(images, sources).is_some())
                .collect::<Vec<_>>();
            for region in requested {
                if !regions.contains(region) {
                    regions.push(*region);
                }
            }
            let limit = limit.min(8192);
            let built = pack(&regions, images, limit, sources);
            let (layout, image) = match built {
                Ok(value) => value,
                Err(_) if regions.iter().any(|r| !requested.contains(r)) => {
                    // Old expressions are a cache, not a reason to reject a
                    // current expression which fits within the device budget.
                    regions.retain(|r| requested.contains(r));
                    pack(&regions, images, limit, sources)?
                }
                Err(error) => return Err(error),
            };
            let rects = layout
                .textures
                .iter()
                .map(|r| URect::from_corners(r.min + UVec2::ONE, r.max - UVec2::ONE))
                .collect();
            // Publish only after every source is valid and the full pack succeeds.
            self.image = Some(images.add(image));
            self.regions = regions;
            self.rects = rects;
            self.dirty = false;
        }
        let rects = requested
            .iter()
            .map(|r| {
                self.rects[self
                    .regions
                    .iter()
                    .position(|stored| stored == r)
                    .expect("packed requested region")]
            })
            .collect();
        Ok((self.image.clone().expect("packed image"), rects))
    }
}

fn pack(
    regions: &[Region],
    images: &Assets<Image>,
    limit: u32,
    sources: Option<&Assets<AtlasSource>>,
) -> Result<(TextureAtlasLayout, Image), String> {
    let mut area = 0u64;
    for r in regions {
        let width = u64::from(r.rect.width()) + 2;
        let height = u64::from(r.rect.height()) + 2;
        if width > u64::from(limit) || height > u64::from(limit) {
            return Err(format!(
                "character part exceeds the {limit}px runtime atlas limit"
            ));
        }
        area += width * height;
        if area > u64::from(limit) * u64::from(limit) {
            return Err("character parts exceed the runtime atlas area budget".into());
        }
    }
    let tiles = regions
        .iter()
        .map(|r| {
            let image = r
                .image
                .get(images, sources)
                .ok_or("character atlas source is still loading")?;
            crop_with_gutter(*r, image)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut builder = TextureAtlasBuilder::default();
    builder
        .initial_size(UVec2::splat(256.min(limit)))
        .max_size(UVec2::splat(limit))
        .format(TextureFormat::Rgba8UnormSrgb)
        .auto_format_conversion(true);
    for tile in &tiles {
        builder.add_texture(None, tile);
    }
    let (layout, _, mut image) = builder
        .build()
        .map_err(|e| format!("cannot pack character parts: {e}"))?;
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    Ok((layout, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(pixel: [u8; 4]) -> Image {
        Image::new_fill(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &pixel,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }

    fn pixel(image: &Image, point: UVec2) -> &[u8] {
        let offset = (point.y as usize * image.width() as usize + point.x as usize) * 4;
        &image.data.as_ref().expect("CPU pixels")[offset..offset + 4]
    }

    #[test]
    fn packs_cpu_source_after_render_extraction() {
        let mut images = Assets::<Image>::default();
        let mut sources = Assets::<AtlasSource>::default();
        let original = image([40, 80, 120, 160]);
        let source = sources.add(AtlasSource(original.clone()));
        let mut extracted = original;
        extracted.data = None;
        let alice = images.add(extracted);
        let region = Region {
            image: SourceId::Cpu(source.id()),
            rect: URect::new(0, 0, 2, 2),
        };
        let mut cache = PackedAtlas::default();
        let (atlas, rects) = cache
            .resolve_with_sources(&[region], &mut images, 256, Some(&sources))
            .expect("CPU-only asset supplies pixels after render extraction");
        assert_eq!(
            pixel(images.get(&atlas).expect("packed atlas"), rects[0].min),
            &[40, 80, 120, 160]
        );
        assert!(images.get(&alice).expect("render metadata").data.is_none());
    }

    #[test]
    fn packs_pixels_and_gutters_without_changing_color_or_alpha() {
        let mut images = Assets::<Image>::default();
        let alice = images.add(image([64, 128, 192, 90]));
        let bob = images.add(image([255, 30, 0, 255]));
        let regions = [
            Region {
                image: alice.id().into(),
                rect: URect::new(0, 0, 2, 2),
            },
            Region {
                image: bob.id().into(),
                rect: URect::new(1, 0, 2, 2),
            },
        ];
        let mut cache = PackedAtlas::default();
        let (atlas, rects) = cache
            .resolve(&regions, &mut images, 256)
            .expect("pack loose parts");
        let packed = images.get(&atlas).expect("atlas image");
        assert_eq!(
            packed.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(rects[1].size(), UVec2::new(1, 2));
        assert_eq!(pixel(packed, rects[0].min), &[64, 128, 192, 90]);
        assert_eq!(
            pixel(packed, rects[0].min - UVec2::ONE),
            &[64, 128, 192, 90]
        );
        assert_eq!(pixel(packed, rects[1].min), &[255, 30, 0, 255]);
        let (same, reused) = cache
            .resolve(&[regions[1], regions[0]], &mut images, 256)
            .expect("reuse");
        assert_eq!(same, atlas);
        assert_eq!(reused, [rects[1], rects[0]]);
    }

    #[test]
    fn atlas_builder_converts_gray_alpha_only_when_packing() {
        let mut images = Assets::<Image>::default();
        let alice = images.add(Image::from_dynamic(
            image::DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(
                2,
                2,
                image::LumaA([128, 96]),
            )),
            true,
            default(),
        ));
        let bob = images.add(image([10, 20, 30, 255]));
        let regions = [
            Region {
                image: alice.id().into(),
                rect: URect::new(0, 0, 2, 2),
            },
            Region {
                image: bob.id().into(),
                rect: URect::new(0, 0, 2, 2),
            },
        ];
        let (atlas, rects) = PackedAtlas::default()
            .resolve(&regions, &mut images, 256)
            .expect("pack mixed formats");
        assert_eq!(
            pixel(images.get(&atlas).expect("atlas"), rects[0].min),
            &[128, 128, 128, 96]
        );
        let original = images.get(&alice).expect("source unchanged");
        assert_eq!(original.texture_descriptor.format, TextureFormat::Rg8Unorm);
        assert_eq!(original.data.as_ref().expect("source pixels").len(), 8);
    }

    #[test]
    fn new_expression_grows_once_and_hot_reload_invalidates() {
        let mut images = Assets::<Image>::default();
        let alice = images.add(image([10; 4]));
        let bob = images.add(image([20; 4]));
        let a = Region {
            image: alice.id().into(),
            rect: URect::new(0, 0, 2, 2),
        };
        let b = Region {
            image: bob.id().into(),
            rect: a.rect,
        };
        let mut cache = PackedAtlas::default();
        let first = cache
            .resolve(&[a], &mut images, 256)
            .expect("first expression")
            .0;
        let second = cache
            .resolve(&[a, b], &mut images, 256)
            .expect("new expression")
            .0;
        assert_ne!(first, second);
        assert_eq!(
            cache
                .resolve(&[a], &mut images, 256)
                .expect("previous expression")
                .0,
            second
        );
        *images.get_mut(&alice).expect("source") = image([42; 4]);
        cache.invalidate(&[alice.id()]);
        let (third, rects) = cache.resolve(&[a], &mut images, 256).expect("hot reload");
        assert_ne!(third, second);
        assert_eq!(
            pixel(images.get(&third).expect("atlas"), rects[0].min),
            &[42; 4]
        );
    }

    #[test]
    fn cache_evicts_unused_expression_when_only_current_parts_fit() {
        let mut images = Assets::<Image>::default();
        let alice = images.add(image([10; 4]));
        let bob = images.add(image([20; 4]));
        let a = Region {
            image: alice.id().into(),
            rect: URect::new(0, 0, 2, 2),
        };
        let b = Region {
            image: bob.id().into(),
            rect: a.rect,
        };
        let mut cache = PackedAtlas::default();
        cache
            .resolve(&[a], &mut images, 4)
            .expect("one padded tile fits");
        cache
            .resolve(&[b], &mut images, 4)
            .expect("evict old expression to fit current one");
        assert_eq!(cache.regions, vec![b]);
    }

    #[test]
    fn failed_rebuild_keeps_previous_atlas_and_limits_are_checked_before_copying() {
        let mut images = Assets::<Image>::default();
        let alice = images.add(image([255; 4]));
        let a = Region {
            image: alice.id().into(),
            rect: URect::new(0, 0, 2, 2),
        };
        let mut cache = PackedAtlas::default();
        let first = cache.resolve(&[a], &mut images, 256).expect("first").0;
        let oversized = Region {
            rect: URect::new(0, 0, 8192, 8192),
            ..a
        };
        assert!(cache.resolve(&[oversized], &mut images, 8192).is_err());
        assert_eq!(
            cache
                .resolve(&[a], &mut images, 256)
                .expect("previous atlas survives")
                .0,
            first
        );
    }
}

fn crop_with_gutter(region: Region, image: &Image) -> Result<Image, String> {
    // PNG color parts use this format. Existing single-image atlases (including
    // GPU-compressed images) bypass packing entirely.
    let bytes_per_pixel = match image.texture_descriptor.format {
        TextureFormat::R8Unorm => 1,
        TextureFormat::Rg8Unorm => 2,
        TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm => 4,
        _ => {
            return Err(format!(
                "unsupported loose character part format {:?}",
                image.texture_descriptor.format
            ));
        }
    };
    let data = image
        .data
        .as_ref()
        .ok_or("character part has no CPU pixel data for packing")?;
    let rect = region.rect;
    if rect.min.x >= rect.max.x
        || rect.min.y >= rect.max.y
        || rect.max.x > image.width()
        || rect.max.y > image.height()
    {
        return Err("invalid character part crop".into());
    }
    let width = rect.width() + 2;
    let height = rect.height() + 2;
    let length = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(bytes_per_pixel))
        .ok_or("character atlas tile size overflow")?;
    let mut pixels = vec![0; length];
    for y in 0..height {
        let source_y = rect.min.y + y.saturating_sub(1).min(rect.height() - 1);
        let start =
            (source_y as usize * image.width() as usize + rect.min.x as usize) * bytes_per_pixel;
        let end = start + rect.width() as usize * bytes_per_pixel;
        let source = data
            .get(start..end)
            .ok_or("truncated character pixel data")?;
        let row = &mut pixels[y as usize * width as usize * bytes_per_pixel
            ..(y as usize + 1) * width as usize * bytes_per_pixel];
        row[..bytes_per_pixel].copy_from_slice(&source[..bytes_per_pixel]);
        row[bytes_per_pixel..bytes_per_pixel + source.len()].copy_from_slice(source);
        row[bytes_per_pixel + source.len()..]
            .copy_from_slice(&source[source.len() - bytes_per_pixel..]);
    }
    Ok(Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        image.texture_descriptor.format,
        RenderAssetUsages::default(),
    ))
}
