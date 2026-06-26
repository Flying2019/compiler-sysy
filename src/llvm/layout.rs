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
    pub(crate) fn try_size(
        &self,
        structs: &HashMap<String, StructLayout>,
    ) -> Result<usize, String> {
        match self {
            Self::I32 => Ok(TARGET_LAYOUT.int_size),
            Self::Ptr(_) | Self::Promise(_) => Ok(TARGET_LAYOUT.pointer_size),
            Self::Void => Ok(0),
            Self::Array(len, inner) => Ok(len * inner.try_size(structs)?),
            Self::Struct(name) => structs
                .get(name)
                .map(|layout| layout.size)
                .ok_or_else(|| format!("Unknown struct type {}", name)),
        }
    }

    pub(crate) fn try_align(
        &self,
        structs: &HashMap<String, StructLayout>,
    ) -> Result<usize, String> {
        match self {
            Self::I32 => Ok(TARGET_LAYOUT.int_align),
            Self::Ptr(_) | Self::Promise(_) => Ok(TARGET_LAYOUT.pointer_align),
            Self::Void => Ok(1),
            Self::Array(_, inner) => inner.try_align(structs),
            Self::Struct(name) => structs
                .get(name)
                .map(|layout| layout.align)
                .ok_or_else(|| format!("Unknown struct type {}", name)),
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
