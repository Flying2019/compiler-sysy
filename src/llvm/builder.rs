use super::codegen::IrModule;
use super::module::ModuleCtx;
use super::types::LlvmType;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, IntValue, PointerValue,
};
use inkwell::IntPredicate;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct VarInfo<'ctx> {
    pub(crate) ptr: PointerValue<'ctx>,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct Value<'ctx> {
    pub(crate) value: Option<BasicValueEnum<'ctx>>,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct LValue<'ctx> {
    pub(crate) ptr: PointerValue<'ctx>,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct LoopLabels {
    pub(crate) break_label: String,
    pub(crate) continue_label: String,
}

pub(crate) struct FunctionCtx<'a, 'ctx> {
    pub(crate) ir: &'a IrModule<'ctx>,
    pub(crate) module: &'a ModuleCtx,
    pub(crate) function: FunctionValue<'ctx>,
    pub(crate) builder: Builder<'ctx>,
    pub(crate) ret_ty: LlvmType,
    pub(crate) vars: Vec<HashMap<String, VarInfo<'ctx>>>,
    pub(crate) consts: Vec<HashMap<String, i32>>,
    pub(crate) tmp_counter: usize,
    pub(crate) label_counter: usize,
    pub(crate) current_label: String,
    pub(crate) current_block: BasicBlock<'ctx>,
    pub(crate) current_terminated: bool,
    pub(crate) blocks: HashMap<String, BasicBlock<'ctx>>,
    pub(crate) loop_stack: Vec<LoopLabels>,
    pub(crate) is_async: bool,
    pub(crate) promise_ptr: Option<PointerValue<'ctx>>,
}

impl<'a, 'ctx> FunctionCtx<'a, 'ctx> {
    pub(crate) fn new(
        ir: &'a IrModule<'ctx>,
        module: &'a ModuleCtx,
        function: FunctionValue<'ctx>,
        ret_ty: LlvmType,
        is_async: bool,
    ) -> Self {
        let builder = ir.context.create_builder();
        let entry = ir.context.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let mut blocks = HashMap::new();
        blocks.insert("entry".to_string(), entry);
        Self {
            ir,
            module,
            function,
            builder,
            ret_ty,
            vars: vec![HashMap::new()],
            consts: vec![HashMap::new()],
            tmp_counter: 0,
            label_counter: 0,
            current_label: "entry".to_string(),
            current_block: entry,
            current_terminated: false,
            blocks,
            loop_stack: Vec::new(),
            is_async,
            promise_ptr: None,
        }
    }

    pub(crate) fn push_scope(&mut self) {
        self.vars.push(HashMap::new());
        self.consts.push(HashMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.vars.pop();
        self.consts.pop();
    }

    pub(crate) fn insert_var(&mut self, name: String, info: VarInfo<'ctx>) -> Result<(), String> {
        self.vars
            .last_mut()
            .ok_or_else(|| "No active variable scope".to_string())?
            .insert(name, info);
        Ok(())
    }

    pub(crate) fn lookup_var(&self, name: &str) -> Option<&VarInfo<'ctx>> {
        self.vars.iter().rev().find_map(|scope| scope.get(name))
    }

    pub(crate) fn insert_const(&mut self, name: String, value: i32) {
        if let Some(scope) = self.consts.last_mut() {
            scope.insert(name, value);
        }
    }

    pub(crate) fn lookup_const(&self, name: &str) -> Option<i32> {
        self.consts
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.module.constants.get(name).copied())
    }

    pub(crate) fn visible_constants(&self) -> HashMap<String, i32> {
        let mut constants = self.module.constants.clone();
        for scope in &self.consts {
            constants.extend(scope.iter().map(|(name, value)| (name.clone(), *value)));
        }
        constants
    }

    pub(crate) fn tmp(&mut self) -> String {
        let name = format!("t{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    pub(crate) fn label(&mut self, prefix: &str) -> String {
        let label = format!("{}.{}", prefix, self.label_counter);
        self.label_counter += 1;
        label
    }

    pub(crate) fn block(&mut self, label: &str) -> BasicBlock<'ctx> {
        if let Some(block) = self.blocks.get(label).copied() {
            return block;
        }
        let block = self.ir.context.append_basic_block(self.function, label);
        self.blocks.insert(label.to_string(), block);
        block
    }

    pub(crate) fn emit_label(&mut self, label: &str) {
        let block = self.block(label);
        self.builder.position_at_end(block);
        self.current_label = label.to_string();
        self.current_block = block;
        self.current_terminated = block.get_terminator().is_some();
    }

    pub(crate) fn terminate_return(&mut self, value: Option<&Value<'ctx>>) -> Result<(), String> {
        if self.current_terminated {
            return Ok(());
        }
        match value {
            Some(value) if !value.ty.is_void() => {
                let value = value.basic_value()?;
                self.builder
                    .build_return(Some(&value))
                    .map_err(|err| err.to_string())?;
            }
            _ => {
                self.builder
                    .build_return(None)
                    .map_err(|err| err.to_string())?;
            }
        }
        self.current_terminated = true;
        Ok(())
    }

    pub(crate) fn terminate_br(&mut self, label: &str) -> Result<(), String> {
        if !self.current_terminated {
            let target = self.block(label);
            self.builder
                .build_unconditional_branch(target)
                .map_err(|err| err.to_string())?;
            self.current_terminated = true;
        }
        Ok(())
    }

    pub(crate) fn terminate_cond_br(
        &mut self,
        cond: IntValue<'ctx>,
        then_label: &str,
        else_label: &str,
    ) -> Result<(), String> {
        if !self.current_terminated {
            let then_block = self.block(then_label);
            let else_block = self.block(else_label);
            self.builder
                .build_conditional_branch(cond, then_block, else_block)
                .map_err(|err| err.to_string())?;
            self.current_terminated = true;
        }
        Ok(())
    }

    pub(crate) fn alloca(&mut self, ty: &LlvmType) -> Result<PointerValue<'ctx>, String> {
        let name = self.tmp();
        self.builder
            .build_alloca(self.ir.basic_type(ty)?, &name)
            .map_err(|err| err.to_string())
    }

    pub(crate) fn load(
        &mut self,
        ptr: PointerValue<'ctx>,
        ty: &LlvmType,
    ) -> Result<Value<'ctx>, String> {
        let name = self.tmp();
        let value = self
            .builder
            .build_load(self.ir.basic_type(ty)?, ptr, &name)
            .map_err(|err| err.to_string())?;
        Ok(Value {
            value: Some(value),
            ty: ty.clone(),
        })
    }

    pub(crate) fn store(
        &mut self,
        value: &Value<'ctx>,
        ptr: PointerValue<'ctx>,
    ) -> Result<(), String> {
        let value = value.basic_value()?;
        self.builder
            .build_store(ptr, value)
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    pub(crate) fn int_const(&self, value: i32) -> Value<'ctx> {
        Value {
            value: Some(self.ir.i32_type().const_int(value as u64, true).into()),
            ty: LlvmType::I32,
        }
    }

    pub(crate) fn i64_const(&self, value: u64) -> Value<'ctx> {
        Value {
            value: Some(self.ir.i64_type().const_int(value, false).into()),
            ty: LlvmType::I64,
        }
    }

    pub(crate) fn default_value(&mut self, ty: &LlvmType) -> Result<Value<'ctx>, String> {
        if ty.is_void() {
            Ok(Value {
                value: None,
                ty: LlvmType::Void,
            })
        } else {
            Ok(Value {
                value: Some(self.ir.basic_type(ty)?.const_zero()),
                ty: ty.clone(),
            })
        }
    }

    pub(crate) fn promise_new(&mut self, value_ty: &LlvmType) -> Result<Value<'ctx>, String> {
        let size = value_ty.try_size(&self.module.layouts)? as u64;
        let value = self.call_named("__sysy_promise_new", &[self.i64_const(size)])?;
        Ok(Value {
            value: value.value,
            ty: LlvmType::Promise(Box::new(value_ty.clone())),
        })
    }

    pub(crate) fn promise_resolve(
        &mut self,
        promise: PointerValue<'ctx>,
        value: Option<&Value<'ctx>>,
    ) -> Result<(), String> {
        let value_ptr = match value {
            Some(value) if !value.ty.is_void() => {
                let slot = self.alloca(&value.ty)?;
                self.store(value, slot)?;
                slot
            }
            _ => self.ir.ptr_type().const_null(),
        };
        self.call_named_void(
            "__sysy_promise_resolve",
            &[
                Value::from_basic(promise.into(), LlvmType::Ptr(Box::new(LlvmType::I32))),
                Value::from_basic(value_ptr.into(), LlvmType::Ptr(Box::new(LlvmType::I32))),
            ],
        )
    }

    pub(crate) fn promise_resolve_typed(
        &mut self,
        promise: PointerValue<'ctx>,
        value: Option<&Value<'ctx>>,
        expected: &LlvmType,
    ) -> Result<(), String> {
        match value {
            Some(value) if matches!((&value.ty, expected), (LlvmType::Ptr(inner), expected) if **inner == *expected) => {
                self.call_named_void(
                    "__sysy_promise_resolve",
                    &[
                        Value::from_basic(promise.into(), LlvmType::Ptr(Box::new(LlvmType::I32))),
                        Value::from_basic(
                            value.ptr_value()?.into(),
                            LlvmType::Ptr(Box::new(LlvmType::I32)),
                        ),
                    ],
                )
            }
            _ => self.promise_resolve(promise, value),
        }
    }

    pub(crate) fn promise_wait(&mut self, promise: Value<'ctx>) -> Result<Value<'ctx>, String> {
        let value_ty = promise.ty.promise_value();
        let promise_ptr = promise.ptr_value()?;
        self.call_named_void("__sysy_promise_wait", &[promise.clone()])?;
        if value_ty.is_void() {
            Ok(Value {
                value: None,
                ty: LlvmType::Void,
            })
        } else {
            self.promise_read(Value {
                value: Some(promise_ptr.into()),
                ty: LlvmType::Promise(Box::new(value_ty)),
            })
        }
    }

    pub(crate) fn promise_read(&mut self, promise: Value<'ctx>) -> Result<Value<'ctx>, String> {
        let value_ty = promise.ty.promise_value();
        if value_ty.is_void() {
            return Ok(Value {
                value: None,
                ty: LlvmType::Void,
            });
        }
        let result_ptr = self.call_named("__sysy_promise_result_ptr", &[promise])?;
        self.load(result_ptr.ptr_value()?, &value_ty)
    }

    pub(crate) fn call_named(
        &mut self,
        name: &str,
        args: &[Value<'ctx>],
    ) -> Result<Value<'ctx>, String> {
        let function = self.ir.function(name)?;
        let fn_ty = function.get_type();
        let ret_ty = fn_ty.get_return_type();
        let args = args
            .iter()
            .map(|arg| arg.basic_metadata_value())
            .collect::<Result<Vec<_>, _>>()?;
        let call_name = self.tmp();
        let call = self
            .builder
            .build_call(function, &args, &call_name)
            .map_err(|err| err.to_string())?;
        if ret_ty.is_none() {
            Ok(Value {
                value: None,
                ty: LlvmType::Void,
            })
        } else {
            Ok(Value {
                value: call.try_as_basic_value().left(),
                ty: LlvmType::Ptr(Box::new(LlvmType::I32)),
            })
        }
    }

    pub(crate) fn call_named_typed(
        &mut self,
        name: &str,
        args: &[Value<'ctx>],
        ret_ty: LlvmType,
    ) -> Result<Value<'ctx>, String> {
        let mut value = self.call_named(name, args)?;
        value.ty = ret_ty;
        Ok(value)
    }

    pub(crate) fn call_named_void(
        &mut self,
        name: &str,
        args: &[Value<'ctx>],
    ) -> Result<(), String> {
        let _ = self.call_named(name, args)?;
        Ok(())
    }

    pub(crate) fn i32_ne_zero(&mut self, value: &Value<'ctx>) -> Result<IntValue<'ctx>, String> {
        let name = self.tmp();
        self.builder
            .build_int_compare(
                IntPredicate::NE,
                value.int_value()?,
                self.ir.i32_type().const_zero(),
                &name,
            )
            .map_err(|err| err.to_string())
    }
}

impl<'ctx> Value<'ctx> {
    pub(crate) fn from_basic(value: BasicValueEnum<'ctx>, ty: LlvmType) -> Self {
        Self {
            value: Some(value),
            ty,
        }
    }

    pub(crate) fn basic_value(&self) -> Result<BasicValueEnum<'ctx>, String> {
        self.value
            .ok_or_else(|| "void value cannot be used as an LLVM operand".to_string())
    }

    pub(crate) fn basic_metadata_value(&self) -> Result<BasicMetadataValueEnum<'ctx>, String> {
        Ok(self.basic_value()?.into())
    }

    pub(crate) fn int_value(&self) -> Result<IntValue<'ctx>, String> {
        Ok(self.basic_value()?.into_int_value())
    }

    pub(crate) fn ptr_value(&self) -> Result<PointerValue<'ctx>, String> {
        Ok(self.basic_value()?.into_pointer_value())
    }
}
