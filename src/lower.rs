// Legacy sugared-Koopa compatibility lowering. Struct/async correctness is
// implemented in the LLVM/RV32 backend, not in this compatibility path.
use std::collections::HashMap;

use crate::ast_tool::{Background, ValueType};
use crate::koopa::{KoopaLine, KoopaLines, KoopaParam, KoopaType};

fn strip_async_type(ty: &KoopaType) -> KoopaType {
    match ty {
        KoopaType::Promise(inner) => strip_async_type(inner),
        KoopaType::Ptr(inner) => KoopaType::Ptr(Box::new(strip_async_type(inner))),
        KoopaType::Array(inner, len) => KoopaType::Array(Box::new(strip_async_type(inner)), *len),
        KoopaType::I32 => KoopaType::I32,
        KoopaType::Void => KoopaType::Void,
        KoopaType::Struct(name) => KoopaType::Struct(name.clone()),
    }
}

fn strip_async_param(param: &KoopaParam) -> KoopaParam {
    KoopaParam::new(param.name.clone(), strip_async_type(&param.ty))
}

pub fn lower_async_sugar_lines(lines: &KoopaLines) -> KoopaLines {
    let mut lowered = KoopaLines::new();
    for line in lines.lines() {
        match line {
            KoopaLine::PromiseDecl(_, _) => {}
            KoopaLine::FuncDecl(name, params, ret_ty) => {
                lowered.add_line(KoopaLine::FuncDecl(
                    name.clone(),
                    params.iter().map(strip_async_param).collect(),
                    strip_async_type(ret_ty),
                ));
            }
            KoopaLine::FuncStart(name, params, ret_ty) => {
                lowered.add_line(KoopaLine::FuncStart(
                    name.clone(),
                    params.iter().map(strip_async_param).collect(),
                    strip_async_type(ret_ty),
                ));
            }
            KoopaLine::AsyncFuncStart(name, params, ret_ty) => {
                lowered.add_line(KoopaLine::FuncStart(
                    name.clone(),
                    params.iter().map(strip_async_param).collect(),
                    strip_async_type(ret_ty),
                ));
            }
            KoopaLine::ArgLabel(label, arg_name, arg_type) => {
                lowered.add_line(KoopaLine::ArgLabel(
                    label.clone(),
                    arg_name.clone(),
                    strip_async_type(arg_type),
                ));
            }
            KoopaLine::GlobalAlloc(name, ty, init) => {
                lowered.add_line(KoopaLine::GlobalAlloc(
                    name.clone(),
                    strip_async_type(ty),
                    init.clone(),
                ));
            }
            KoopaLine::Alloc(name, ty) => {
                lowered.add_line(KoopaLine::Alloc(name.clone(), strip_async_type(ty)));
            }
            KoopaLine::HeapAlloc(name, ty) => {
                lowered.add_line(KoopaLine::HeapAlloc(name.clone(), strip_async_type(ty)));
            }
            KoopaLine::AsyncCall(dest, func, args) => {
                lowered.add_line(KoopaLine::Call(dest.clone(), func.clone(), args.clone()));
            }
            KoopaLine::CallbackWrap(dest, promise, _) => {
                lowered.add_line(KoopaLine::Binary(
                    dest.clone(),
                    "add".to_string(),
                    "0".to_string(),
                    promise.clone(),
                ));
            }
            KoopaLine::Sleep(dest, _) => {
                lowered.add_line(KoopaLine::Binary(
                    dest.clone(),
                    "add".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                ));
            }
            KoopaLine::PromiseWait(_) => {}
            other => lowered.add_line(other.clone()),
        }
    }
    lowered
}

fn lower_type(ty: &KoopaType, bg: &Background) -> String {
    match ty {
        KoopaType::I32 => "i32".to_string(),
        KoopaType::Void => "void".to_string(),
        KoopaType::Promise(inner) => lower_type(inner, bg),
        KoopaType::Struct(name) => {
            let slots = bg.get_struct_layout(name).size / 4;
            format!("[i32, {}]", slots)
        }
        KoopaType::Ptr(inner) => match inner.as_ref() {
            KoopaType::Struct(_) => "*i32".to_string(),
            _ => format!("*{}", lower_type(inner, bg)),
        },
        KoopaType::Array(inner, len) => format!("[{}, {}]", lower_type(inner, bg), len),
    }
}

fn lower_decl_param(param: &KoopaParam, bg: &Background) -> String {
    lower_type(&param.ty, bg)
}

fn lower_def_param(param: &KoopaParam, bg: &Background) -> String {
    format!("{}: {}", param.name, lower_type(&param.ty, bg))
}

