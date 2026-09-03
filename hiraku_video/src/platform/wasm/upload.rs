use bevy::prelude::*;

/// Web frames are copied into Bevy `Image` assets, so no render-world upload
/// bridge is required.
#[derive(Resource, Default)]
pub(crate) struct VideoUpload;

impl VideoUpload {
    pub fn publish(&mut self, _frame: crate::platform::VideoFrameUpload) {
        unreachable!("WebCodecs frames never use the native strided upload path")
    }
}

pub(crate) fn install_video_upload(app: &mut App) {
    app.init_resource::<VideoUpload>();
}
