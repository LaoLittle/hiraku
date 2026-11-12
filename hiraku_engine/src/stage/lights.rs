use bevy::prelude::*;

#[derive(Component)]
pub(super) struct StageLightIdentity {
    pub name: String,
    pub authored: Color,
}

pub(super) fn sync(
    runtime: Res<super::runtime::StageRuntime>,
    shared: Res<crate::state::SceneSharedState>,
    parents: Query<&ChildOf>,
    mut lights: Query<(Entity, &StageLightIdentity, &mut SpotLight)>,
    mut redraw: super::redraw::StageRedraw,
) {
    let Some(root) = runtime.root else { return };
    let Some(state) = &shared.0.spatial_stage else {
        return;
    };
    let Some(definition) = &runtime.definition else {
        return;
    };
    if !runtime.ready(state) {
        return;
    }
    for (entity, identity, mut light) in &mut lights {
        if !parents
            .iter_ancestors(entity)
            .any(|ancestor| ancestor == root)
        {
            continue;
        }
        let color = state
            .light_colors
            .get(&identity.name)
            .map_or(identity.authored, |[r, g, b]| {
                definition.shading.light_color(Color::srgb(*r, *g, *b))
            });
        if light.color != color {
            light.color = color;
            redraw.request();
        }
    }
}
