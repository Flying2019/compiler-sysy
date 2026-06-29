use super::layout::{RISCV64_DATA_LAYOUT, TARGET_LAYOUT};
use super::module::ModuleCtx;
use super::names::sanitize_ident;
use super::types::LlvmType;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::{TargetData, TargetTriple};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType, StructType};
use inkwell::values::{FunctionValue, GlobalValue};
use inkwell::AddressSpace;
use std::collections::HashMap;

pub(crate) struct IrModule<'ctx> {
    pub(crate) context: &'ctx Context,
    pub(crate) module: Module<'ctx>,
    structs: HashMap<String, StructType<'ctx>>,
    functions: HashMap<String, FunctionValue<'ctx>>,
    globals: HashMap<String, GlobalValue<'ctx>>,
}

impl<'ctx> IrModule<'ctx> {
    pub(crate) fn new(
        context: &'ctx Context,
        name: &str,
        target_triple: &str,
        module_ctx: &ModuleCtx,
    ) -> Result<Self, String> {
        let module = context.create_module(name);
        module.set_triple(&TargetTriple::create(target_triple));
        if target_triple.starts_with("riscv64") {
            let target_data = TargetData::create(RISCV64_DATA_LAYOUT);
            module.set_data_layout(&target_data.get_data_layout());
        }

        let mut out = Self {
            context,
            module,
            structs: HashMap::new(),
            functions: HashMap::new(),
            globals: HashMap::new(),
        };
        out.declare_named_structs(module_ctx)?;
        out.declare_runtime_functions()?;
        Ok(out)
    }

    fn declare_named_structs(&mut self, module_ctx: &ModuleCtx) -> Result<(), String> {
        let mut names = module_ctx.layouts.keys().cloned().collect::<Vec<_>>();
        names.sort();
        for name in &names {
            let llvm_name = format!("struct.{}", sanitize_ident(name));
            let struct_ty = self.context.opaque_struct_type(&llvm_name);
            self.structs.insert(name.clone(), struct_ty);
        }
        for name in names {
            let layout = module_ctx
                .layouts
                .get(&name)
                .ok_or_else(|| format!("Unknown struct layout {}", name))?;
            let fields = layout
                .fields
                .iter()
                .map(|field| self.basic_type(&field.ty))
                .collect::<Result<Vec<_>, _>>()?;
            self.structs
                .get(&name)
                .ok_or_else(|| format!("Missing LLVM struct {}", name))?
                .set_body(&fields, false);
        }
        let promise = self.context.opaque_struct_type("promise");
        promise.set_body(
            &[
                self.i32_type().into(),
                self.i32_type().into(),
                self.i64_type().into(),
                self.ptr_type().into(),
                self.ptr_type().into(),
                self.ptr_type().into(),
                self.ptr_type().into(),
                self.ptr_type().into(),
                self.ptr_type().into(),
            ],
            false,
        );
        self.structs.insert("__sysy_promise".to_string(), promise);
        Ok(())
    }

