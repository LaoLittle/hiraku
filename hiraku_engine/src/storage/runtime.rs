use bevy::prelude::*;
use hiraku_storage::RuntimeStorageStatus;

pub(crate) fn initialize_runtime_storage(config: Res<crate::RuntimeLaunchConfig>) {
    hiraku_storage::initialize_runtime(&config.storage_namespace, vec![
        super::save_storage(&super::save_root_path()),
        super::user_settings::settings_storage(),
        super::profile::backend(),
    ]);
}

pub(crate) fn storage_ready() -> bool {
    hiraku_storage::runtime_status() == RuntimeStorageStatus::Ready
}

pub(crate) fn poll_runtime_storage(
    mut redraw: crate::redraw::Redraw,
    mut previous: Local<Option<RuntimeStorageStatus>>,
    frontend: Option<ResMut<crate::scene::FrontendState>>,
) {
    let status = hiraku_storage::runtime_status();
    if !matches!(status, RuntimeStorageStatus::Ready | RuntimeStorageStatus::Failed(_)) {
        redraw.request();
    }
    if previous.as_ref() == Some(&status) { return }
    if let RuntimeStorageStatus::Failed(error) = &status {
        error!("persistent storage failed; runtime is paused: {error}");
        if let Some(mut frontend) = frontend { frontend.notice = Some(format!("Storage failed: {error}")); }
    } else if status == RuntimeStorageStatus::Ready && previous.as_ref() == Some(&RuntimeStorageStatus::Writing) {
        debug!("persistent storage transaction completed");
    }
    *previous = Some(status);
}