fn default_ret_value(ty: &KoopaType) -> Option<&'static str> {
    match ty {
        KoopaType::Void => None,
        KoopaType::I32 | KoopaType::Ptr(_) => Some("0"),
        KoopaType::Promise(inner) => default_ret_value(inner),
        KoopaType::Struct(_) | KoopaType::Array(_, _) => Some("zeroinit"),
    }
}

fn value_size(ty: &KoopaType, bg: &Background) -> usize {
    match ty {
        KoopaType::I32 | KoopaType::Ptr(_) => 4,
        KoopaType::Void => 0,
        KoopaType::Promise(inner) => value_size(inner, bg),
        KoopaType::Struct(name) => bg.get_struct_layout(name).size,
        KoopaType::Array(inner, len) => value_size(inner, bg) * len,
    }
}

fn field_storage_name(obj: &str, field_name: &str) -> String {
    let prefix = if obj.starts_with('%') { "%" } else { "@" };
    let mut base = String::new();
    for ch in obj.chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch);
        }
    }
    format!("{}f{}field{}", prefix, base, field_name)
}

#[derive(Debug, Clone)]
struct StructBaseInfo {
    struct_name: String,
    object_symbol: Option<String>,
}

#[derive(Debug, Clone)]
enum ValueInfo {
    StructBase(StructBaseInfo),
    FieldStorage {
        field_ty: ValueType,
        stored_symbol: Option<String>,
    },
    PointerStorage {
        stored_symbol: Option<String>,
    },
    Alias(String),
}

struct LowerState<'a> {
    bg: &'a Background,
    current_ret_type: Option<KoopaType>,
    values: HashMap<String, ValueInfo>,
}

impl<'a> LowerState<'a> {
    fn new(bg: &'a Background) -> Self {
        Self {
            bg,
            current_ret_type: None,
            values: HashMap::new(),
        }
    }

    fn resolve_symbol(&self, symbol: &str) -> String {
        let mut cur = symbol.to_string();
        while let Some(ValueInfo::Alias(next)) = self.values.get(&cur) {
            cur = next.clone();
        }
        cur
    }

    fn value_info(&self, symbol: &str) -> Option<ValueInfo> {
        let resolved = self.resolve_symbol(symbol);
        self.values.get(&resolved).cloned()
    }

    fn field_at_slot(&self, struct_name: &str, slot: usize) -> Option<(String, ValueType)> {
        self.bg
            .get_struct_layout(struct_name)
            .fields
            .iter()
            .find(|field| field.offset / 4 == slot)
            .map(|field| (field.name.clone(), field.ty.clone()))
    }

    fn lower_struct_alloc(&mut self, name: &str, struct_name: &str) -> Vec<String> {
        let mut lines = vec![format!(
            "\t{} = alloc {}",
            name,
            lower_type(&KoopaType::Struct(struct_name.to_string()), self.bg)
        )];
        self.values.insert(
            name.to_string(),
            ValueInfo::StructBase(StructBaseInfo {
                struct_name: struct_name.to_string(),
                object_symbol: Some(name.to_string()),
            }),
        );
        for field in &self.bg.get_struct_layout(struct_name).fields {
            if matches!(field.ty, ValueType::Pointer(_)) {
                lines.push(format!(
                    "\t{} = alloc {}",
                    field_storage_name(name, &field.name),
                    lower_type(&field.ty.to_koopa_type(self.bg), self.bg)
                ));
                self.values.insert(
                    field_storage_name(name, &field.name),
                    ValueInfo::FieldStorage {
                        field_ty: field.ty.clone(),
                        stored_symbol: None,
                    },
                );
            }
        }
        lines
    }

    fn lower_struct_global_alloc(
        &mut self,
        name: &str,
        struct_name: &str,
        init: &str,
    ) -> Vec<String> {
        let mut lines = vec![format!(
            "global {} = alloc {}, {}\n",
            name,
            lower_type(&KoopaType::Struct(struct_name.to_string()), self.bg),
            init
        )];
        self.values.insert(
            name.to_string(),
            ValueInfo::StructBase(StructBaseInfo {
                struct_name: struct_name.to_string(),
                object_symbol: Some(name.to_string()),
            }),
        );
        for field in &self.bg.get_struct_layout(struct_name).fields {
            if matches!(field.ty, ValueType::Pointer(_)) {
                lines.push(format!(
                    "global {} = alloc {}, zeroinit\n",
                    field_storage_name(name, &field.name),
                    lower_type(&field.ty.to_koopa_type(self.bg), self.bg)
                ));
                self.values.insert(
                    field_storage_name(name, &field.name),
                    ValueInfo::FieldStorage {
                        field_ty: field.ty.clone(),
                        stored_symbol: None,
                    },
                );
            }
        }
        lines
    }

