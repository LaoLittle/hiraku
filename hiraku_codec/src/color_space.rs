//! WebCodecs color metadata. Unknown values stay absent; this is distinct from
//! the renderer's resolved transfer function and conversion matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoColorPrimaries {
    Bt709,
    Bt470Bg,
    Smpte170M,
    Bt2020,
    Smpte432,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoTransferCharacteristics {
    Bt709,
    Smpte170M,
    Iec61966_2_1,
    Linear,
    Pq,
    Hlg,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoMatrixCoefficients {
    Rgb,
    Bt709,
    Bt470Bg,
    Smpte170M,
    Bt2020Ncl,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoColorSpaceInit {
    pub primaries: Option<VideoColorPrimaries>,
    pub transfer: Option<VideoTransferCharacteristics>,
    pub matrix: Option<VideoMatrixCoefficients>,
    pub full_range: Option<bool>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoColorSpace(VideoColorSpaceInit);
impl VideoColorSpace {
    pub fn new(init: VideoColorSpaceInit) -> Self {
        Self(init)
    }
    pub fn primaries(&self) -> Option<VideoColorPrimaries> {
        self.0.primaries
    }
    pub fn transfer(&self) -> Option<VideoTransferCharacteristics> {
        self.0.transfer
    }
    pub fn matrix(&self) -> Option<VideoMatrixCoefficients> {
        self.0.matrix
    }
    pub fn full_range(&self) -> Option<bool> {
        self.0.full_range
    }
    /// Rust equivalent of toJSON, without coupling codec to a JSON library.
    pub fn to_init(&self) -> VideoColorSpaceInit {
        self.0
    }
}
