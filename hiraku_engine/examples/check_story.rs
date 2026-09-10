//! Offline story checking. Supply paths explicitly; no bundled game fixtures.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = std::env::args().skip(1).collect::<Vec<_>>();
    if paths.first().is_some_and(|arg| arg == "--ui-project-args") {
        if paths.len() != 5 {
            return Err("usage: --ui-project-args ROOT SETTINGS UI HSON_ARGUMENT_LIST".into());
        }
        let hiraku_script::hson::HsonValue::Array(values) = hiraku_script::hson::parse(&paths[4])?
        else {
            return Err("UI arguments must be an HSON list".into());
        };
        let arguments = values
            .into_iter()
            .map(stored_argument)
            .collect::<Result<Vec<_>, _>>()?;
        hiraku_engine::validate_ui_document_with_arguments(
            std::path::Path::new(&paths[1]),
            &paths[2],
            &paths[3],
            hiraku_engine::UiContext::default(),
            &arguments,
        )?;
        println!("Built typed UI document without rendering");
        return Ok(());
    }
    if paths.first().is_some_and(|arg| arg == "--story-project") {
        if paths.len() != 4 {
            return Err("usage: --story-project ROOT SETTINGS ENTRY".into());
        }
        hiraku_engine::validate_story_project(
            std::path::Path::new(&paths[1]),
            &paths[2],
            &paths[3],
        )?;
        println!("Compiled and linked story project without rendering");
        return Ok(());
    }
    if paths.first().is_some_and(|arg| arg == "--ui-project") {
        if paths.len() < 4 {
            return Err("usage: --ui-project ROOT SETTINGS UI_PATH...".into());
        }
        let root = std::path::PathBuf::from(paths.remove(1));
        let settings = paths.remove(1);
        for path in &paths[1..] {
            hiraku_engine::validate_ui_document(&root, &settings, path)?;
        }
        println!("Built {} UI documents without rendering", paths.len() - 1);
        return Ok(());
    }
    if paths.is_empty() {
        return Err("provide one or more HKS source paths".into());
    }
    for path in &paths {
        let source = std::fs::read_to_string(path)?;
        if path.ends_with(".ui.hks") {
            hiraku_engine::validate_ui_source(path, &source)?;
        } else {
            hiraku_engine::validate_story_source(path, &source)?;
        }
    }
    println!("Validated {} script sources", paths.len());
    Ok(())
}

fn stored_argument(
    value: hiraku_script::hson::HsonValue,
) -> Result<hiraku_engine::StoredValue, String> {
    use hiraku_engine::StoredValue as V;
    use hiraku_script::hson::HsonValue as H;
    Ok(match value {
        H::Bool(v) => V::Bool(v),
        H::Integer(v) => V::Int(v),
        H::Unsigned(v) => {
            V::Int(i64::try_from(v).map_err(|_| "integer argument exceeds Int range")?)
        }
        H::Float(v) => V::Float(v),
        H::String(v) => V::String(v),
        H::Array(v) => V::Array(
            v.into_iter()
                .map(stored_argument)
                .collect::<Result<_, _>>()?,
        ),
        H::Map(v) => V::Map(
            v.into_iter()
                .map(|(k, v)| Ok((k, stored_argument(v)?)))
                .collect::<Result<_, String>>()?,
        ),
        H::Null => return Err("UI entry arguments do not support stored null values".into()),
    })
}