    fn lower_heap_alloc(&mut self, name: &str, ty: &KoopaType) -> Vec<String> {
        let mut lines = vec![format!(
            "\t{} = call @malloc({})",
            name,
            value_size(ty, self.bg)
        )];
        if let KoopaType::Struct(struct_name) = ty {
            self.values.insert(
                name.to_string(),
                ValueInfo::StructBase(StructBaseInfo {
                    struct_name: struct_name.to_string(),
                    object_symbol: Some(name.to_string()),
                }),
            );
            for field in &self.bg.get_struct_layout(struct_name).fields {
                if matches!(field.ty, ValueType::Pointer(_)) {
                    lines.push(format!(
                        "\t{} = alloc {}",
                        field_storage_name(name, &field.name),
                        lower_type(&field.ty.to_koopa_type(self.bg), self.bg)
                    ));
                    self.values.insert(
                        field_storage_name(name, &field.name),
                        ValueInfo::FieldStorage {
                            field_ty: field.ty.clone(),
                            stored_symbol: None,
                        },
                    );
                }
            }
        }
        lines
    }

    fn lower_line(&mut self, line: &KoopaLine) -> Result<Vec<String>, String> {
        match line {
            KoopaLine::StructDecl(_, _) | KoopaLine::PromiseDecl(_, _) => Ok(Vec::new()),
            KoopaLine::FuncDecl(name, args, ret_type) => {
                let args = args
                    .iter()
                    .map(|p| lower_decl_param(p, self.bg))
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(ret_type, KoopaType::Void) {
                    Ok(vec![format!("decl @{}({})", name, args)])
                } else {
                    Ok(vec![format!(
                        "decl @{}({}): {}",
                        name,
                        args,
                        lower_type(ret_type, self.bg)
                    )])
                }
            }
            KoopaLine::FuncStart(name, args, ret_type) => {
                self.current_ret_type = Some(ret_type.clone());
                let args = args
                    .iter()
                    .map(|p| lower_def_param(p, self.bg))
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(ret_type, KoopaType::Void) {
                    Ok(vec![format!("fun @{}({}) {{", name, args)])
                } else {
                    Ok(vec![format!(
                        "fun @{}({}): {} {{",
                        name,
                        args,
                        lower_type(ret_type, self.bg)
                    )])
                }
            }
            KoopaLine::AsyncFuncStart(name, args, ret_type) => {
                self.current_ret_type = Some(ret_type.clone());
                let args = args
                    .iter()
                    .map(|p| lower_def_param(p, self.bg))
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(ret_type, KoopaType::Void) {
                    Ok(vec![format!("fun @{}({}) {{", name, args)])
                } else {
                    Ok(vec![format!(
                        "fun @{}({}): {} {{",
                        name,
                        args,
                        lower_type(ret_type, self.bg)
                    )])
                }
            }
            KoopaLine::FuncEnd => {
                self.current_ret_type = None;
                Ok(vec!["}\n".to_string()])
            }
            KoopaLine::Label(label) => Ok(vec![format!("{}:", label)]),
            KoopaLine::ArgLabel(label, arg_name, arg_type) => Ok(vec![format!(
                "\n{}({}: {}):",
                label,
                arg_name,
                lower_type(arg_type, self.bg)
            )]),
            KoopaLine::GlobalAlloc(name, KoopaType::Struct(struct_name), init) => {
                Ok(self.lower_struct_global_alloc(name, struct_name, init))
            }
            KoopaLine::GlobalAlloc(name, ty @ KoopaType::Ptr(_), init) => {
                self.values.insert(
                    name.clone(),
                    ValueInfo::PointerStorage {
                        stored_symbol: None,
                    },
                );
                Ok(vec![format!(
                    "global {} = alloc {}, {}\n",
                    name,
                    lower_type(ty, self.bg),
                    init
                )])
            }
            KoopaLine::GlobalAlloc(name, ty, init) => Ok(vec![format!(
                "global {} = alloc {}, {}\n",
                name,
                lower_type(ty, self.bg),
                init
            )]),
            KoopaLine::Alloc(ptr_name, KoopaType::Struct(struct_name)) => {
                Ok(self.lower_struct_alloc(ptr_name, struct_name))
            }
            KoopaLine::Alloc(ptr_name, KoopaType::Ptr(inner)) => {
                self.values.insert(
                    ptr_name.clone(),
                    ValueInfo::PointerStorage {
                        stored_symbol: None,
                    },
                );
                Ok(vec![format!(
                    "\t{} = alloc {}",
                    ptr_name,
                    lower_type(&KoopaType::Ptr(inner.clone()), self.bg)
                )])
            }
            KoopaLine::Alloc(ptr_name, ptr_type) => Ok(vec![format!(
                "\t{} = alloc {}",
                ptr_name,
                lower_type(ptr_type, self.bg)
            )]),
            KoopaLine::HeapAlloc(ptr_name, ptr_type) => {
                Ok(self.lower_heap_alloc(ptr_name, ptr_type))
            }
            KoopaLine::Store(value, ptr) => {
                let value = self.resolve_symbol(value);
                let ptr = self.resolve_symbol(ptr);
                if let Some(ValueInfo::PointerStorage { .. }) = self.values.get(&ptr) {
                    self.values.insert(
                        ptr.clone(),
                        ValueInfo::PointerStorage {
                            stored_symbol: Some(value.clone()),
                        },
                    );
                }
                if let Some(ValueInfo::FieldStorage { field_ty, .. }) =
                    self.values.get(&ptr).cloned()
                {
                    self.values.insert(
                        ptr.clone(),
                        ValueInfo::FieldStorage {
                            field_ty,
                            stored_symbol: Some(value.clone()),
                        },
                    );
                }
                Ok(vec![format!("\tstore {}, {}", value, ptr)])
            }
            KoopaLine::Load(dest, ptr) => {
                let ptr = self.resolve_symbol(ptr);
                if let Some(ValueInfo::PointerStorage {
                    stored_symbol: Some(symbol),
                }) = self.value_info(&ptr)
                {
                    if let Some(ValueInfo::StructBase(info)) = self.value_info(&symbol) {
                        self.values
                            .insert(dest.clone(), ValueInfo::StructBase(info));
                    }
                }
                if let Some(ValueInfo::FieldStorage {
                    field_ty,
                    stored_symbol,
                }) = self.value_info(&ptr)
                {
                    if let Some(symbol) = stored_symbol {
                        if let Some(ValueInfo::StructBase(info)) = self.value_info(&symbol) {
                            self.values
                                .insert(dest.clone(), ValueInfo::StructBase(info));
                        }
                    }
                    if let ValueType::Pointer(inner) = field_ty {
                        if let ValueType::Struct(struct_name) = *inner {
                            self.values.entry(dest.clone()).or_insert_with(|| {
                                ValueInfo::StructBase(StructBaseInfo {
                                    struct_name,
                                    object_symbol: None,
                                })
                            });
                        }
                    }
                }
                Ok(vec![format!("\t{} = load {}", dest, ptr)])
            }
            KoopaLine::StructFieldPtr(dest, ptr, field_name) => {
                let ptr = self.resolve_symbol(ptr);
                if let Some(ValueInfo::StructBase(info)) = self.value_info(&ptr) {
                    let field = self
                        .bg
                        .get_struct_layout(&info.struct_name)
                        .fields
                        .iter()
                        .find(|field| field.name == *field_name)
                        .ok_or_else(|| {
                            format!(
                                "struct {} has no field named {}",
                                info.struct_name, field_name
                            )
                        })?;
                    if matches!(field.ty, ValueType::Pointer(_)) {
                        if let Some(owner) = info.object_symbol {
                            let storage = field_storage_name(&owner, field_name);
                            self.values
                                .insert(dest.clone(), ValueInfo::Alias(storage.clone()));
                            self.values
                                .entry(storage)
                                .or_insert_with(|| ValueInfo::FieldStorage {
                                    field_ty: field.ty.clone(),
                                    stored_symbol: None,
                                });
                            return Ok(Vec::new());
                        }
                        return Err(format!(
                            "unsupported lowering: pointer field access on non-materialized struct pointer {}",
                            ptr
                        ));
                    }

                    let slot = field.offset / 4;
                    if let ValueType::Struct(inner_struct) = &field.ty {
                        self.values.insert(
                            dest.clone(),
                            ValueInfo::StructBase(StructBaseInfo {
                                struct_name: inner_struct.clone(),
                                object_symbol: None,
                            }),
                        );
                    }
                    return Ok(vec![format!("\t{} = getptr {}, {}", dest, ptr, slot)]);
                }
                Err(format!(
                    "unsupported lowering: field access on unknown struct base {}",
                    ptr
                ))
            }
            KoopaLine::GetElemPtr(dest, ptr, idx) => {
                let ptr = self.resolve_symbol(ptr);
                if idx == "0" {
                    if let Some(ValueInfo::StructBase(info)) = self.value_info(&ptr) {
                        self.values
                            .insert(dest.clone(), ValueInfo::StructBase(info));
                    }
                }
                Ok(vec![format!("\t{} = getelemptr {}, {}", dest, ptr, idx)])
            }
            KoopaLine::GetPtr(dest, ptr, idx) => {
                let ptr = self.resolve_symbol(ptr);
                let Some(slot) = idx.parse::<usize>().ok() else {
                    return Ok(vec![format!("\t{} = getptr {}, {}", dest, ptr, idx)]);
                };
                if let Some(ValueInfo::StructBase(info)) = self.value_info(&ptr) {
                    if let Some((field_name, field_ty)) =
                        self.field_at_slot(&info.struct_name, slot)
                    {
                        if matches!(field_ty, ValueType::Pointer(_)) {
                            if let Some(owner) = info.object_symbol {
                                let storage = field_storage_name(&owner, &field_name);
                                self.values.insert(dest.clone(), ValueInfo::Alias(storage));
                                self.values.insert(
                                    field_storage_name(&owner, &field_name),
                                    ValueInfo::FieldStorage {
                                        field_ty,
                                        stored_symbol: None,
                                    },
                                );
                                return Ok(Vec::new());
                            }
                            return Err(format!(
                                "unsupported lowering: pointer field access on non-materialized struct pointer {}",
                                ptr
                            ));
                        }
                        if let ValueType::Struct(inner_struct) = &field_ty {
                            self.values.insert(
                                dest.clone(),
                                ValueInfo::StructBase(StructBaseInfo {
                                    struct_name: inner_struct.clone(),
                                    object_symbol: None,
                                }),
                            );
                        }
                    }
                }
                Ok(vec![format!("\t{} = getptr {}, {}", dest, ptr, idx)])
            }
            KoopaLine::Binary(dest, op, lhs, rhs) => {
                let lhs = self.resolve_symbol(lhs);
                let rhs = self.resolve_symbol(rhs);
                Ok(vec![format!("\t{} = {} {}, {}", dest, op, lhs, rhs)])
            }
            KoopaLine::AsyncCall(dest, func, arg) => {
                Ok(vec![format!("\t{} = call @{}({})", dest, func, arg)])
            }
            KoopaLine::CallbackWrap(dest, promise, _) => {
                let promise = self.resolve_symbol(promise);
                Ok(vec![format!("\t{} = add 0, {}", dest, promise)])
            }
            KoopaLine::Sleep(dest, _duration) => Ok(vec![format!("\t{} = add 0, 0", dest)]),
            KoopaLine::PromiseWait(_) => Ok(Vec::new()),
            KoopaLine::Br(cond, then_bb, else_bb) => {
                let cond = self.resolve_symbol(cond);
                Ok(vec![format!("\tbr {}, {}, {}", cond, then_bb, else_bb)])
            }
            KoopaLine::Jump(target) => Ok(vec![format!("\tjump {}", target)]),
            KoopaLine::ArgJump(target, arg) => {
                let arg = self.resolve_symbol(arg);
                Ok(vec![format!("\tjump {}({})", target, arg)])
            }
            KoopaLine::Call(dest, func, arg) => {
                Ok(vec![format!("\t{} = call @{}({})", dest, func, arg)])
            }
            KoopaLine::VoidCall(func, arg) => Ok(vec![format!("\tcall @{}({})", func, arg)]),
            KoopaLine::Ret(value) => {
                let value = self.resolve_symbol(value);
                Ok(vec![format!("\tret {}", value)])
            }
            KoopaLine::VoidRet => {
                if let Some(ret_ty) = &self.current_ret_type {
                    if let Some(value) = default_ret_value(ret_ty) {
                        return Ok(vec![format!("\tret {}", value)]);
                    }
                }
                Ok(vec!["\tret".to_string()])
            }
        }
    }
}

pub fn lower_lines(lines: &KoopaLines, bg: &Background) -> String {
    let mut state = LowerState::new(bg);
    let mut lowered = Vec::new();
    let needs_malloc = lines
        .lines()
        .iter()
        .any(|line| matches!(line, KoopaLine::HeapAlloc(_, _)));
    if needs_malloc
        && !lines
            .lines()
            .iter()
            .any(|line| matches!(line, KoopaLine::FuncDecl(name, _, _) if name == "malloc"))
    {
        lowered.push("decl @malloc(i32): *i32".to_string());
    }
    for line in lines.lines() {
        match state.lower_line(line) {
            Ok(mut chunk) => lowered.append(&mut chunk),
            Err(err) => panic!("{}", err),
        }
    }
    lowered.join("\n")
}
