//! Typed registration and dispatch for embedding-owned HKS functions.
//!
//! Registries contain Rust functions and deliberately are not serializable. Bytecode and
//! snapshots only retain [`BuiltinId`] values plus the manifest hash.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::{
    symbol::{SymbolId, SymbolInterner},
    vm::{
        BuiltinCall, BuiltinId, BuiltinManifest, FunctionSignature, StaticMember, StaticMemberKind,
        Value,
    },
};

type NativeResult = Result<Value, NativeError>;
type NativeThunk<C> = dyn Fn(&mut C, &[Value]) -> NativeResult + Send + Sync + 'static;
type RawNativeThunk<C> = dyn Fn(&mut C, &BuiltinCall) -> NativeResult + Send + Sync + 'static;

pub struct NativeRegistry<C> {
    names: BTreeMap<String, BuiltinId>,
    selectors: BTreeMap<(String, String), BuiltinId>,
    operators: BTreeMap<String, BuiltinId>,
    functions: BTreeMap<BuiltinId, Box<NativeThunk<C>>>,
    raw_functions: BTreeMap<BuiltinId, Box<RawNativeThunk<C>>>,
    symbols: SymbolInterner,
    signatures: BTreeMap<BuiltinId, FunctionSignature>,
    static_members: Vec<StaticMember>,
}

impl<C> Default for NativeRegistry<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> NativeRegistry<C> {
    pub fn new() -> Self {
        Self {
            names: BTreeMap::new(),
            selectors: BTreeMap::new(),
            operators: BTreeMap::new(),
            functions: BTreeMap::new(),
            raw_functions: BTreeMap::new(),
            symbols: SymbolInterner::new(),
            signatures: BTreeMap::new(),
            static_members: Vec::new(),
        }
    }

    pub fn define_type(&mut self, name: impl Into<String>) -> SymbolId {
        self.symbols.intern(name)
    }

    pub fn set_signature(
        &mut self,
        builtin: BuiltinId,
        signature: FunctionSignature,
    ) -> Result<(), RegistrationError> {
        if !self.functions.contains_key(&builtin) {
            return Err(RegistrationError::UnknownBuiltin(builtin));
        }
        self.signatures.insert(builtin, signature);
        Ok(())
    }

