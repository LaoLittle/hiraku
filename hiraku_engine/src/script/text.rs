use hiraku_script::{TemplateError, Value, Vm, runtime::TemplateValue};
use hiraku_text::template::{Template, TextSnapshot, TextValue};
use std::sync::Arc;

pub(super) fn evaluate(vm: &Vm, value: &TemplateValue) -> Result<String, TemplateError> {
    let source = vm.eval_template_value_with(value, |source| Ok(source.to_owned()))?;
    evaluate_with(&source, |root| {
        match value.captures.get(root) {
            Some(value) => vm.export_value(value).map(Some),
            None => vm.lexical_value(root),
        }
        .map_err(|error| TemplateError::InvalidExpression(error.to_string()))
    })
}

pub(super) fn evaluate_with(
    source: &str,
    mut binding: impl FnMut(&str) -> Result<Option<Value>, TemplateError>,
) -> Result<String, TemplateError> {
    let template = Template::parse(&source)
        .map_err(|error| TemplateError::InvalidExpression(error.to_string()))?;
    if template.selectors().next().is_none() {
        return Ok(source.to_owned());
    }
    let mut snapshot = TextSnapshot::default();
    for root in template.selectors().flat_map(|selector| selector.roots()) {
        if !snapshot.values.contains_key(root) {
            let data = binding(root)?.ok_or_else(|| TemplateError::UnknownPath(root.to_owned()))?;
            snapshot
                .values
                .insert(root.to_owned(), Arc::new(text_value(&data)));
        }
    }
    template
        .render(&snapshot)
        .map(|document| document.to_markup())
        .map_err(|error| TemplateError::InvalidExpression(error.to_string()))
}

fn text_value(value: &Value) -> TextValue {
    match value {
        Value::Bool(value) => TextValue::Bool(*value),
        Value::Int(value) => TextValue::Int(*value),
        Value::UInt(value) => TextValue::UInt(*value),
        Value::Number(value) | Value::Percent(value) => TextValue::Float(*value),
        Value::String(value) => TextValue::String(value.clone()),
        Value::Optional(None) => TextValue::Null,
        Value::Optional(Some(value)) | Value::Typed { value, .. } => text_value(value),
        Value::List(values) | Value::Tuple(values) => TextValue::List(
            values
                .iter()
                .map(|value| Arc::new(text_value(value)))
                .collect(),
        ),
        Value::Map(values) => TextValue::Object(
            values
                .iter()
                .map(|(name, value)| (name.clone(), Arc::new(text_value(value))))
                .collect(),
        ),
        _ => TextValue::Opaque,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::execution_runtime::{ExecutionEvent, ExecutionRuntime};

    #[test]
    fn story_templates_capture_local_values_before_host_dispatch() {
        let program = crate::script::compile_story_bytecode(
            "memory://alice.hks",
            r##"fn line(a: Int) { narrate("#let b = {a};It's #b") }
line(1)
"##,
        )
        .expect("story compiles");
        let mut runtime = ExecutionRuntime::new(program).expect("runtime");
        for _ in 0..500 {
            if let Some(ExecutionEvent::Call { call, .. }) =
                runtime.step().expect("story evaluates")
            {
                let source = call
                    .arguments
                    .iter()
                    .find_map(|argument| match &argument.value {
                        Value::String(value) => Some(value),
                        Value::TextTemplate(value) => Some(&value.source),
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("expected text argument in {call:?}"));
                assert_eq!(hiraku_text::parse(source).expect("render").text, "It's 1");
                return;
            }
        }
        panic!("expected narration");
    }
}
