//! Typed registration and dispatch for embedding-owned HKS functions.
//!
//! Registries contain Rust functions and deliberately are not serializable. Bytecode and
//! snapshots only retain [`BuiltinId`] values plus the manifest hash.

use std::{collections::BTreeMap, error::Error, fmt};

use crate::{
    runtime::{
        BuiltinCall, BuiltinId, BuiltinManifest, FunctionSignature, StaticMember, StaticMemberKind,
        Value,
    },
    symbol::{SymbolId, SymbolInterner},
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
    globals: BTreeMap<String, crate::ScriptType>,
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
            globals: BTreeMap::new(),
        }
    }

    pub fn define_type(&mut self, name: impl Into<String>) -> SymbolId {
        self.symbols.intern(name)
    }

    pub fn define_global(
        &mut self,
        name: impl Into<String>,
        ty: crate::ScriptType,
    ) -> Result<(), RegistrationError> {
        let name = name.into();
        if self.globals.insert(name.clone(), ty).is_some() {
            return Err(RegistrationError::DuplicateName(name));
        }
        Ok(())
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

    pub fn set_signature_for(
        &mut self,
        name: &str,
        signature: FunctionSignature,
    ) -> Result<(), RegistrationError> {
        let builtin = self
            .names
            .get(name)
            .copied()
            .ok_or_else(|| RegistrationError::UnknownName(name.to_string()))?;
        self.set_signature(builtin, signature)
    }

    pub fn register_raw_fn<F>(
        &mut self,
        name: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let name = name.into();
        let id = stable_builtin_id(&name);
        self.register_raw_fn_with_id(id, name, function)?;
        Ok(id)
    }

    pub fn register_selector_raw_fn<F>(
        &mut self,
        selector: impl Into<String>,
        method: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let selector = selector.into();
        let method = method.into();
        let id = stable_builtin_id(&format!("{selector}.{method}"));
        self.register_selector_raw_fn_with_id(id, selector, method, function)?;
        Ok(id)
    }

    pub fn register_selector_fn<F, Args>(
        &mut self,
        selector: impl Into<String>,
        method: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let selector = selector.into();
        let method = method.into();
        let id = stable_builtin_id(&format!("{selector}.{method}"));
        self.register_selector_fn_with_id(id, selector, method, function)?;
        Ok(id)
    }

    pub fn register_operator_raw_fn<F>(
        &mut self,
        operator: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let operator = operator.into();
        let id = stable_builtin_id(&format!("operator {operator}"));
        self.register_operator_raw_fn_with_id(id, operator, function)?;
        Ok(id)
    }

    pub fn register_operator_fn<F, Args>(
        &mut self,
        operator: impl Into<String>,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: IntoNativeFunction<C, Args>,
    {
        let operator = operator.into();
        let id = stable_builtin_id(&format!("operator {operator}"));
        self.register_operator_fn_with_id(id, operator, function)?;
        Ok(id)
    }

    pub fn register_static_raw_fn<F>(
        &mut self,
        owner: SymbolId,
        name: impl Into<String>,
        signature: FunctionSignature,
        kind: StaticMemberKind,
        function: F,
    ) -> Result<BuiltinId, RegistrationError>
    where
        F: Fn(&mut C, &BuiltinCall) -> Result<Value, NativeError> + Send + Sync + 'static,
    {
        let name = name.into();
        let owner_name = self
            .symbols
            .resolve(owner)
            .ok_or(RegistrationError::UnknownType(owner))?;
        let id = stable_builtin_id(&format!("{owner_name}.{name}"));
        self.register_static_raw_fn_with_id(id, owner, name, signature, kind, function)?;
        Ok(id)
    }

    fn register_static_raw_fn_with_id<F>(
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

    fn register_fn_with_id<F, Args>(
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
    fn register_raw_fn_with_id<F>(
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

    fn register_selector_raw_fn_with_id<F>(
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

    fn register_selector_fn_with_id<F, Args>(
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

    fn register_operator_fn_with_id<F, Args>(
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

    fn register_operator_raw_fn_with_id<F>(
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
        .with_globals(self.globals.clone())
    }

    pub fn call(&self, context: &mut C, call: &BuiltinCall) -> NativeResult {
        if let Some(function) = self.raw_functions.get(&call.builtin) {
            return function(context, call);
        }
        let function = self
            .functions
            .get(&call.builtin)
            .ok_or(NativeError::UnknownBuiltin(call.builtin))?;
        let include_receiver = self
            .signatures
            .get(&call.builtin)
            .is_none_or(|signature| signature.receiver.is_some());
        let mut values = call
            .receiver
            .iter()
            .filter(|_| include_receiver)
            .cloned()
            .chain(call.arguments.iter().map(|argument| argument.value.clone()))
            .collect::<Vec<_>>();
        if let Some(signature) = self.signatures.get(&call.builtin) {
            let expected = signature.parameters.len() + usize::from(signature.receiver.is_some());
            while values.len() < expected {
                values.push(Value::Null);
            }
        }
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

/// A save-safe script closure passed to embedding functions.
///
/// It contains bytecode region identifiers and captured script values; no Rust
/// function pointer or host address enters [`Value`]. The embedding decides
/// when and how to schedule it.
#[derive(Clone, Debug, PartialEq)]
pub struct HksClosure {
    pub module: Option<u32>,
    pub region: u32,
    pub captures: Vec<Value>,
}

impl FromHksValue for HksClosure {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::Closure {
                module,
                region,
                captures,
            } => Ok(Self {
                module: *module,
                region: *region,
                captures: captures.clone(),
            }),
            _ => Err(NativeError::TypeMismatch("function")),
        }
    }
}

impl IntoHksValue for HksClosure {
    fn into_hks_value(self) -> Value {
        Value::Closure {
            module: self.module,
            region: self.region,
            captures: self.captures,
        }
    }
}

impl HksScriptType for HksClosure {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Function
    }
}

pub trait IntoHksValue {
    fn into_hks_value(self) -> Value;
}

/// Maps a Rust native API type to the compiler-visible HKS type.
pub trait HksScriptType {
    fn hks_script_type<C>(registry: &mut NativeRegistry<C>) -> crate::ScriptType;
}

pub trait HksNativeType: Sized {
    const HKS_TYPE_NAME: &'static str;

    fn encode_hks_payload(self) -> Value;
    fn decode_hks_payload(value: &Value) -> Result<Self, NativeError>;

    fn into_hks_typed(self, type_id: SymbolId) -> Value {
        Value::Typed {
            type_id,
            value: Box::new(self.encode_hks_payload()),
        }
    }
}

impl FromHksValue for Value {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        Ok(value.clone())
    }
}

impl HksScriptType for Value {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Any
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

impl HksScriptType for String {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::String
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

impl HksScriptType for bool {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Bool
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

impl HksScriptType for f64 {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Number
    }
}

impl FromHksValue for f32 {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        f64::from_hks_value(value).map(|value| value as f32)
    }
}

impl IntoHksValue for f32 {
    fn into_hks_value(self) -> Value {
        Value::Number(f64::from(self))
    }
}

impl HksScriptType for f32 {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Number
    }
}

macro_rules! impl_integer_value {
    ($( $type:ty ),* $(,)?) => {
        $(
            impl FromHksValue for $type {
                fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
                    let Value::Number(value) = value else {
                        return Err(NativeError::TypeMismatch("integer"));
                    };
                    if !value.is_finite() || value.fract() != 0.0
                        || *value < <$type>::MIN as f64
                        || *value > <$type>::MAX as f64
                    {
                        return Err(NativeError::message(format!(
                            "number {value} is outside the range of {}",
                            stringify!($type),
                        )));
                    }
                    Ok(*value as $type)
                }
            }

            impl IntoHksValue for $type {
                fn into_hks_value(self) -> Value {
                    Value::Number(self as f64)
                }
            }

            impl HksScriptType for $type {
                fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
                    crate::ScriptType::Int
                }
            }
        )*
    };
}

impl_integer_value!(u8, u16, u32, i8, i16, i32);

impl IntoHksValue for () {
    fn into_hks_value(self) -> Value {
        Value::Null
    }
}

impl HksScriptType for () {
    fn hks_script_type<C>(_registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Unit
    }
}

impl<T: HksScriptType> HksScriptType for Option<T> {
    fn hks_script_type<C>(registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::Nullable(Box::new(T::hks_script_type(registry)))
    }
}

impl<T: FromHksValue> FromHksValue for Option<T> {
    fn from_hks_value(value: &Value) -> Result<Self, NativeError> {
        match value {
            Value::Null => Ok(None),
            value => T::from_hks_value(value).map(Some),
        }
    }
}

impl<T: HksScriptType> HksScriptType for Vec<T> {
    fn hks_script_type<C>(registry: &mut NativeRegistry<C>) -> crate::ScriptType {
        crate::ScriptType::List(Box::new(T::hks_script_type(registry)))
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
impl_native_function!(5; A a, B b, D d, E e, J j);
impl_native_function!(6; A a, B b, D d, E e, J j, K k);
impl_native_function!(7; A a, B b, D d, E e, J j, K k, L l);
impl_native_function!(8; A a, B b, D d, E e, J j, K k, L l, M m);

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
    UnknownName(String),
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
            Self::UnknownName(name) => write!(formatter, "builtin `{name}` is not registered"),
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
