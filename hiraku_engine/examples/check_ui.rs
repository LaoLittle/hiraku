//! Construct a UI offline using caller-supplied HSON model data and arguments.
use hiraku_engine::{StoredValue, UiContext};
use hiraku_script::hson::{self, HsonValue};

fn stored(value: HsonValue) -> Result<StoredValue, String> {
    Ok(match value {
        HsonValue::Null => return Err("UI validation fixtures cannot contain null".into()),
        HsonValue::Bool(v) => StoredValue::Bool(v),
        HsonValue::Integer(v) => StoredValue::Int(v),
        HsonValue::Unsigned(v) => StoredValue::UInt(v),
        HsonValue::Float(v) => StoredValue::Float(v),
        HsonValue::String(v) => StoredValue::String(v),
        HsonValue::Array(v) => {
            StoredValue::Array(v.into_iter().map(stored).collect::<Result<_, _>>()?)
        }
        HsonValue::Map(v) => StoredValue::Map(
            v.into_iter()
                .map(|(k, v)| Ok((k, stored(v)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [root, settings, path, context, arguments] = args.as_slice() else {
        return Err("usage: check_ui <asset-root> <settings-path> <ui-path> <context-hson-map> <arguments-hson-list>".into());
    };
    let StoredValue::Map(context) = stored(hson::parse(context)?)? else {
        return Err("context must be an HSON map".into());
    };
    let StoredValue::Array(arguments) = stored(hson::parse(arguments)?)? else {
        return Err("arguments must be an HSON list".into());
    };
    hiraku_engine::validate_ui_document_with_arguments(
        std::path::Path::new(root),
        settings,
        path,
        UiContext::new(context),
        &arguments,
    )?;
    println!("UI compiled and constructed successfully.");
    Ok(())
}
