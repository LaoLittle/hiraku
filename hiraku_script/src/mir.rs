//! Owned control-flow IR between arena HIR and executable bytecode.

use crate::{
    BinaryOp, HirBlock, HirExpr, HirExprKind, HirLiteral, HirPlace, HirProgram, HirStmtKind,
    NumberUnit, ResolvedFunction, Span, SymbolId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualRegister(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MirBlockId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct MirProgram {
    pub entry: MirFunction,
    pub functions: Vec<MirFunction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MirFunction {
    pub blocks: Vec<MirBasicBlock>,
    pub regions: Vec<MirFunction>,
    pub parameters: Vec<crate::HirLocalId>,
    pub virtual_register_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MirBasicBlock {
    pub instructions: Vec<MirInstruction>,
    pub terminator: MirTerminator,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MirInstruction {
    Constant {
        dst: VirtualRegister,
        value: MirConstant,
    },
    MakeClosure {
        dst: VirtualRegister,
        region: u32,
    },
    LoadLocal {
        dst: VirtualRegister,
        local: crate::HirLocalId,
    },
    StoreLocal {
        local: crate::HirLocalId,
        src: VirtualRegister,
    },
    LoadGlobal {
        dst: VirtualRegister,
        global: crate::HirGlobalId,
    },
    StoreGlobal {
        global: crate::HirGlobalId,
        src: VirtualRegister,
    },
    GetMember {
        dst: VirtualRegister,
        object: VirtualRegister,
        member: SymbolId,
        safe: bool,
    },
    SetMember {
        dst: VirtualRegister,
        object: VirtualRegister,
        member: SymbolId,
        value: VirtualRegister,
    },
    UnaryMinus {
        dst: VirtualRegister,
        value: VirtualRegister,
    },
    Binary {
        dst: VirtualRegister,
        op: BinaryOp,
        left: VirtualRegister,
        right: VirtualRegister,
    },
    MakeTuple {
        dst: VirtualRegister,
        values: Vec<VirtualRegister>,
    },
    MakeList {
        dst: VirtualRegister,
        values: Vec<VirtualRegister>,
    },
    MakeMap {
        dst: VirtualRegister,
        fields: Vec<(SymbolId, VirtualRegister)>,
    },
    Call {
        dst: VirtualRegister,
        function: ResolvedFunction,
        receiver: Option<VirtualRegister>,
        dynamic_callee: Option<VirtualRegister>,
        arguments: Vec<(Option<SymbolId>, VirtualRegister)>,
    },
    AssertNonNull {
        dst: VirtualRegister,
        value: VirtualRegister,
    },
    SelectNonNull {
        dst: VirtualRegister,
        value: VirtualRegister,
        fallback: VirtualRegister,
    },
    Statement {
        value: VirtualRegister,
        string: bool,
        emit_value: bool,
    },
}

impl MirInstruction {
    pub fn defined_register(&self) -> Option<VirtualRegister> {
        match self {
            Self::Constant { dst, .. }
            | Self::MakeClosure { dst, .. }
            | Self::LoadLocal { dst, .. }
            | Self::LoadGlobal { dst, .. }
            | Self::GetMember { dst, .. }
            | Self::SetMember { dst, .. }
            | Self::UnaryMinus { dst, .. }
            | Self::Binary { dst, .. }
            | Self::MakeTuple { dst, .. }
            | Self::MakeList { dst, .. }
            | Self::MakeMap { dst, .. }
            | Self::Call { dst, .. }
            | Self::AssertNonNull { dst, .. }
            | Self::SelectNonNull { dst, .. } => Some(*dst),
            Self::StoreLocal { .. } | Self::StoreGlobal { .. } | Self::Statement { .. } => None,
        }
    }

    pub fn used_registers(&self) -> Vec<VirtualRegister> {
        match self {
            Self::Constant { .. }
            | Self::MakeClosure { .. }
            | Self::LoadLocal { .. }
            | Self::LoadGlobal { .. } => Vec::new(),
            Self::StoreLocal { src, .. } | Self::StoreGlobal { src, .. } => vec![*src],
            Self::GetMember { object, .. } => vec![*object],
            Self::SetMember { object, value, .. } => vec![*object, *value],
            Self::UnaryMinus { value, .. } | Self::AssertNonNull { value, .. } => vec![*value],
            Self::Binary { left, right, .. } => vec![*left, *right],
            Self::MakeTuple { values, .. } | Self::MakeList { values, .. } => values.clone(),
            Self::MakeMap { fields, .. } => fields.iter().map(|(_, value)| *value).collect(),
            Self::Call {
                receiver,
                dynamic_callee,
                arguments,
                ..
            } => receiver
                .iter()
                .copied()
                .chain(dynamic_callee.iter().copied())
                .chain(arguments.iter().map(|(_, value)| *value))
                .collect(),
            Self::SelectNonNull {
                value, fallback, ..
            } => vec![*value, *fallback],
            Self::Statement { value, .. } => vec![*value],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MirConstant {
    Null,
    Ellipsis,
    Bool(bool),
    Number(f64),
    Percent(f64),
    String(String),
    Symbol(SymbolId),
    Selector(SymbolId),
    Function(ResolvedFunction),
    Unit,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MirTerminator {
    Jump(MirBlockId),
    Branch {
        condition: VirtualRegister,
        then_block: MirBlockId,
        else_block: MirBlockId,
    },
    Return(Option<VirtualRegister>),
    Halt,
    Unset,
}

impl MirTerminator {
    pub fn successors(&self) -> Vec<MirBlockId> {
        match self {
            Self::Jump(target) => vec![*target],
            Self::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Self::Return(_) | Self::Halt | Self::Unset => Vec::new(),
        }
    }

    pub fn used_registers(&self) -> Vec<VirtualRegister> {
        match self {
            Self::Branch { condition, .. } => vec![*condition],
            Self::Return(Some(value)) => vec![*value],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirLoweringError {
    pub message: String,
    pub span: Span,
}

pub fn lower_hir_to_mir(hir: &HirProgram<'_>) -> Result<MirProgram, Vec<MirLoweringError>> {
    let mut errors = Vec::new();
    let entry = MirBuilder::lower(hir.entry, false, Vec::new(), &mut errors);
    let functions = hir
        .functions
        .iter()
        .map(|function| MirBuilder::lower(function.body, true, Vec::new(), &mut errors))
        .collect();
    if errors.is_empty() {
        Ok(MirProgram { entry, functions })
    } else {
        Err(errors)
    }
}

struct MirBuilder {
    blocks: Vec<MirBasicBlock>,
    regions: Vec<MirFunction>,
    current: MirBlockId,
    next_register: u32,
}

impl MirBuilder {
    fn lower(
        block: &HirBlock<'_>,
        function: bool,
        parameters: Vec<crate::HirLocalId>,
        errors: &mut Vec<MirLoweringError>,
    ) -> MirFunction {
        let mut builder = Self {
            blocks: vec![MirBasicBlock {
                instructions: Vec::new(),
                terminator: MirTerminator::Unset,
            }],
            regions: Vec::new(),
            current: MirBlockId(0),
            next_register: 0,
        };
        let result = builder.lower_block(block, errors);
        if matches!(builder.current_block().terminator, MirTerminator::Unset) {
            builder.current_block_mut().terminator = if function {
                MirTerminator::Return(result)
            } else {
                MirTerminator::Halt
            };
        }
        MirFunction {
            blocks: builder.blocks,
            regions: builder.regions,
            parameters,
            virtual_register_count: builder.next_register,
        }
    }

    fn lower_block(
        &mut self,
        block: &HirBlock<'_>,
        errors: &mut Vec<MirLoweringError>,
    ) -> Option<VirtualRegister> {
        let mut result = None;
        for statement in block.statements {
            result = self.lower_statement(statement, errors);
        }
        result
    }

    fn lower_statement(
        &mut self,
        statement: &crate::HirStmt<'_>,
        errors: &mut Vec<MirLoweringError>,
    ) -> Option<VirtualRegister> {
        match statement.kind {
            HirStmtKind::Let { local, value } => {
                let value = self.lower_expression(value, errors)?;
                self.push(MirInstruction::StoreLocal { local, src: value });
                self.push(MirInstruction::Statement {
                    value,
                    string: false,
                    emit_value: false,
                });
                Some(value)
            }
            HirStmtKind::Global { global, value } => {
                let value = match value {
                    Some(value) => self.lower_expression(value, errors)?,
                    None => self.constant(MirConstant::Null),
                };
                self.push(MirInstruction::StoreGlobal { global, src: value });
                self.push(MirInstruction::Statement {
                    value,
                    string: false,
                    emit_value: false,
                });
                Some(value)
            }
            HirStmtKind::Assign { target, value } => {
                let value = self.lower_expression(value, errors)?;
                self.store_place(target, value);
                self.push(MirInstruction::Statement {
                    value,
                    string: false,
                    emit_value: false,
                });
                Some(value)
            }
            HirStmtKind::Expr(expression) => {
                let value = self.lower_expression(expression, errors)?;
                self.push(MirInstruction::Statement {
                    value,
                    string: matches!(expression.kind, HirExprKind::Literal(HirLiteral::String(_))),
                    emit_value: true,
                });
                Some(value)
            }
            HirStmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                let condition = self.lower_expression(condition, errors)?;
                let then_id = self.new_block();
                let else_id = self.new_block();
                let join_id = self.new_block();
                self.current_block_mut().terminator = MirTerminator::Branch {
                    condition,
                    then_block: then_id,
                    else_block: else_id,
                };
                self.current = then_id;
                self.lower_block(then_block, errors);
                self.jump_if_unset(join_id);
                self.current = else_id;
                if let Some(else_block) = else_block {
                    self.lower_block(else_block, errors);
                }
                self.jump_if_unset(join_id);
                self.current = join_id;
                None
            }
            HirStmtKind::While { condition, body } => {
                let condition_id = self.new_block();
                let body_id = self.new_block();
                let exit_id = self.new_block();
                self.jump_if_unset(condition_id);
                self.current = condition_id;
                let condition = self.lower_expression(condition, errors)?;
                self.current_block_mut().terminator = MirTerminator::Branch {
                    condition,
                    then_block: body_id,
                    else_block: exit_id,
                };
                self.current = body_id;
                self.lower_block(body, errors);
                self.jump_if_unset(condition_id);
                self.current = exit_id;
                None
            }
        }
    }

    fn lower_expression(
        &mut self,
        expression: &HirExpr<'_>,
        errors: &mut Vec<MirLoweringError>,
    ) -> Option<VirtualRegister> {
        match expression.kind {
            HirExprKind::Literal(literal) => Some(self.constant(match literal {
                HirLiteral::Unit => MirConstant::Unit,
                HirLiteral::Null => MirConstant::Null,
                HirLiteral::Ellipsis => MirConstant::Ellipsis,
                HirLiteral::Bool(value) => MirConstant::Bool(value),
                HirLiteral::Number {
                    value,
                    unit: NumberUnit::Scalar,
                } => MirConstant::Number(value),
                HirLiteral::Number {
                    value,
                    unit: NumberUnit::Percent,
                } => MirConstant::Percent(value),
                HirLiteral::String(value) => MirConstant::String(value.to_string()),
            })),
            HirExprKind::Local(local) => {
                let dst = self.register();
                self.push(MirInstruction::LoadLocal { dst, local });
                Some(dst)
            }
            HirExprKind::Global(global) => {
                let dst = self.register();
                self.push(MirInstruction::LoadGlobal { dst, global });
                Some(dst)
            }
            HirExprKind::Symbol(symbol) => Some(self.constant(MirConstant::Symbol(symbol))),
            HirExprKind::Selector(selector) => Some(self.constant(MirConstant::Selector(selector))),
            HirExprKind::UnaryMinus(value) => {
                let value = self.lower_expression(value, errors)?;
                let dst = self.register();
                self.push(MirInstruction::UnaryMinus { dst, value });
                Some(dst)
            }
            HirExprKind::Member {
                object,
                member,
                safe,
            } => {
                let object = self.lower_expression(object, errors)?;
                let dst = self.register();
                self.push(MirInstruction::GetMember {
                    dst,
                    object,
                    member,
                    safe,
                });
                Some(dst)
            }
            HirExprKind::Elvis { value, fallback } => {
                let value = self.lower_expression(value, errors)?;
                let fallback = self.lower_expression(fallback, errors)?;
                let dst = self.register();
                self.push(MirInstruction::SelectNonNull {
                    dst,
                    value,
                    fallback,
                });
                Some(dst)
            }
            HirExprKind::NonNull(value) => {
                let value = self.lower_expression(value, errors)?;
                let dst = self.register();
                self.push(MirInstruction::AssertNonNull { dst, value });
                Some(dst)
            }
            HirExprKind::Call {
                callee,
                arguments,
                function,
            } => {
                let dynamic_callee = matches!(function, ResolvedFunction::Dynamic)
                    .then(|| self.lower_expression(callee, errors))
                    .flatten();
                let receiver = match callee.kind {
                    HirExprKind::Member { object, .. }
                        if matches!(
                            function,
                            ResolvedFunction::Builtin(_) | ResolvedFunction::External(_)
                        ) =>
                    {
                        self.lower_expression(object, errors)
                    }
                    _ => None,
                };
                let arguments = arguments
                    .iter()
                    .map(|argument| {
                        self.lower_expression(argument.value, errors)
                            .map(|value| (argument.label, value))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let dst = self.register();
                self.push(MirInstruction::Call {
                    dst,
                    function,
                    receiver,
                    dynamic_callee,
                    arguments,
                });
                Some(dst)
            }
            HirExprKind::Tuple(values) | HirExprKind::List(values) => {
                let values = values
                    .iter()
                    .map(|value| self.lower_expression(value, errors))
                    .collect::<Option<Vec<_>>>()?;
                let dst = self.register();
                if matches!(expression.kind, HirExprKind::Tuple(_)) {
                    self.push(MirInstruction::MakeTuple { dst, values });
                } else {
                    self.push(MirInstruction::MakeList { dst, values });
                }
                Some(dst)
            }
            HirExprKind::Map { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|(name, value)| {
                        self.lower_expression(value, errors)
                            .map(|value| (*name, value))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let dst = self.register();
                self.push(MirInstruction::MakeMap { dst, fields });
                Some(dst)
            }
            HirExprKind::Binary { left, op, right } => {
                let left = self.lower_expression(left, errors)?;
                let right = self.lower_expression(right, errors)?;
                let dst = self.register();
                self.push(MirInstruction::Binary {
                    dst,
                    op,
                    left,
                    right,
                });
                Some(dst)
            }
            HirExprKind::Block(block) => {
                let region = Self::lower(block, true, Vec::new(), errors);
                let region_id = self.regions.len() as u32;
                self.regions.push(region);
                let dst = self.register();
                self.push(MirInstruction::MakeClosure {
                    dst,
                    region: region_id,
                });
                Some(dst)
            }
            HirExprKind::Lambda { parameters, body } => {
                let region = Self::lower(body, true, parameters.to_vec(), errors);
                let region_id = self.regions.len() as u32;
                self.regions.push(region);
                let dst = self.register();
                self.push(MirInstruction::MakeClosure {
                    dst,
                    region: region_id,
                });
                Some(dst)
            }
            HirExprKind::Function(function) => {
                Some(self.constant(MirConstant::Function(ResolvedFunction::User(function))))
            }
            HirExprKind::Builtin(builtin) => {
                Some(self.constant(MirConstant::Function(ResolvedFunction::Builtin(builtin))))
            }
            HirExprKind::Unresolved(symbol) => {
                Some(self.constant(MirConstant::Function(ResolvedFunction::External(symbol))))
            }
        }
    }

    fn load_place(&mut self, place: &HirPlace<'_>) -> VirtualRegister {
        match *place {
            HirPlace::Local(local) => {
                let dst = self.register();
                self.push(MirInstruction::LoadLocal { dst, local });
                dst
            }
            HirPlace::Global(global) => {
                let dst = self.register();
                self.push(MirInstruction::LoadGlobal { dst, global });
                dst
            }
            HirPlace::Member { object, member } => {
                let object = self.load_place(object);
                let dst = self.register();
                self.push(MirInstruction::GetMember {
                    dst,
                    object,
                    member,
                    safe: false,
                });
                dst
            }
        }
    }

    fn store_place(&mut self, place: &HirPlace<'_>, value: VirtualRegister) {
        match *place {
            HirPlace::Local(local) => self.push(MirInstruction::StoreLocal { local, src: value }),
            HirPlace::Global(global) => {
                self.push(MirInstruction::StoreGlobal { global, src: value })
            }
            HirPlace::Member { object, member } => {
                let object_value = self.load_place(object);
                let updated = self.register();
                self.push(MirInstruction::SetMember {
                    dst: updated,
                    object: object_value,
                    member,
                    value,
                });
                self.store_place(object, updated);
            }
        }
    }

    fn constant(&mut self, value: MirConstant) -> VirtualRegister {
        let dst = self.register();
        self.push(MirInstruction::Constant { dst, value });
        dst
    }

    fn register(&mut self) -> VirtualRegister {
        let register = VirtualRegister(self.next_register);
        self.next_register += 1;
        register
    }

    fn push(&mut self, instruction: MirInstruction) {
        self.current_block_mut().instructions.push(instruction);
    }

    fn new_block(&mut self) -> MirBlockId {
        let id = MirBlockId(self.blocks.len() as u32);
        self.blocks.push(MirBasicBlock {
            instructions: Vec::new(),
            terminator: MirTerminator::Unset,
        });
        id
    }

    fn current_block(&self) -> &MirBasicBlock {
        &self.blocks[self.current.0 as usize]
    }

    fn current_block_mut(&mut self) -> &mut MirBasicBlock {
        &mut self.blocks[self.current.0 as usize]
    }

    fn jump_if_unset(&mut self, target: MirBlockId) {
        if matches!(self.current_block().terminator, MirTerminator::Unset) {
            self.current_block_mut().terminator = MirTerminator::Jump(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{HirArena, lower_to_hir, parse_program};

    use super::*;

    #[test]
    fn lowers_if_and_while_into_a_control_flow_graph() {
        let syntax =
            parse_program("let a = 0\nwhile a < 2 { if a == 1 { a += 1 } else { a += 1 } }")
                .expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("HIR lowers");
        let mir = lower_hir_to_mir(&hir).expect("MIR lowers");
        assert!(mir.entry.blocks.len() >= 7);
        assert!(
            mir.entry
                .blocks
                .iter()
                .any(|block| matches!(block.terminator, MirTerminator::Branch { .. }))
        );
    }

    #[test]
    fn recursive_places_become_get_set_member_instructions() {
        let syntax =
            parse_program("let player = .{ stats: .{ health: 1 } }\nplayer.stats.health = 2")
                .expect("source parses");
        let arena = HirArena::new();
        let hir = lower_to_hir(&arena, &syntax, None).expect("HIR lowers");
        let mir = lower_hir_to_mir(&hir).expect("MIR lowers");
        let instructions = &mir.entry.blocks[0].instructions;
        assert_eq!(
            instructions
                .iter()
                .filter(|instruction| matches!(instruction, MirInstruction::SetMember { .. }))
                .count(),
            2
        );
    }
}
