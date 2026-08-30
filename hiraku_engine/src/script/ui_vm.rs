use std::collections::{BTreeMap, BTreeSet};

use hiraku_script::native::{FromHksValue, HksClosure, IntoHksValue, NativeError, NativeRegistry};
use hiraku_script::{
    BuiltinManifest, LinkedVm, LinkedVmEvent, ModuleId, RenderOptions, SourceMap, StatementValue,
    Value, compile_with_manifest, link_named_modules, parse_program, render_diagnostics,
};
use thiserror::Error;

use crate::{
    glossary::{TermCatalog, TermId},
    state::StoredValue,
    texture::TextureCatalog,
    ui::{
        ButtonNode, ContainerNode, ScreenImageButtonNode, ScreenImageNode, ScreenLayout,
        ScreenNode, ScreenSpec, ScreenTexture, SpacerNode, TextNode,
    },
};

use super::ui_runtime::UiContext;

const UI_NODE_HANDLE_TYPE: u32 = 0x5549_4e4f;
const UI_TEXT_BINDING_HANDLE_TYPE: u32 = 0x5549_424e;
const UI_STDLIB_PATH: &str = "hiraku://std/ui.hks";
const UI_STDLIB_SOURCE: &str = include_str!("std/ui.hks");

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "UiNode", handle_type = UI_NODE_HANDLE_TYPE)]
struct UiNodeHandle(u64);

#[derive(Clone, Copy, hiraku_script::HksHandle)]
#[hks(name = "UiTextBinding", handle_type = UI_TEXT_BINDING_HANDLE_TYPE)]
struct UiTextBindingHandle(u64);

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum UiPosition {
    Absolute(f64, f64),
    Relative(f64, f64),
}

impl UiPosition {
    fn abs(x: f64, y: f64) -> UiPosition { Self::Absolute(x, y) }
    fn rel(x: f64, y: f64) -> UiPosition { Self::Relative(x, y) }
}
}

hiraku_script::hks_define! {
#[derive(Clone, Copy, Debug, PartialEq)]
enum UiSize {
    Absolute(f64, f64),
    Relative(f64, f64),
}

impl UiSize {
    fn abs(width: f64, height: f64) -> UiSize { Self::Absolute(width, height) }
    fn rel(width: f64, height: f64) -> UiSize { Self::Relative(width, height) }
}
}

#[derive(Clone, Debug)]
enum UiDraftKind {
    Screen,
    Column,
    Row,
    Image(String),
    Text {
        text: String,
        binding: Option<String>,
    },
    Term(TermId),
    Button(Value),
    Spacer,
}

#[derive(Clone, Debug)]
struct UiDraft {
    kind: UiDraftKind,
    content: Option<HksClosure>,
    hovered: Option<HksClosure>,
    layout: ScreenLayout,
    panel: bool,
    enabled: bool,
    hovered_when_disabled: bool,
    gap: f32,
    background_texture: Option<String>,
    overlay: Option<[f32; 4]>,
}

impl UiDraft {
    fn new(kind: UiDraftKind, content: Option<HksClosure>) -> Self {
        Self {
            kind,
            content,
            hovered: None,
            layout: ScreenLayout::default(),
            panel: true,
            enabled: true,
            hovered_when_disabled: false,
            gap: 12.0,
            background_texture: None,
            overlay: None,
        }
    }
}

struct UiVmContext {
    values: UiContext,
    terms: TermCatalog,
    next_node: u64,
    nodes: BTreeMap<u64, UiDraft>,
    bindings: BTreeMap<u64, String>,
}

impl UiVmContext {
    fn new(values: UiContext, terms: TermCatalog) -> Self {
        Self {
            values,
            terms,
            next_node: 0,
            nodes: BTreeMap::new(),
            bindings: BTreeMap::new(),
        }
    }

    fn insert(&mut self, draft: UiDraft) -> UiNodeHandle {
        self.next_node += 1;
        self.nodes.insert(self.next_node, draft);
        UiNodeHandle(self.next_node)
    }

