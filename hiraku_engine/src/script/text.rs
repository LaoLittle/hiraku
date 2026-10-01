use hiraku_script::{TemplateError, Value, Vm, runtime::TemplateValue};
use hiraku_text::template::{Template, TextSnapshot, TextValue};
use std::sync::Arc;

pub(crate) struct RecordedText {
    pub text: hiraku_text::template::LocalizableText,
    pub rendered: String,
}

pub(super) fn evaluate_recorded(
    vm: &Vm,
    value: &TemplateValue,
    records: &mut Vec<RecordedText>,
) -> Result<String, TemplateError> {
    let source = vm.eval_template_value_with(value, |source| Ok(source.to_owned()))?;
    let mut context = TextSnapshot::default();
    let mut bindings = vm.lexical_bindings();
    bindings.extend(
        value
            .captures
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    for (name, value) in bindings {
        let value = vm
            .export_value(&value)
            .map_or(TextValue::Opaque, |value| text_value(&value));
        context.values.insert(name, Arc::new(value));
    }
    let text = hiraku_text::template::LocalizableText {
        key: None,
        source,
        context: Arc::new(context),
    };
    let rendered = evaluate_with(&text.source, |root| {
        match value.captures.get(root) {
            Some(value) => vm.export_value(value).map(Some),
            None => vm.lexical_value(root),
        }
        .map_err(|error| TemplateError::InvalidExpression(error.to_string()))
    })?;
    if records.len() >= 64 {
        records.remove(0);
    }
    records.push(RecordedText {
        text,
        rendered: rendered.clone(),
    });
    Ok(rendered)
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
                let records = runtime.take_text_records();
                let captured = records
                    .iter()
                    .find(|record| record.text.source.contains("{a}"))
                    .expect("original source survives wrapper evaluation");
                assert_eq!(
                    captured.text.context.values["a"].as_ref(),
                    &TextValue::Int(1)
                );
                assert_eq!(
                    captured
                        .text
                        .render_with(|_, _| Ok("Translated {a}".into()))
                        .expect("translation uses captured local")
                        .text,
                    "Translated 1"
                );
                return;
            }
        }
        panic!("expected narration");
    }
}
