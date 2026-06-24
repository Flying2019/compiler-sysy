use clap::{Parser, ValueEnum};
use compile_sysy::ast::*;
use compile_sysy::ast_tool;
use compile_sysy::ast_tool::{scan_global_symbol, Background};
use compile_sysy::koopa::KoopaLines;
use compile_sysy::lalr::CompUnit;
use compile_sysy::lexer::Lexer;
use compile_sysy::llvm_ir::{
    compile_llvm_to_riscv_asm, try_compile_to_llvm, try_compile_to_llvm_with_target,
};
use lalrpop_util::lalrpop_mod;
use std::env;
use std::io::{self, Result};
use std::{fs::read_to_string, path::PathBuf};

#[derive(ValueEnum, Clone, Debug)]
enum Mode {
    Koopa,
    Llvm,
    Riscv,
}

#[derive(Parser, Debug)]
#[command(version, long_about = None)]
struct Args {
    #[arg(short, long, value_enum)]
    mode: Mode,

    input: PathBuf,

    #[arg(short, long)]
    output: PathBuf,

    #[arg(long, default_value = "riscv32-unknown-unknown-elf")]
    target: String,
}

lalrpop_mod!(sysy);

// 为了支持非标准的 -koopa 和 -riscv 形式进行的预处理
fn preprocess_args() -> Vec<String> {
    let mut raw_args: Vec<String> = env::args().collect();
    for arg in raw_args.iter_mut() {
        if arg == "-koopa" {
            *arg = "--mode=koopa".to_string();
        } else if arg == "-llvm" {
            *arg = "--mode=llvm".to_string();
        } else if arg == "-riscv" {
            *arg = "--mode=riscv".to_string();
        }
    }
    raw_args
}

fn str_to_ast(input: &str) -> Result<CompUnit> {
    sysy::CompUnitParser::new()
        .parse(Lexer::new(input))
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, format!("parse error: {err:?}")))
}

fn ast_to_koopa_lines(ast: &CompUnit) -> Result<(KoopaLines, Background)> {
    ast.validate_semantics()
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    if ast.contains_async_syntax() {
        let _ = ast.analyze_async();
    }
    let mut bg = ast_tool::Background::new();
    scan_global_symbol(ast.clone(), &mut bg);
    let lines = ast.to_koopa_lines(&mut bg);
    Ok((lines, bg))
}

fn main() -> Result<()> {
    let args = Args::parse_from(preprocess_args());
    match args.mode {
        Mode::Koopa => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input)?;
            let (koopa_lines, _) = ast_to_koopa_lines(&ast)?;
            eprintln!(
                "warning: -koopa is a debug/legacy output path; LLVM/RV32 is the correctness path for struct and async semantics"
            );
            std::fs::write(args.output, koopa_lines.to_wrapped_string())?;
        }
        Mode::Llvm => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input)?;
            let target = resolve_llvm_target(&args.target);
            let llvm_ir = try_compile_to_llvm_with_target(&ast, &target)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
            std::fs::write(args.output, llvm_ir)?;
        }
        Mode::Riscv => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input)?;
            let llvm_ir = try_compile_to_llvm(&ast)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
            compile_llvm_to_riscv_asm(&llvm_ir, &args.output)?;
        }
    }
    Ok(())
}

fn resolve_llvm_target(target: &str) -> String {
    match target {
        "host" => host_llvm_target(),
        other => other.to_string(),
    }
}

fn host_llvm_target() -> String {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("aarch64", "macos") => "arm64-apple-macosx".to_string(),
        _ => "riscv32-unknown-unknown-elf".to_string(),
    }
}