    pub fn register_static_raw_fn_with_id<F>(
        &mut self,
        id: BuiltinId,
        owner: SymbolId,
        name: impl Into<String>,
        signature: FunctionSignature,
        kind: StaticMemberKind,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let name = name.into();
        if self.symbols.resolve(owner).is_none() {
            return Err(RegistrationError::UnknownType(owner));
        }
        if kind == StaticMemberKind::Getter && !signature.parameters.is_empty() {
            return Err(RegistrationError::GetterHasParameters(name));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: name,
            });
        }
        let name_id = self.symbols.intern(name.clone());
        if self
            .static_members
            .iter()
            .any(|member| member.owner == owner && member.name == name_id)
        {
            return Err(RegistrationError::DuplicateName(format!(
                "{}.{}",
                self.symbols.resolve(owner).unwrap_or("<unknown>"),
                name
            )));
        }
        self.functions.insert(
            id,
            Box::new(move |_context, _| {
                unreachable!("raw functions are dispatched before typed thunks")
            }),
        );
        self.raw_functions.insert(id, Box::new(function));
        self.signatures.insert(id, signature);
        self.static_members.push(StaticMember {
            owner,
            name: name_id,
            builtin: id,
            kind,
        });
        Ok(())
    }

    /// Registers a function using an ID deterministically derived from its public name.
    ///
    /// Use [`Self::register_fn_with_id`] for a shipped API whose numeric ABI must never change.
    pub fn register_fn<F, Args>(
        &mut self,
        name: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let name = name.into();
        let id = stable_builtin_id(&name);
        self.register_fn_with_id(id, name, function)?;
        Ok(id)
    }

    pub fn register_fn_with_id<F, Args>(
        &mut self,
        id: BuiltinId,
        name: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let name = name.into();
        if self.names.contains_key(&name) {
            return Err(RegistrationError::DuplicateName(name));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: name,
            });
        }
        self.names.insert(name, id);
        self.functions.insert(id, function.into_native_function());
        Ok(())
    }

    /// Registers a low-level function that needs named-argument metadata or custom validation.
    pub fn register_raw_fn_with_id<F>(
        &mut self,
        id: BuiltinId,
        name: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let name = name.into();
        if self.names.contains_key(&name) {
            return Err(RegistrationError::DuplicateName(name));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: name,
            });
        }
        self.names.insert(name, id);
        self.functions.insert(
            id,
            Box::new(move |_context, _| {
                unreachable!("raw functions are dispatched before typed thunks")
            }),
        );
        self.raw_functions.insert(id, Box::new(function));
        Ok(())
    }

    pub fn register_selector_raw_fn_with_id<F>(
        &mut self,
        id: BuiltinId,
        selector: impl Into<String>,
        method: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let key = (selector.into(), method.into());
        if self.selectors.contains_key(&key) {
            return Err(RegistrationError::DuplicateName(format!(
                "{}.{}",
                key.0, key.1
            )));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: format!("{}.{}", key.0, key.1),
            });
        }
        self.selectors.insert(key, id);
        self.functions.insert(
            id,
            Box::new(move |_context, _| {
                unreachable!("raw functions are dispatched before typed thunks")
            }),
        );
        self.raw_functions.insert(id, Box::new(function));
        Ok(())
    }

    pub fn register_selector_fn_with_id<F, Args>(
        &mut self,
        id: BuiltinId,
        selector: impl Into<String>,
        method: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let key = (selector.into(), method.into());
        if self.selectors.contains_key(&key) {
            return Err(RegistrationError::DuplicateName(format!(
                "{}.{}",
                key.0, key.1
            )));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: format!("{}.{}", key.0, key.1),
            });
        }
        self.selectors.insert(key, id);
        self.functions.insert(id, function.into_native_function());
        Ok(())
    }

    pub fn register_operator_fn_with_id<F, Args>(
        &mut self,
        id: BuiltinId,
        operator: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let operator = operator.into();
        self.validate_operator(id, &operator)?;
        self.operators.insert(operator, id);
        self.functions.insert(id, function.into_native_function());
        Ok(())
    }

    pub fn register_operator_raw_fn_with_id<F>(
        &mut self,
        id: BuiltinId,
        operator: impl Into<String>,
        function: F,
    ) -> Result<(), RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let operator = operator.into();
        self.validate_operator(id, &operator)?;
        self.operators.insert(operator, id);
        self.functions.insert(
            id,
            Box::new(move |_context, _| {
                unreachable!("raw functions are dispatched before typed thunks")
            }),
        );
        self.raw_functions.insert(id, Box::new(function));
        Ok(())
    }

    fn validate_operator(&self, id: BuiltinId, operator: &str) -> Result<(), RegistrationError> {
        if self.operators.contains_key(operator) {
            return Err(RegistrationError::DuplicateName(format!(
                "operator {operator}"
            )));
        }
        if let Some(existing) = self.name_for_id(id) {
            return Err(RegistrationError::DuplicateId {
                id,
                existing,
                attempted: format!("operator {operator}"),
            });
        }
        Ok(())
    }

    pub fn manifest(&self) -> BuiltinManifest {
        BuiltinManifest::with_operators(
            self.names.clone(),
            self.selectors.clone(),
            self.operators.clone(),
        )
        .with_type_metadata(
            self.symbols.manifest(),
            self.signatures.clone(),
            self.static_members.clone(),
        )
    }

    pub fn call(&self, context: &mut C, call: &BuiltinCall) -> NativeResult {
        if let Some(function) = self.raw_functions.get(&call.builtin) {
            return function(context, call);
        }
        let function = self
            .functions
            .get(&call.builtin)
            .ok_or(NativeError::UnknownBuiltin(call.builtin))?;
        let values = call
            .receiver
            .iter()
            .cloned()
            .chain(call.arguments.iter().map(|argument| argument.value.clone()))
            .collect::<Vec<_>>();
        function(context, &values)
    }

    fn name_for_id(&self, id: BuiltinId) -> Option<String> {
        self.names
            .iter()
            .find_map(|(name, existing)| (*existing == id).then_some(name.clone()))
            .or_else(|| {
                self.selectors
                    .iter()
                    .find_map(|((selector, method), existing)| {
                        (*existing == id).then_some(format!("{selector}.{method}"))
                    })
            })
            .or_else(|| {
                self.operators.iter().find_map(|(operator, existing)| {
                    (*existing == id).then_some(format!("operator {operator}"))
                })
            })
            .or_else(|| {
                self.static_members.iter().find_map(|member| {
                    (member.builtin == id).then(|| {
                        format!(
                            "{}.{}",
                            self.symbols.resolve(member.owner).unwrap_or("<unknown>"),
                            self.symbols.resolve(member.name).unwrap_or("<unknown>")
                        )
                    })
                })
            })
    }
}

