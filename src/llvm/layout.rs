use super::types::LlvmType;
use std::collections::HashMap;

pub(crate) const RISCV64_DATA_LAYOUT: &str = "e-m:e-p:64:64-i64:64-i128:128-n64-S128";

#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetLayout {
    pub(crate) pointer_size: usize,
    pub(crate) pointer_align: usize,
    pub(crate) int_size: usize,
    pub(crate) int_align: usize,
    pub(crate) malloc_size_type: &'static str,
}

pub(crate) const TARGET_LAYOUT: TargetLayout = TargetLayout {
    pointer_size: 8,
    pointer_align: 8,
    int_size: 4,
    int_align: 4,
    malloc_size_type: "i64",
};

#[derive(Debug, Clone)]
pub(crate) struct FieldLayout {
    pub(crate) name: String,
    pub(crate) ty: LlvmType,
    pub(crate) index: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct StructLayout {
    pub(crate) fields: Vec<FieldLayout>,
    pub(crate) size: usize,
    pub(crate) align: usize,
}

impl LlvmType {
    pub(crate) fn size(&self, structs: &HashMap<String, StructLayout>) -> usize {
        match self {
            Self::I32 => TARGET_LAYOUT.int_size,
            Self::Ptr(_) | Self::Promise(_) => TARGET_LAYOUT.pointer_size,
            Self::Void => 0,
            Self::Array(len, inner) => len * inner.size(structs),
            Self::Struct(name) => {
                structs
                    .get(name)
                    .unwrap_or_else(|| panic!("Unknown struct type {}", name))
                    .size
            }
        }
    }

    pub(crate) fn align(&self, structs: &HashMap<String, StructLayout>) -> usize {
        match self {
            Self::I32 => TARGET_LAYOUT.int_align,
            Self::Ptr(_) | Self::Promise(_) => TARGET_LAYOUT.pointer_align,
            Self::Void => 1,
            Self::Array(_, inner) => inner.align(structs),
            Self::Struct(name) => {
                structs
                    .get(name)
                    .unwrap_or_else(|| panic!("Unknown struct type {}", name))
                    .align
            }
        }
    }
}

pub(crate) fn align_up(value: usize, align: usize) -> usize {
    if align == 0 {
        value
    } else {
        (value + align - 1) / align * align
    }
}
