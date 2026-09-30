#[derive(Clone, Copy)]
pub(crate) enum Function {
    Text,
    Ruby,
    Color,
    Strong,
    Emph,
    Strike,
    Underline,
    Linebreak,
}

pub(crate) fn lookup(name: &str) -> Option<Function> {
    Some(match name {
        "text" => Function::Text,
        "ruby" => Function::Ruby,
        "color" => Function::Color,
        "strong" => Function::Strong,
        "emph" => Function::Emph,
        "strike" => Function::Strike,
        "underline" => Function::Underline,
        "linebreak" => Function::Linebreak,
        _ => return None,
    })
}