/// Stable across registration order and process runs. Collisions are rejected by the registry.
pub fn stable_builtin_id(name: &str) -> BuiltinId {
    let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    BuiltinId(hash)
}

pub trait FromHksValue: Sized {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorValue(pub String);

impl FromHksValue for SelectorValue {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::Selector(selector) => Ok(Self(selector.clone())),
            _ => Err(NativeError::TypeMismatch("selector")),
        }
    }
}

pub trait IntoHksValue {
    fn into_hks_value(self) -> Value;
}

impl FromHksValue for Value {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        Ok(value.clone())
    }
}

impl IntoHksValue for Value {
    fn into_hks_value(self) -> Value {
        self
    }
}

impl FromHksValue for String {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::String(value) => Ok(value.clone()),
            _ => Err(NativeError::TypeMismatch("string")),
        }
    }
}

impl IntoHksValue for String {
    fn into_hks_value(self) -> Value {
        Value::String(self)
    }
}

impl FromHksValue for bool {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::Bool(value) => Ok(*value),
            _ => Err(NativeError::TypeMismatch("bool")),
        }
    }
}

impl IntoHksValue for bool {
    fn into_hks_value(self) -> Value {
        Value::Bool(self)
    }
}

impl FromHksValue for f64 {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::Number(value) => Ok(*value),
            _ => Err(NativeError::TypeMismatch("number")),
        }
    }
}

impl IntoHksValue for f64 {
    fn into_hks_value(self) -> Value {
        Value::Number(self)
    }
}

impl IntoHksValue for () {
    fn into_hks_value(self) -> Value {
        Value::Null
    }
}

pub trait IntoNativeFunction<C, Args>: Send + Sync + 'static {
    fn into_native_function(self) -> Box<NativeThunk<C>>;
}

macro_rules! impl_native_function {
    ($count:expr; $( $type:ident $value:ident ),*) => {
        impl<C, F, R, $( $type, )*> IntoNativeFunction<C, ($( $type, )*)> for F
        where
            C: 'static,
            F: Fn(&mut C, $( $type ),*) -> Result<R, NativeError> + Send + Sync + 'static,
            R: IntoHksValue,
            $( $type: FromHksValue, )*
        {
            fn into_native_function(self) -> Box<NativeThunk<C>> {
                Box::new(move |context, arguments| {
                    if arguments.len() != $count {
                        return Err(NativeError::Arity {
                            expected: $count,
                            actual: arguments.len(),
                        });
                    }
                    #[allow(unused_mut, unused_variables)]
                    let mut arguments = arguments.iter();
                    $(
                        let $value = $type::from_hks_value(
                            arguments.next().expect("arity was checked"),
                        )?;
                    )*
                    self(context, $( $value ),*).map(IntoHksValue::into_hks_value)
                })
            }
        }
    };
}

impl_native_function!(0;);
impl_native_function!(1; A a);
impl_native_function!(2; A a, B b);
impl_native_function!(3; A a, B b, D d);
impl_native_function!(4; A a, B b, D d, E e);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeError {
    UnknownBuiltin(BuiltinId),
    Arity { expected: usize, actual: usize },
    TypeMismatch(&'static str),
    Message(String),
}

impl NativeError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBuiltin(id) => write!(formatter, "unknown builtin {id:?}"),
            Self::Arity { expected, actual } => {
                write!(formatter, "expected {expected} arguments, got {actual}")
            }
            Self::TypeMismatch(expected) => write!(formatter, "expected {expected}"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for NativeError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrationError {
    DuplicateName(String),
    UnknownBuiltin(BuiltinId),
    UnknownType(SymbolId),
    GetterHasParameters(String),
    DuplicateId {
        id: BuiltinId,
        existing: String,
        attempted: String,
    },
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName(name) => {
                write!(formatter, "native function `{name}` is already registered")
            }
            Self::UnknownBuiltin(id) => write!(formatter, "builtin {id:?} is not registered"),
            Self::UnknownType(id) => write!(formatter, "script type {id:?} is not registered"),
            Self::GetterHasParameters(name) => {
                write!(formatter, "getter `{name}` cannot declare parameters")
            }
            Self::DuplicateId {
                id,
                existing,
                attempted,
            } => write!(
                formatter,
                "builtin ID {id:?} is already used by `{existing}` and cannot be assigned to `{attempted}`"
            ),
        }
    }
}