    fn declare_runtime_functions(&mut self) -> Result<(), String> {
        self.declare_function(
            "malloc",
            &LlvmType::Ptr(Box::new(LlvmType::I32)),
            &[LlvmType::I64],
        )?;
        self.declare_function(
            "free",
            &LlvmType::Void,
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function("getint", &LlvmType::I32, &[])?;
        self.declare_function("getch", &LlvmType::I32, &[])?;
        self.declare_function(
            "getarray",
            &LlvmType::I32,
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function("putint", &LlvmType::Void, &[LlvmType::I32])?;
        self.declare_function("putch", &LlvmType::I32, &[LlvmType::I32])?;
        self.declare_function(
            "putarray",
            &LlvmType::Void,
            &[LlvmType::I32, LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function("starttime", &LlvmType::Void, &[])?;
        self.declare_function("stoptime", &LlvmType::Void, &[])?;
        self.declare_function(
            "__sysy_promise_new",
            &LlvmType::Ptr(Box::new(LlvmType::I32)),
            &[LlvmType::I64],
        )?;
        self.declare_function(
            "__sysy_promise_resolve",
            &LlvmType::Void,
            &[
                LlvmType::Ptr(Box::new(LlvmType::I32)),
                LlvmType::Ptr(Box::new(LlvmType::I32)),
            ],
        )?;
        self.declare_function(
            "__sysy_promise_result_ptr",
            &LlvmType::Ptr(Box::new(LlvmType::I32)),
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function(
            "__sysy_promise_wait",
            &LlvmType::Void,
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function(
            "__sysy_promise_is_ready",
            &LlvmType::I32,
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function(
            "__sysy_promise_set_callback",
            &LlvmType::Void,
            &[
                LlvmType::Ptr(Box::new(LlvmType::I32)),
                LlvmType::Ptr(Box::new(LlvmType::I32)),
                LlvmType::Ptr(Box::new(LlvmType::I32)),
            ],
        )?;
        self.declare_function(
            "__sysy_pending_add",
            &LlvmType::Void,
            &[
                LlvmType::Ptr(Box::new(LlvmType::I32)),
                LlvmType::Ptr(Box::new(LlvmType::I32)),
                LlvmType::Ptr(Box::new(LlvmType::I32)),
            ],
        )?;
        self.declare_function(
            "__sysy_promise_clear_driver",
            &LlvmType::Void,
            &[LlvmType::Ptr(Box::new(LlvmType::I32))],
        )?;
        self.declare_function(
            "__sysy_sleep",
            &LlvmType::Ptr(Box::new(LlvmType::I32)),
            &[LlvmType::I32],
        )?;
        let _ = TARGET_LAYOUT.malloc_size_type;
        Ok(())
    }

    pub(crate) fn i32_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i32_type()
    }

    pub(crate) fn i64_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.i64_type()
    }

    pub(crate) fn bool_type(&self) -> inkwell::types::IntType<'ctx> {
        self.context.bool_type()
    }

    pub(crate) fn ptr_type(&self) -> inkwell::types::PointerType<'ctx> {
        self.context.ptr_type(AddressSpace::default())
    }

    pub(crate) fn basic_type(&self, ty: &LlvmType) -> Result<BasicTypeEnum<'ctx>, String> {
        match ty {
            LlvmType::I32 => Ok(self.i32_type().into()),
            LlvmType::I64 => Ok(self.i64_type().into()),
            LlvmType::Ptr(_) | LlvmType::Promise(_) => Ok(self.ptr_type().into()),
            LlvmType::Struct(name) => self
                .structs
                .get(name)
                .copied()
                .map(Into::into)
                .ok_or_else(|| format!("Unknown LLVM struct type {}", name)),
            LlvmType::Array(len, inner) => Ok(self
                .basic_type(inner)?
                .array_type(
                    (*len)
                        .try_into()
                        .map_err(|_| "Array type is too large".to_string())?,
                )
                .into()),
            LlvmType::Void => Err("void is not a first-class LLVM value type".to_string()),
        }
    }

    pub(crate) fn struct_type(&self, name: &str) -> Result<StructType<'ctx>, String> {
        self.structs
            .get(name)
            .copied()
            .ok_or_else(|| format!("Unknown LLVM struct type {}", name))
    }

    pub(crate) fn function_type(
        &self,
        ret: &LlvmType,
        params: &[LlvmType],
    ) -> Result<FunctionType<'ctx>, String> {
        let params = params
            .iter()
            .map(|ty| self.basic_type(ty).map(BasicMetadataTypeEnum::from))
            .collect::<Result<Vec<_>, _>>()?;
        if ret.is_void() {
            Ok(self.context.void_type().fn_type(&params, false))
        } else {
            Ok(self.basic_type(ret)?.fn_type(&params, false))
        }
    }

    pub(crate) fn declare_function(
        &mut self,
        name: &str,
        ret: &LlvmType,
        params: &[LlvmType],
    ) -> Result<FunctionValue<'ctx>, String> {
        let safe_name = sanitize_ident(name);
        if let Some(function) = self.functions.get(&safe_name).copied() {
            return Ok(function);
        }
        let fn_ty = self.function_type(ret, params)?;
        let function = self.module.add_function(&safe_name, fn_ty, None);
        self.functions.insert(safe_name, function);
        Ok(function)
    }

    pub(crate) fn function(&self, name: &str) -> Result<FunctionValue<'ctx>, String> {
        let safe_name = sanitize_ident(name);
        self.functions
            .get(&safe_name)
            .copied()
            .or_else(|| self.module.get_function(&safe_name))
            .ok_or_else(|| format!("Unknown LLVM function {}", name))
    }

    pub(crate) fn add_frame_struct(
        &mut self,
        name: &str,
        fields: &[LlvmType],
    ) -> Result<StructType<'ctx>, String> {
        if let Some(existing) = self.structs.get(name).copied() {
            return Ok(existing);
        }
        let struct_ty = self.context.opaque_struct_type(name);
        let body = fields
            .iter()
            .map(|ty| self.basic_type(ty))
            .collect::<Result<Vec<_>, _>>()?;
        struct_ty.set_body(&body, false);
        self.structs.insert(name.to_string(), struct_ty);
        Ok(struct_ty)
    }

    pub(crate) fn add_global(
        &mut self,
        name: &str,
        ty: &LlvmType,
        initializer: inkwell::values::BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        let safe_name = sanitize_ident(name);
        if self.globals.contains_key(&safe_name) {
            return Ok(());
        }
        let global = self
            .module
            .add_global(self.basic_type(ty)?, None, &safe_name);
        global.set_initializer(&initializer);
        self.globals.insert(safe_name, global);
        Ok(())
    }

    pub(crate) fn global(&self, name: &str) -> Result<GlobalValue<'ctx>, String> {
        let safe_name = sanitize_ident(name);
        self.globals
            .get(&safe_name)
            .copied()
            .or_else(|| self.module.get_global(&safe_name))
            .ok_or_else(|| format!("Unknown LLVM global {}", name))
    }
}
