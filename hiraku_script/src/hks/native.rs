//! Typed registration and dispatch for embedding-owned HKS functions.
//!
//! Registries contain Rust functions and deliberately are not serializable. Bytecode and
//! snapshots only retain [`BuiltinId`] values plus the manifest hash.

use std::{collections::BTreeMap, error::Error, fmt};

use super::vm::{BuiltinCall, BuiltinId, BuiltinManifest, Value};

type NativeResult = Result<Value, NativeError>;
type NativeThunk<C> = dyn Fn(&mut C, &[Value]) -> NativeResult + Send + Sync + 'static;
type RawNativeThunk<C> = dyn Fn(&mut C, &BuiltinCall) -> NativeResult + Send + Sync + 'static;

pub struct NativeRegistry<C> {
    names: BTreeMap<String, BuiltinId>,
    functions: BTreeMap<BuiltinId, Box<NativeThunk<C>>>,
    raw_functions: BTreeMap<BuiltinId, Box<RawNativeThunk<C>>>,
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
            functions: BTreeMap::new(),
            raw_functions: BTreeMap::new(),
        }
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
        if let Some(existing) = self
            .names
            .iter()
            .find_map(|(name, existing)| (*existing == id).then_some(name.clone()))
        {
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
        if let Some(existing) = self
            .names
            .iter()
            .find_map(|(name, existing)| (*existing == id).then_some(name.clone()))
        {
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

    pub fn manifest(&self) -> BuiltinManifest {
        BuiltinManifest::new(self.names.iter().map(|(name, id)| (name.clone(), *id)))
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
            .arguments
            .iter()
            .map(|argument| argument.value.clone())
            .collect::<Vec<_>>();
        function(context, &values)
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
    use crate::hks::{parse_program, vm::compile_with_manifest};

    #[derive(Default)]
    struct Context {
        calls: usize,
    }

    fn greet(context: &mut Context, name: String) -> Result<String, NativeError> {
        context.calls += 1;
        Ok(format!("hello {name}"))
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
        let super::super::vm::Instruction::CallBuiltin { builtin, .. } = &bytecode.instructions[1]
        else {
            panic!("expected builtin call")
        };
        assert_eq!(*builtin, id);

        let mut context = Context::default();
        let result = registry
            .call(
                &mut context,
                &BuiltinCall {
                    builtin: id,
                    arguments: vec![super::super::vm::CallArgument {
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
}
