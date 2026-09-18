use bevy::prelude::*;

use super::{
    rich_text::RubyLabel,
    screen_ui::FitText,
};

pub(crate) fn sync(
    managed_text: Query<
        (Entity, Option<&ChildOf>),
        (
            With<InheritedVisibility>,
            Or<(With<FitText>, With<RubyLabel>)>,
        ),
    >,
    mut visibility: Query<(&Visibility, &mut InheritedVisibility)>,
    children: Query<&Children>,
    mut pending: Local<Vec<(Entity, bool)>>,
) {
    pending.clear();

    for (root, parent) in &managed_text {
        let parent_visible = parent
            .and_then(|parent| visibility.get(parent.parent()).ok())
            .is_none_or(|(_, inherited)| inherited.get());

        pending.push((root, parent_visible));

        while let Some((entity, parent_visible)) = pending.pop() {
            let Ok((explicit, mut inherited)) =
                visibility.get_mut(entity)
            else {
                continue;
            };

            let is_visible = match *explicit {
                Visibility::Visible => true,
                Visibility::Hidden => false,
                Visibility::Inherited => parent_visible,
            };

            if inherited.get() == is_visible {
                continue;
            }

            *inherited = if is_visible {
                InheritedVisibility::VISIBLE
            } else {
                InheritedVisibility::HIDDEN
            };

            if let Ok(descendants) = children.get(entity) {
                for &child in descendants {
                    pending.push((child, is_visible));
                }
            }
        }
    }
}