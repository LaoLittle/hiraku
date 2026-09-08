//! Compiler-assisted declarative UI, independent of the story engine.
mod compiler;
mod document;
mod property;
mod runtime;
pub use compiler::{CompositionPlan, RegionKind, RegionSite, UiCompiler};
pub use document::{UiCompileError, UiDocument, UiInvocationError};
pub use property::{PropertyComputation, PropertyError};
pub use runtime::{CompositionError, compose};
#[cfg(test)]
mod tests;