impl Error for RegistrationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        parse_program,
        vm::{ScriptType, StaticMemberKind, compile_with_manifest},
    };

    #[derive(Default)]
    struct Context {
        calls: usize,
    }

    fn greet(context: &mut Context, name: String) -> Result<String, NativeError> {
        context.calls += 1;
        Ok(format!("hello {name}"))
    }

    fn selector_zoom(
        context: &mut Context,
        selector: SelectorValue,
        scale: f64,
    ) -> Result<(), NativeError> {
        assert_eq!(selector.0, "camera");
        assert_eq!(scale, 1.2);
        context.calls += 1;
        Ok(())
    }

    #[test]
    fn registers_a_typed_rust_function_and_dispatches_compiled_calls() {
        let mut registry = NativeRegistry::new();
        let id = registry.register_fn("greet", greet).unwrap();
        let bytecode = compile_with_manifest(
            &parse_program(r#"greet("HKS")"#).unwrap(),
            42,
            &registry.manifest(),
        )
        .unwrap();
        let crate::vm::Instruction::CallBuiltin { builtin, .. } = &bytecode.instructions[1] else {
            panic!("expected builtin call")
        };
        assert_eq!(*builtin, id);

        let mut context = Context::default();
        let result = registry
            .call(
                &mut context,
                &BuiltinCall {
                    builtin: id,
                    receiver: None,
                    arguments: vec![crate::vm::CallArgument {
                        label: None,
                        value: Value::String("HKS".to_string()),
                    }],
                },
            )
            .unwrap();
        assert_eq!(result, Value::String("hello HKS".to_string()));
        assert_eq!(context.calls, 1);
    }

    #[test]
    fn selector_methods_preserve_a_typed_receiver_in_bytecode_calls() {
        let mut registry = NativeRegistry::<Context>::new();
        let id = BuiltinId(77);
        registry
            .register_selector_raw_fn_with_id(id, "camera", "zoom", |_context, call| {
                assert_eq!(call.receiver, Some(Value::Selector("camera".to_string())));
                Ok(Value::Null)
            })
            .expect("selector method registration must succeed");
        let bytecode = compile_with_manifest(
            &parse_program("camera.zoom(1.2)").expect("selector call must parse"),
            43,
            &registry.manifest(),
        )
        .expect("selector call must compile");
        assert!(matches!(
            bytecode.instructions.first(),
            Some(crate::vm::Instruction::Constant(Value::Selector(selector)))
                if selector == "camera"
        ));
        let mut vm = crate::vm::Vm::new(bytecode).expect("selector VM must initialize");
        let Some(crate::vm::VmEvent::Call(call)) =
            vm.step().expect("selector VM step must succeed")
        else {
            panic!("expected selector builtin call")
        };
        registry
            .call(&mut Context::default(), &call)
            .expect("selector call must dispatch with its receiver");
    }

    #[test]
    fn selector_methods_can_register_typed_rust_functions() {
        let mut registry = NativeRegistry::<Context>::new();
        registry
            .register_selector_fn_with_id(BuiltinId(78), "camera", "zoom", selector_zoom)
            .expect("typed selector method registration must succeed");
        let bytecode = compile_with_manifest(
            &parse_program("camera.zoom(1.2)").expect("selector call must parse"),
            44,
            &registry.manifest(),
        )
        .expect("typed selector call must compile");
        let mut vm = crate::vm::Vm::new(bytecode).expect("selector VM must initialize");
        let Some(crate::vm::VmEvent::Call(call)) =
            vm.step().expect("selector VM step must succeed")
        else {
            panic!("expected selector builtin call")
        };
        let mut context = Context::default();
        registry
            .call(&mut context, &call)
            .expect("typed selector call must dispatch");
        assert_eq!(context.calls, 1);
    }

    #[test]
    fn name_ids_do_not_depend_on_registration_order() {
        let mut first = NativeRegistry::<Context>::new();
        let a = first.register_fn("greet", greet).unwrap();
        let mut second = NativeRegistry::<Context>::new();
        second
            .register_fn("other", |_: &mut Context| Ok(()))
            .unwrap();
        let b = second.register_fn("greet", greet).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn operators_are_embedding_registered_native_functions() {
        let mut registry = NativeRegistry::<Context>::new();
        registry
            .register_operator_fn_with_id(
                BuiltinId(90),
                ":",
                |context: &mut Context, speaker: Value, text: String| {
                    assert_eq!(speaker, Value::Ellipsis);
                    assert_eq!(text, "continued");
                    context.calls += 1;
                    Ok(())
                },
            )
            .expect("operator registration must succeed");
        let bytecode = compile_with_manifest(
            &parse_program(r#"...: "continued""#).expect("operator expression must parse"),
            45,
            &registry.manifest(),
        )
        .expect("registered operator must compile");
        let mut vm = crate::vm::Vm::new(bytecode).expect("VM must initialize");
        let Some(crate::vm::VmEvent::Call(call)) = vm.step().expect("VM must yield") else {
            panic!("expected operator native call")
        };
        let mut context = Context::default();
        registry
            .call(&mut context, &call)
            .expect("operator must dispatch through registry");
        assert_eq!(context.calls, 1);
    }

    #[test]
    fn registered_types_expose_static_methods_and_getters() {
        let mut registry = NativeRegistry::<Context>::new();
        let position = registry.define_type("Position");
        registry
            .register_static_raw_fn_with_id(
                BuiltinId(100),
                position,
                "rel",
                FunctionSignature {
                    receiver: None,
                    parameters: vec![ScriptType::Number, ScriptType::Number],
                    result: ScriptType::Named(position),
                },
                StaticMemberKind::Method,
                |_context, call| {
                    Ok(Value::Tuple(
                        call.arguments
                            .iter()
                            .map(|argument| argument.value.clone())
                            .collect(),
                    ))
                },
            )
            .expect("Position.rel must register");
        registry
            .register_static_raw_fn_with_id(
                BuiltinId(101),
                position,
                "left",
                FunctionSignature {
                    receiver: None,
                    parameters: Vec::new(),
                    result: ScriptType::Named(position),
                },
                StaticMemberKind::Getter,
                |_context, _call| Ok(Value::Symbol("left".to_string())),
            )
            .expect("Position.left must register");

        let manifest = registry.manifest();
        assert_eq!(manifest.symbols().resolve(position), Some("Position"));
        let bytecode = compile_with_manifest(
            &parse_program("let a = .rel(1, 12)\nlet b = .left")
                .expect("static member syntax must parse"),
            46,
            &manifest,
        )
        .expect("registered static members must compile");
        assert!(bytecode.instructions.iter().any(|instruction| matches!(
            instruction,
            crate::vm::Instruction::CallBuiltin {
                builtin: BuiltinId(100),
                has_receiver: false,
                ..
            }
        )));
        assert!(bytecode.instructions.iter().any(|instruction| matches!(
            instruction,
            crate::vm::Instruction::CallBuiltin {
                builtin: BuiltinId(101),
                has_receiver: false,
                ..
            }
        )));
    }

    #[test]
    fn static_method_signatures_are_checked_during_compilation() {
        let mut registry = NativeRegistry::<Context>::new();
        let position = registry.define_type("Position");
        registry
            .register_static_raw_fn_with_id(
                BuiltinId(102),
                position,
                "rel",
                FunctionSignature {
                    receiver: None,
                    parameters: vec![ScriptType::Number, ScriptType::Number],
                    result: ScriptType::Named(position),
                },
                StaticMemberKind::Method,
                |_context, _call| Ok(Value::Null),
            )
            .expect("Position.rel must register");
        let errors = compile_with_manifest(
            &parse_program(r#".rel("wrong", 12)"#).expect("call must parse"),
            47,
            &registry.manifest(),
        )
        .expect_err("argument type mismatch must fail compilation");
        assert!(errors[0].message.contains("expects Number"));
    }
}