    fn node_mut(&mut self, handle: UiNodeHandle) -> Result<&mut UiDraft, NativeError> {
        self.nodes
            .get_mut(&handle.0)
            .ok_or_else(|| NativeError::message(format!("unknown UiNode handle {}", handle.0)))
    }

    fn insert_binding(&mut self, template: String) -> UiTextBindingHandle {
        self.next_node += 1;
        self.bindings.insert(self.next_node, template);
        UiTextBindingHandle(self.next_node)
    }
}

#[hiraku_script::hks_module("ui")]
mod reactive_ui {
    use super::*;

    #[hks]
    fn native_bind(
        context: &mut UiVmContext,
        template: String,
    ) -> Result<UiTextBindingHandle, NativeError> {
        let template = context
            .values
            .expand_binding(&template)
            .map_err(|error| NativeError::message(error.to_string()))?;
        Ok(context.insert_binding(template))
    }
}

#[hiraku_script::hks_module]
mod native_ui {
    use super::*;

    #[hks(name = "__uiScreen")]
    fn ui_screen(
        context: &mut UiVmContext,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Screen, Some(content))))
    }

    #[hks(name = "__uiColumn")]
    fn ui_column(
        context: &mut UiVmContext,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Column, Some(content))))
    }

    #[hks(name = "__uiRow")]
    fn ui_row(context: &mut UiVmContext, content: HksClosure) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Row, Some(content))))
    }

    #[hks(name = "__uiImage")]
    fn ui_image(context: &mut UiVmContext, path: String) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Image(path), None)))
    }

    #[hks(name = "__uiText")]
    fn ui_text(context: &mut UiVmContext, value: Value) -> Result<UiNodeHandle, NativeError> {
        let (text, binding) = match value {
            Value::String(value) => (
                context
                    .values
                    .expand(&value)
                    .map_err(|error| NativeError::message(error.to_string()))?,
                None,
            ),
            value => {
                let binding = UiTextBindingHandle::from_hks_value(&value)?;
                let template = context.bindings.get(&binding.0).cloned().ok_or_else(|| {
                    NativeError::message(format!("unknown UI text binding {}", binding.0))
                })?;
                (template.clone(), Some(template))
            }
        };
        Ok(context.insert(UiDraft::new(UiDraftKind::Text { text, binding }, None)))
    }

    #[hks(name = "__uiTerm")]
    fn ui_term(context: &mut UiVmContext, id: String) -> Result<UiNodeHandle, NativeError> {
        let term = context
            .terms
            .resolve(&id)
            .ok_or_else(|| NativeError::message(format!("term `{id}` is not defined")))?;
        Ok(context.insert(UiDraft::new(UiDraftKind::Term(term), None)))
    }

    #[hks(name = "__uiButton")]
    fn ui_button(
        context: &mut UiVmContext,
        value: Value,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Button(value), Some(content))))
    }

    #[hks(name = "__uiSpacer")]
    fn ui_spacer(context: &mut UiVmContext) -> Result<UiNodeHandle, NativeError> {
        Ok(context.insert(UiDraft::new(UiDraftKind::Spacer, None)))
    }

    #[hks(name = "at", receiver)]
    fn ui_at(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        position: UiPosition,
    ) -> Result<UiNodeHandle, NativeError> {
        let layout = &mut context.node_mut(node)?.layout;
        match position {
            UiPosition::Absolute(x, y) => {
                layout.left = Some(finite_f32(x, "absolute UI x")?);
                layout.top = Some(finite_f32(y, "absolute UI y")?);
            }
            UiPosition::Relative(x, y) => {
                layout.left_percent = Some(percent(x, "relative UI x")?);
                layout.top_percent = Some(percent(y, "relative UI y")?);
            }
        }
        Ok(node)
    }

    #[hks(name = "size", receiver)]
    fn ui_size(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        size: UiSize,
    ) -> Result<UiNodeHandle, NativeError> {
        let layout = &mut context.node_mut(node)?.layout;
        match size {
            UiSize::Absolute(width, height) => {
                layout.width = Some(non_negative(width, "absolute UI width")?);
                layout.height = Some(non_negative(height, "absolute UI height")?);
            }
            UiSize::Relative(width, height) => {
                layout.width_percent = Some(percent(width, "relative UI width")?);
                layout.height_percent = Some(percent(height, "relative UI height")?);
            }
        }
        Ok(node)
    }

    #[hks(name = "gap", receiver)]
    fn ui_gap(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        gap: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.gap = non_negative(gap, "UI gap")?;
        Ok(node)
    }

    #[hks(name = "panel", receiver)]
    fn ui_panel(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        panel: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.panel = panel;
        Ok(node)
    }

    #[hks(name = "background", receiver)]
    fn ui_background(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        texture: String,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.background_texture = Some(texture);
        Ok(node)
    }

    #[hks(name = "overlay", receiver)]
    fn ui_overlay(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.overlay = Some([
            color_component(red)?,
            color_component(green)?,
            color_component(blue)?,
            color_component(alpha)?,
        ]);
        Ok(node)
    }

    #[hks(name = "enabled", receiver)]
    fn ui_enabled(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        enabled: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.enabled = enabled;
        Ok(node)
    }

    #[hks(name = "hoveredWhenDisabled", receiver)]
    fn ui_hovered_when_disabled(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        enabled: bool,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hovered_when_disabled = enabled;
        Ok(node)
    }

    #[hks(name = "hovered", receiver)]
    fn ui_hovered(
        context: &mut UiVmContext,
        node: UiNodeHandle,
        content: HksClosure,
    ) -> Result<UiNodeHandle, NativeError> {
        context.node_mut(node)?.hovered = Some(content);
        Ok(node)
    }
}

