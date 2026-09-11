use super::*;
use bevy::image::CompressedImageFormats;
use hiraku_hdp::Archive;

#[test]
fn asset_server_loads_uastc_with_standard_mask_settings() {
    use bevy::{
        asset::RenderAssetUsages,
        image::{ImageLoaderSettings, ImageSampler},
        prelude::*,
    };
    let temp = tempfile::tempdir().expect("temporary asset root");
    let bytes = encode_rgba(&[32, 64, 96, 255].repeat(64), 8, 8).expect("encode fixture");
    fs::write(temp.path().join("alice.uastc.ktx2"), bytes).expect("fixture texture");
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: temp.path().to_string_lossy().into_owned(),
            ..Default::default()
        },
    ))
    .init_asset::<Image>()
    .add_plugins(hiraku_uastc::UastcPlugin);
    app.finish();
    app.cleanup();
    let handle: Handle<Image> = app
        .world()
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.is_srgb = false;
            settings.sampler = ImageSampler::nearest();
            settings.asset_usage = RenderAssetUsages::MAIN_WORLD;
        })
        .load("alice.uastc.ktx2");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        app.update();
        if let Some(image) = app.world().resource::<Assets<Image>>().get(&handle) {
            assert!(!image.texture_descriptor.format.is_srgb());
            assert_eq!(image.sampler, ImageSampler::nearest());
            assert_eq!(image.asset_usage, RenderAssetUsages::MAIN_WORLD);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "asset loader did not deliver the UASTC image"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn synthetic_uastc_roundtrips_through_pure_rust_runtime() {
    for (width, height) in [(8, 8), (7, 5)] {
        let pixels = [24, 96, 180, 128].repeat(width * height);
        let encoded =
            encode_rgba(&pixels, width as u32, height as u32).expect("encode synthetic RGBA");
        assert_eq!(
            &encoded[44..48],
            &[0; 4],
            "KTX2 must not use internal supercompression"
        );
        for formats in [
            CompressedImageFormats::NONE,
            CompressedImageFormats::BC,
            CompressedImageFormats::ASTC_LDR,
        ] {
            let image =
                hiraku_uastc::decode(&encoded, formats).expect("runtime transcodes encoder output");
            assert_eq!(image.width(), width as u32);
            assert_eq!(image.height(), height as u32);
            assert!(image.texture_descriptor.format.is_srgb());
            assert_eq!(
                image.texture_descriptor.format.block_dimensions(),
                if width == 8 && formats != CompressedImageFormats::NONE {
                    (4, 4)
                } else {
                    (1, 1)
                }
            );
            if formats == CompressedImageFormats::NONE {
                let decoded = image.data.expect("RGBA bytes");
                assert_eq!(decoded.len(), pixels.len());
                for (original, actual) in pixels.iter().zip(&decoded) {
                    assert!((*original as i16 - *actual as i16).abs() < 12);
                }
            }
        }
    }
    assert!(encode_rgba(&[0; 4], 0, 1).is_err());
    assert!(encode_rgba_with_threads(&[0; 4], 1, 1, 0).is_err());
    assert!(encode_rgba_with_threads(&[0; 4], 1, 1, 65).is_err());
    assert_eq!(UASTC_LEVEL, 3);
    assert!((1..=8).contains(&encoder_threads()));
}

#[test]
fn manifests_share_one_encoded_texture_and_unreferenced_images_are_excluded() {
    let temp = tempfile::tempdir().expect("test directory");
    let root = temp.path().join("source");
    fs::create_dir_all(root.join("textures/nested")).expect("fixture directories");
    let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([20, 40, 60, 128]));
    image
        .save(root.join("textures/alice.png"))
        .expect("synthetic image");
    image
        .save(root.join("textures/bob.png"))
        .expect("unreferenced image");
    let first = b".{ name: \"alice\", image: \"alice.png\" }";
    fs::write(root.join("textures/alice.texture.hson"), first).expect("manifest");
    fs::write(
        root.join("textures/nested/face.texture.hson"),
        b".{ image: \"../alice.png\", regions: .{ face: (0, 0, 4, 4) } }",
    )
    .expect("alias manifest");
    fs::write(root.join("startup.hks"), b"bg(\"alice\")").expect("script");
    let output = temp.path().join("test.hdp");
    let mut events = Vec::new();
    let packed = pack_directory_with_progress(
        &root,
        &output,
        PackOptions {
            chunk_size: 64,
            max_volume_size: Some(4096),
            ..Default::default()
        },
        |event| events.push(event),
    )
    .expect("pack generated inputs");
    assert_eq!(events.first().expect("scan progress").phase, "scan");
    assert_eq!(events.last().expect("completed progress").phase, "done");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.phase == "encode")
            .count(),
        1
    );
    let completed = events
        .iter()
        .find(|event| event.phase == "texture-done")
        .expect("texture completion");
    assert_eq!((completed.completed, completed.total), (1, 1));
    assert!(packed.volume_sizes.iter().all(|size| *size <= 4096));
    let archive = Archive::open(&output).expect("open package");
    let names = archive.files().collect::<Vec<_>>();
    let encoded = names
        .iter()
        .filter(|name| name.ends_with(".uastc.ktx2"))
        .collect::<Vec<_>>();
    assert_eq!(encoded.len(), 1);
    let dependencies: hiraku_hdp::dependencies::DependencyManifest = hson::from_slice(
        &archive
            .read_file(hiraku_hdp::dependencies::DEPENDENCY_MANIFEST)
            .expect("dependency manifest"),
    )
    .expect("dependency schema");
    assert_eq!(
        dependencies.scripts["startup.hks"],
        std::collections::BTreeSet::from([(*encoded[0]).to_owned()])
    );
    assert_eq!(dependencies.image_bytes[*encoded[0]], 8 * 8 * 4);
    assert!(
        dependencies.windows["startup.hks"]
            .nodes
            .iter()
            .any(|node| node.images.contains(*encoded[0]))
    );
    assert!(
        dependencies
            .windows
            .values()
            .flat_map(|g| &g.nodes)
            .all(|node| node.images.iter().all(|path| path.ends_with(".uastc.ktx2")))
    );
    assert!(!names.iter().any(|name| name.ends_with(".png")));
    assert_eq!(
        archive.read_file("startup.hks").expect("script retained"),
        b"bg(\"alice\")"
    );
    for manifest in [
        "textures/alice.texture.hson",
        "textures/nested/face.texture.hson",
    ] {
        let value: HsonValue =
            hson::from_slice(&archive.read_file(manifest).expect("packed manifest"))
                .expect("rewritten HSON");
        let HsonValue::String(path) = &value.as_map().expect("map")["image"] else {
            panic!("image string")
        };
        assert!(path.ends_with(*encoded[0]));
    }
    assert_eq!(
        fs::read(root.join("textures/alice.texture.hson")).expect("source retained"),
        first
    );
    hiraku_uastc::decode(
        &archive.read_file(encoded[0]).expect("texture bytes"),
        CompressedImageFormats::NONE,
    )
    .expect("packaged texture decodes");
}
