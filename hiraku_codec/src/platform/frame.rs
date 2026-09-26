//! Checked copying at the driver-owned frame boundary. No borrowed driver
//! memory escapes the codec worker; output remains valid after buffer release.
use crate::{CodecError, TransferFunction, VideoFrame, VideoPixels, YuvColorTransform};

pub(super) struct Plane<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
    pub pixel_stride: usize,
}

pub(super) fn copy_plane(
    plane: Plane<'_>,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, CodecError> {
    let error =
        || CodecError::Operation("invalid decoded plane dimensions/stride/buffer length".into());
    if width == 0 || height == 0 || plane.pixel_stride == 0 {
        return Err(error());
    }
    let row = (width - 1)
        .checked_mul(plane.pixel_stride)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(error)?;
    let len = (height - 1)
        .checked_mul(plane.stride)
        .and_then(|n| n.checked_add(row))
        .ok_or_else(error)?;
    if row > plane.stride || len > plane.bytes.len() {
        return Err(error());
    }
    let mut output = vec![0; width.checked_mul(height).ok_or_else(error)?];
    if plane.pixel_stride == 1 && plane.stride == width {
        output.copy_from_slice(&plane.bytes[..len]);
    } else {
        for (y, target) in output.chunks_exact_mut(width).enumerate() {
            let source = &plane.bytes[y * plane.stride..][..row];
            if plane.pixel_stride == 1 {
                target.copy_from_slice(source);
            } else {
                for (x, byte) in target.iter_mut().enumerate() {
                    *byte = source[x * plane.pixel_stride];
                }
            }
        }
    }
    Ok(output)
}

pub(super) fn planar_frame(
    timestamp: i64,
    width: u32,
    height: u32,
    planes: [Plane<'_>; 3],
    color_transform: YuvColorTransform,
    transfer: TransferFunction,
) -> Result<VideoFrame, CodecError> {
    let [y, u, v] = planes;
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    Ok(VideoFrame {
        timestamp,
        width,
        height,
        chroma_width: cw,
        chroma_height: ch,
        color_transform,
        transfer,
        pixels: VideoPixels::I420Planar {
            y: copy_plane(y, width as usize, height as usize)?,
            u: copy_plane(u, cw as usize, ch as usize)?,
            v: copy_plane(v, cw as usize, ch as usize)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn padded_rows_and_interleaved_chroma() {
        let bytes = [1, 9, 2, 9, 0, 0, 3, 9, 4];
        assert_eq!(
            copy_plane(
                Plane {
                    bytes: &bytes,
                    stride: 6,
                    pixel_stride: 2
                },
                2,
                2
            )
            .expect("valid plane"),
            [1, 2, 3, 4]
        );
        assert!(
            copy_plane(
                Plane {
                    bytes: &bytes[..8],
                    stride: 6,
                    pixel_stride: 2
                },
                2,
                2
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_invalid_geometry_and_overflow() {
        for (stride, pixel_stride, w, h) in [
            (1, 1, 2, 1),
            (4, 0, 1, 1),
            (4, 1, 0, 1),
            (usize::MAX, 2, usize::MAX, 2),
        ] {
            assert!(
                copy_plane(
                    Plane {
                        bytes: &[0; 8],
                        stride,
                        pixel_stride
                    },
                    w,
                    h
                )
                .is_err()
            );
        }
    }
    #[test]
    fn odd_size_frame_uses_ceil_chroma_dimensions() {
        let frame = planar_frame(
            42,
            3,
            3,
            [
                Plane {
                    bytes: &[1; 9],
                    stride: 3,
                    pixel_stride: 1,
                },
                Plane {
                    bytes: &[2; 4],
                    stride: 2,
                    pixel_stride: 1,
                },
                Plane {
                    bytes: &[3; 4],
                    stride: 2,
                    pixel_stride: 1,
                },
            ],
            YuvColorTransform::from_luma_coefficients(0.2126, 0.0722, true),
            TransferFunction::Bt1886,
        )
        .expect("valid frame");
        assert_eq!(
            (frame.timestamp, frame.chroma_width, frame.chroma_height),
            (42, 2, 2)
        );
    }
}