fn finite_f32(value: f64, label: &str) -> Result<f32, NativeError> {
    value
        .is_finite()
        .then_some(value as f32)
        .ok_or_else(|| NativeError::message(format!("{label} must be finite")))
}

fn non_negative(value: f64, label: &str) -> Result<f32, NativeError> {
    if !value.is_finite() || value < 0.0 {
        return Err(NativeError::message(format!(
            "{label} must be a non-negative number"
        )));
    }
    Ok(value as f32)
}

fn percent(value: f64, label: &str) -> Result<f32, NativeError> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(NativeError::message(format!(
            "{label} must be between 0 and 100"
        )));
    }
    Ok(value as f32)
}

fn color_component(value: f64) -> Result<f32, NativeError> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(NativeError::message(
            "UI color components must be between 0 and 1",
        ));
    }
    Ok(value as f32)
}

fn ui_registry() -> NativeRegistry<UiVmContext> {
    let mut registry = NativeRegistry::new();
    UiPosition::register_hks(&mut registry)
        .expect("UiPosition registration must be internally consistent");
    UiSize::register_hks(&mut registry).expect("UiSize registration must be internally consistent");
    // Register the nominal result type before compiling the HKS standard library.
    registry.define_type("UiNode");
    native_ui::register_hks(&mut registry)
        .expect("UI native primitives must be internally consistent");
    reactive_ui::register_hks(&mut registry)
        .expect("reactive UI API registration must be internally consistent");
    registry
}

#[derive(Debug, Error)]
pub enum UiVmError {
    #[error("failed to compile declarative UI: {0}")]
    Compile(String),
    #[error("failed to link declarative UI: {0}")]
    Link(String),
    #[error("declarative UI runtime failed: {0}")]
    Runtime(String),
    #[error("invalid declarative UI: {0}")]
    Invalid(String),
}

