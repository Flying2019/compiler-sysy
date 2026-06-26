use super::module::ModuleCtx;
use super::types::LlvmType;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct VarInfo {
    pub(crate) ptr: String,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct Value {
    pub(crate) name: String,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct LValue {
    pub(crate) ptr: String,
    pub(crate) ty: LlvmType,
}

#[derive(Debug, Clone)]
pub(crate) struct LoopLabels {
    pub(crate) break_label: String,
    pub(crate) continue_label: String,
}

#[derive(Debug)]
pub(crate) struct FunctionCtx<'a> {
    pub(crate) module: &'a ModuleCtx,
    pub(crate) ret_ty: LlvmType,
    pub(crate) vars: Vec<HashMap<String, VarInfo>>,
    pub(crate) consts: Vec<HashMap<String, i32>>,
    pub(crate) lines: Vec<String>,
    pub(crate) tmp_counter: usize,
    pub(crate) label_counter: usize,
    pub(crate) current_label: String,
    pub(crate) current_terminated: bool,
    pub(crate) loop_stack: Vec<LoopLabels>,
    pub(crate) is_async: bool,
    pub(crate) promise_ptr: Option<String>,
}

impl<'a> FunctionCtx<'a> {
    pub(crate) fn new(module: &'a ModuleCtx, ret_ty: LlvmType, is_async: bool) -> Self {
        Self {
            module,
            ret_ty,
            vars: vec![HashMap::new()],
            consts: vec![HashMap::new()],
            lines: Vec::new(),
            tmp_counter: 0,
            label_counter: 0,
            current_label: "entry".to_string(),
            current_terminated: false,
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

    pub(crate) fn insert_var(&mut self, name: String, info: VarInfo) -> Result<(), String> {
        self.vars
            .last_mut()
            .ok_or_else(|| "No active variable scope".to_string())?
            .insert(name, info);
        Ok(())
    }

    pub(crate) fn lookup_var(&self, name: &str) -> Option<&VarInfo> {
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
        let name = format!("%t{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    pub(crate) fn label(&mut self, prefix: &str) -> String {
        let label = format!("{}.{}", prefix, self.label_counter);
        self.label_counter += 1;
        label
    }

    pub(crate) fn emit(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    pub(crate) fn emit_label(&mut self, label: &str) {
        self.lines.push(format!("{}:", label));
        self.current_label = label.to_string();
        self.current_terminated = false;
    }

    pub(crate) fn terminate(&mut self, line: impl Into<String>) {
        if !self.current_terminated {
            self.emit(line);
            self.current_terminated = true;
        }
    }

    pub(crate) fn alloca(&mut self, ty: &LlvmType) -> String {
        let ptr = self.tmp();
        self.emit(format!("  {} = alloca {}", ptr, ty.llvm()));
        ptr
    }

    pub(crate) fn load(&mut self, ptr: &str, ty: &LlvmType) -> Value {
        let val = self.tmp();
        self.emit(format!("  {} = load {}, ptr {}", val, ty.llvm(), ptr));
        Value {
            name: val,
            ty: ty.clone(),
        }
    }

    pub(crate) fn store(&mut self, value: &Value, ptr: &str) {
        self.emit(format!(
            "  store {} {}, ptr {}",
            value.ty.llvm(),
            value.name,
            ptr
        ));
    }

    pub(crate) fn default_value(&mut self, ty: &LlvmType) -> Result<Value, String> {
        match ty {
            LlvmType::I32 => Ok(Value {
                name: "0".to_string(),
                ty: LlvmType::I32,
            }),
            LlvmType::Ptr(_) | LlvmType::Promise(_) => Ok(Value {
                name: "null".to_string(),
                ty: ty.clone(),
            }),
            other => Err(format!("No scalar default value for {:?}", other)),
        }
    }

    pub(crate) fn promise_new(&mut self, value_ty: &LlvmType) -> Result<Value, String> {
        let ptr = self.tmp();
        let size = value_ty.try_size(&self.module.layouts)?;
        self.emit(format!(
            "  {} = call ptr @__sysy_promise_new(i64 {})",
            ptr, size
        ));
        Ok(Value {
            name: ptr,
            ty: LlvmType::Promise(Box::new(value_ty.clone())),
        })
    }

    pub(crate) fn promise_resolve(&mut self, promise: &str, value: Option<&Value>) {
        match value {
            Some(value) if !value.ty.is_void() => {
                let slot = self.alloca(&value.ty);
                self.store(value, &slot);
                self.emit(format!(
                    "  call void @__sysy_promise_resolve(ptr {}, ptr {})",
                    promise, slot
                ));
            }
            _ => self.emit(format!(
                "  call void @__sysy_promise_resolve(ptr {}, ptr null)",
                promise
            )),
        }
    }

    pub(crate) fn promise_resolve_typed(
        &mut self,
        promise: &str,
        value: Option<&Value>,
        expected: &LlvmType,
    ) {
        match value {
            Some(value) if matches!((&value.ty, expected), (LlvmType::Ptr(inner), expected) if **inner == *expected) =>
            {
                self.emit(format!(
                    "  call void @__sysy_promise_resolve(ptr {}, ptr {})",
                    promise, value.name
                ));
            }
            _ => self.promise_resolve(promise, value),
        }
    }

    pub(crate) fn promise_wait(&mut self, promise: Value) -> Value {
        let value_ty = promise.ty.promise_value();
        self.emit(format!(
            "  call void @__sysy_promise_wait(ptr {})",
            promise.name
        ));
        if value_ty.is_void() {
            Value {
                name: "0".to_string(),
                ty: LlvmType::Void,
            }
        } else {
            self.promise_read(Value {
                name: promise.name,
                ty: LlvmType::Promise(Box::new(value_ty)),
            })
        }
    }

    pub(crate) fn promise_read(&mut self, promise: Value) -> Value {
        let value_ty = promise.ty.promise_value();
        if value_ty.is_void() {
            Value {
                name: "0".to_string(),
                ty: LlvmType::Void,
            }
        } else {
            let result_ptr = self.tmp();
            self.emit(format!(
                "  {} = call ptr @__sysy_promise_result_ptr(ptr {})",
                result_ptr, promise.name
            ));
            self.load(&result_ptr, &value_ty)
        }
    }
}