pub fn evaluate_ui_component_named(
    path: &str,
    source: &str,
    values: UiContext,
    textures: &TextureCatalog,
    terms: &TermCatalog,
) -> Result<ScreenSpec, UiVmError> {
    let registry = ui_registry();
    let manifest = registry.manifest();
    let standard = compile_module(UI_STDLIB_PATH, UI_STDLIB_SOURCE, &manifest)?;
    let document = compile_module(path, source, &manifest)?;
    let program = link_named_modules(
        vec![(Some("ui.widgets".to_string()), standard), (None, document)],
        &manifest,
    )
    .map_err(|errors| {
        UiVmError::Link(
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    let materialize_program = program.clone();
    let mut context = UiVmContext::new(values, terms.clone());
    let vm = LinkedVm::new(program, ModuleId(1))
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    let roots = collect_nodes(vm, &registry, &mut context)?;
    if roots.len() != 1 {
        return Err(UiVmError::Invalid(format!(
            "a UI document must produce exactly one root node, got {}",
            roots.len()
        )));
    }
    materialize_screen(
        roots[0],
        &materialize_program,
        &registry,
        &mut context,
        textures,
    )
}

fn compile_module(
    path: &str,
    source: &str,
    manifest: &BuiltinManifest,
) -> Result<hiraku_script::Bytecode, UiVmError> {
    let mut sources = SourceMap::new();
    let source_id = sources.insert(path, source);
    let program = parse_program(source).map_err(|errors| {
        UiVmError::Compile(render_diagnostics(
            &errors
                .into_iter()
                .map(|error| error.diagnostic(source_id.clone()))
                .collect::<Vec<_>>(),
            &sources,
            RenderOptions::terminal(),
        ))
    })?;
    compile_with_manifest(&program, source_hash(path, source), manifest).map_err(|errors| {
        UiVmError::Compile(render_diagnostics(
            &errors
                .into_iter()
                .map(|error| error.diagnostic(source_id.clone()))
                .collect::<Vec<_>>(),
            &sources,
            RenderOptions::terminal(),
        ))
    })
}

fn source_hash(path: &str, source: &str) -> u64 {
    path.bytes()
        .chain([0])
        .chain(source.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
        })
}

fn collect_nodes(
    mut vm: LinkedVm,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let mut nodes = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let event = match vm.step() {
            Ok(event) => event,
            Err(error) => {
                let snapshot = vm.snapshot();
                let frame = snapshot.frames.last();
                return Err(UiVmError::Runtime(match frame {
                    Some(frame) => format!(
                        "{error:?} in module {} at {:?}:{} with registers {:?}",
                        frame.module.0,
                        frame.vm.location,
                        frame.vm.pc.saturating_sub(1),
                        frame.vm.registers,
                    ),
                    None => format!("{error:?}"),
                }));
            }
        };
        match event {
            Some(LinkedVmEvent::Call(call)) => {
                let value = registry
                    .call(context, &call)
                    .map_err(|error| UiVmError::Runtime(error.to_string()))?;
                vm.resume(value)
                    .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
            }
            Some(LinkedVmEvent::Statement(StatementValue::Value(value))) => {
                let node = UiNodeHandle::from_hks_value(&value).map_err(|_| {
                    UiVmError::Invalid("UI expression statements must produce UiNode".into())
                })?;
                // Script-defined components can return the same handle through
                // several wrapper frames. It still represents one emitted node.
                if seen.insert(node.0) {
                    nodes.push(node);
                }
            }
            Some(LinkedVmEvent::Statement(StatementValue::Commit)) => {}
            Some(LinkedVmEvent::Statement(StatementValue::String(_))) => {
                return Err(UiVmError::Invalid(
                    "bare strings are not UI nodes; wrap the value with text(...)".into(),
                ));
            }
            Some(LinkedVmEvent::Completed(_)) => return Ok(nodes),
            None => {
                return Err(UiVmError::Runtime(
                    "UI VM stopped without completing or requesting a native call".into(),
                ));
            }
        }
    }
}

fn closure_children(
    closure: Option<HksClosure>,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
) -> Result<Vec<UiNodeHandle>, UiVmError> {
    let Some(closure) = closure else {
        return Ok(Vec::new());
    };
    let callable = closure.into_hks_value();
    let vm = LinkedVm::from_callable(program.clone(), &callable, Vec::new())
        .map_err(|error| UiVmError::Runtime(format!("{error:?}")))?;
    collect_nodes(vm, registry, context)
}

fn materialize_screen(
    root: UiNodeHandle,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
    textures: &TextureCatalog,
) -> Result<ScreenSpec, UiVmError> {
    let draft = context
        .nodes
        .get(&root.0)
        .cloned()
        .ok_or_else(|| UiVmError::Invalid("UI root handle no longer exists".into()))?;
    if !matches!(draft.kind, UiDraftKind::Screen) {
        return Err(UiVmError::Invalid(
            "a UI document root must be screen { ... } or canvas { ... }".into(),
        ));
    }
    let child_handles = closure_children(draft.content, program, registry, context)?;
    let children = child_handles
        .into_iter()
        .map(|child| materialize_node(child, program, registry, context, textures))
        .collect::<Result<Vec<_>, _>>()?;
    let background_texture = draft
        .background_texture
        .as_deref()
        .map(|name| resolve_texture(textures, name))
        .transpose()?;
    Ok(ScreenSpec {
        title: None,
        panel: draft.panel,
        width: draft.layout.width,
        background_texture,
        xalign: 0.5,
        yalign: 0.5,
        padding: 24.0,
        gap: draft.gap,
        overlay: draft.overlay,
        background: None,
        border: None,
        children,
    })
}

fn materialize_node(
    handle: UiNodeHandle,
    program: &hiraku_script::LinkedProgram,
    registry: &NativeRegistry<UiVmContext>,
    context: &mut UiVmContext,
    textures: &TextureCatalog,
) -> Result<ScreenNode, UiVmError> {
    let draft = context
        .nodes
        .get(&handle.0)
        .cloned()
        .ok_or_else(|| UiVmError::Invalid(format!("unknown UiNode handle {}", handle.0)))?;
    match draft.kind {
        UiDraftKind::Screen => Err(UiVmError::Invalid(
            "screen nodes may only appear at the document root".into(),
        )),
        UiDraftKind::Image(path) => Ok(ScreenNode::Image(ScreenImageNode {
            texture: resolve_texture(textures, &path)?,
            layout: draft.layout,
        })),
        UiDraftKind::Text { text, binding } => Ok(ScreenNode::Text(TextNode {
            text,
            binding,
            size: 28.0,
            color: None,
            align: None,
            layout: draft.layout,
        })),
        UiDraftKind::Term(term) => {
            let definition = context
                .terms
                .get(term)
                .ok_or_else(|| UiVmError::Invalid("interned term no longer exists".into()))?;
            Ok(ScreenNode::Text(TextNode {
                text: definition.name.clone(),
                binding: None,
                size: 28.0,
                color: None,
                align: None,
                layout: draft.layout,
            }))
        }
        UiDraftKind::Spacer => Ok(ScreenNode::Spacer(SpacerNode {
            width: draft.layout.width.unwrap_or(0.0),
            height: draft.layout.height.unwrap_or(0.0),
        })),
        UiDraftKind::Column | UiDraftKind::Row => {
            let row = matches!(draft.kind, UiDraftKind::Row);
            let child_handles = closure_children(draft.content, program, registry, context)?;
            let children = child_handles
                .into_iter()
                .map(|child| materialize_node(child, program, registry, context, textures))
                .collect::<Result<Vec<_>, _>>()?;
            let container = ContainerNode {
                gap: draft.gap,
                padding: 0.0,
                background: None,
                border: None,
                justify: None,
                align_items: None,
                layout: draft.layout,
                children,
            };
            Ok(if row {
                ScreenNode::Row(container)
            } else {
                ScreenNode::Column(container)
            })
        }
        UiDraftKind::Button(value) => {
            let normal_handles = closure_children(draft.content, program, registry, context)?;
            if normal_handles.len() != 1 {
                return Err(UiVmError::Invalid(format!(
                    "button content must produce exactly one text or image node, got {}",
                    normal_handles.len()
                )));
            }
            let normal = materialize_node(normal_handles[0], program, registry, context, textures)?;
            let value = stored_value(value)?;
            match normal {
                ScreenNode::Text(text) => Ok(ScreenNode::Button(ButtonNode {
                    text: text.text,
                    value: Some(value),
                    action: None,
                    enabled: draft.enabled,
                    size: text.size,
                    color: None,
                    hovered_color: None,
                    pressed_color: None,
                    insensitive_color: None,
                    background: None,
                    border: None,
                    hovered_background: None,
                    pressed_background: None,
                    align: text.align,
                    padding_x: None,
                    padding_y: None,
                    border_width: None,
                    radius: None,
                    layout: draft.layout,
                })),
                ScreenNode::Image(image) => {
                    let hovered = closure_children(draft.hovered, program, registry, context)?;
                    let hovered = match hovered.as_slice() {
                        [] => None,
                        [handle] => Some(materialize_node(
                            *handle, program, registry, context, textures,
                        )?),
                        _ => {
                            return Err(UiVmError::Invalid(
                                "hovered content must produce at most one image node".into(),
                            ));
                        }
                    };
                    let (hovered_texture, hovered_layout) = match hovered {
                        Some(ScreenNode::Image(image)) => (Some(image.texture), Some(image.layout)),
                        Some(_) => {
                            return Err(UiVmError::Invalid(
                                "an image button's hovered state must produce image(...)".into(),
                            ));
                        }
                        None => (None, None),
                    };
                    Ok(ScreenNode::ImageButton(ScreenImageButtonNode {
                        texture: image.texture,
                        hovered_texture,
                        hovered_layout,
                        value,
                        enabled: draft.enabled,
                        hovered_when_disabled: draft.hovered_when_disabled,
                        layout: image.layout,
                    }))
                }
                _ => Err(UiVmError::Invalid(
                    "button content must be text(...) or image(...)".into(),
                )),
            }
        }
    }
}

fn stored_value(value: Value) -> Result<StoredValue, UiVmError> {
    match value {
        Value::Bool(value) => Ok(StoredValue::Bool(value)),
        Value::Number(value) if value.fract() == 0.0 => Ok(StoredValue::Int(value as i64)),
        Value::Number(value) => Ok(StoredValue::Float(value)),
        Value::String(value) | Value::Symbol(value) => Ok(StoredValue::String(value)),
        Value::List(values) | Value::Tuple(values) => values
            .into_iter()
            .map(stored_value)
            .collect::<Result<Vec<_>, _>>()
            .map(StoredValue::Array),
        Value::Map(values) => values
            .into_iter()
            .map(|(key, value)| Ok((key, stored_value(value)?)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(StoredValue::Map),
        _ => Err(UiVmError::Invalid(
            "button values must be persistable HKS values".into(),
        )),
    }
}

fn resolve_texture(textures: &TextureCatalog, name: &str) -> Result<ScreenTexture, UiVmError> {
    let texture = textures
        .resolve(name)
        .ok_or_else(|| UiVmError::Invalid(format!("texture `{name}` is not defined")))?;
    Ok(ScreenTexture {
        path: texture.path.clone(),
        rect: texture.rect,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NamespaceTestContext;

    #[hiraku_script::hks_module("ui.widgets")]
    mod namespaced_test_api {
        use super::*;

        #[hks]
        fn native_label(
            _context: &mut NamespaceTestContext,
            value: String,
        ) -> Result<String, NativeError> {
            Ok(value)
        }
    }

    #[test]
    fn module_macro_registers_a_namespaced_selector_and_signature() {
        let mut registry = NativeRegistry::<NamespaceTestContext>::new();
        namespaced_test_api::register_hks(&mut registry)
            .expect("namespaced module should register");
        let manifest = registry.manifest();
        let builtin = manifest
            .resolve_selector("ui.widgets", "label")
            .expect("module namespace should become the selector namespace");
        let signature = manifest
            .signature(builtin)
            .expect("module macro should generate a signature");
        assert_eq!(
            signature.parameters,
            vec![hiraku_script::ScriptType::String]
        );
        assert_eq!(signature.result, hiraku_script::ScriptType::String);
    }

    #[test]
    fn evaluates_script_defined_compose_ui() {
        let source = r#"
import ui.widgets.*

screen {
    column {
        text("Hello ${playerName}")
        button("continue") {
            text("Continue")
        }
    }.gap(18)
}
"#;
        let values = UiContext::new(BTreeMap::from([(
            "playerName".to_string(),
            StoredValue::String("alice".to_string()),
        )]));

        let screen = evaluate_ui_component_named(
            "memory://compose.ui.hks",
            source,
            values,
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("the declarative UI should compile and evaluate");

        assert_eq!(screen.children.len(), 1);
        let ScreenNode::Column(column) = &screen.children[0] else {
            panic!("the root child should be a column");
        };
        assert_eq!(column.gap, 18.0);
        assert_eq!(column.children.len(), 2);
        let ScreenNode::Text(text) = &column.children[0] else {
            panic!("the first column child should be text");
        };
        assert_eq!(text.text, "Hello alice");
        let ScreenNode::Button(button) = &column.children[1] else {
            panic!("the second column child should be a button");
        };
        assert_eq!(button.text, "Continue");
        assert_eq!(button.value, Some(StoredValue::String("continue".into())));
    }

    #[test]
    fn canvas_is_an_unpanelled_transparent_root() {
        let screen = evaluate_ui_component_named(
            "memory://overlay.ui.hks",
            "import ui.widgets.*\ncanvas { text(\"HUD\") }",
            UiContext::default(),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("canvas UI should evaluate");

        assert!(!screen.panel);
        assert_eq!(screen.overlay, Some([0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn live_text_bindings_capture_story_values_and_preserve_signals() {
        let screen = evaluate_ui_component_named(
            "memory://live_overlay.ui.hks",
            concat!(
                "import ui.widgets.*\n",
                "canvas { text(ui.bind(\"Player ${playerName}, ${time.elapsedSeconds}s\")) }",
            ),
            UiContext::new(BTreeMap::from([(
                "playerName".to_string(),
                StoredValue::String("alice".to_string()),
            )])),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect("live UI binding should evaluate");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("canvas child should be text");
        };
        assert_eq!(
            text.binding.as_deref(),
            Some("Player alice, ${time.elapsedSeconds}s")
        );
    }

    #[test]
    fn rejects_bare_strings_as_ui_nodes() {
        let error = evaluate_ui_component_named(
            "memory://invalid.ui.hks",
            "import ui.widgets.*\nscreen { \"not a node\" }",
            UiContext::new(BTreeMap::new()),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("bare strings must not silently become UI nodes");

        assert!(
            error.to_string().contains("wrap the value with text"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn term_accepts_only_a_user_defined_string_id() {
        let terms = crate::glossary::parse_term_catalog(
            "memory://glossary.hson",
            r#".{ terms: [
                .{ id: "ether", name: "Ether", description: "A fictional substance." },
            ] }"#,
        )
        .expect("glossary should parse");
        let screen = evaluate_ui_component_named(
            "memory://terms.ui.hks",
            r#"import ui.widgets.*
screen { term("ether") }"#,
            UiContext::default(),
            &TextureCatalog::default(),
            &terms,
        )
        .expect("a defined string term should render");

        let ScreenNode::Text(text) = &screen.children[0] else {
            panic!("the default term component should render its display name");
        };
        assert_eq!(text.text, "Ether");
    }

    #[test]
    fn fluent_methods_accept_trailing_closures() {
        let source = r#"
import ui.widgets.*

screen {
    image("alice/background").at(.rel(0, 0)).size(.rel(100, 100))
    image("alice/logo").at(.rel(50, 10)).size(.rel(25, 25))

    button("alice") {
        image("alice/idle")
    }
        .enabled(false)
        .hoveredWhenDisabled(true)
        .hovered {
            image("alice/hovered")
        }

    button("bob") {
        image("bob/idle").at(.rel(10, 70)).size(.rel(20, 20))
    }.hovered {
        image("bob/hovered").at(.rel(10, 69)).size(.rel(20, 21))
    }
}.panel(false)
"#;
        let error = evaluate_ui_component_named(
            "memory://hover.ui.hks",
            source,
            UiContext::new(BTreeMap::new()),
            &TextureCatalog::default(),
            &TermCatalog::default(),
        )
        .expect_err("the fake texture is intentionally absent");

        assert!(
            matches!(error, UiVmError::Invalid(ref message) if message.contains("alice/")),
            "the fluent closure should execute before texture resolution: {error}"
        );
    }
}
