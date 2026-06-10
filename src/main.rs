use clap::{Parser, ValueEnum};
use compile_sysy::asm;
use compile_sysy::asm::program_to_asm;
use compile_sysy::ast::*;
use compile_sysy::ast_tool;
use compile_sysy::ast_tool::{scan_global_symbol, Background};
use compile_sysy::koopa::KoopaLines;
use compile_sysy::lalr::CompUnit;
use compile_sysy::lexer::Lexer;
use compile_sysy::lower::lower_lines;
use lalrpop_util::lalrpop_mod;
use std::env;
use std::io::Result;
use std::{fs::read_to_string, path::PathBuf};

#[derive(ValueEnum, Clone, Debug)]
enum Mode {
    Koopa,
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
}

lalrpop_mod!(sysy);

// 为了支持非标准的 -koopa 和 -riscv 形式进行的预处理
fn preprocess_args() -> Vec<String> {
    let mut raw_args: Vec<String> = env::args().collect();
    for arg in raw_args.iter_mut() {
        if arg == "-koopa" {
            *arg = "--mode=koopa".to_string();
        } else if arg == "-riscv" {
            *arg = "--mode=riscv".to_string();
        }
    }
    raw_args
}

fn str_to_ast(input: &str) -> CompUnit {
    sysy::CompUnitParser::new()
        .parse(Lexer::new(input))
        .expect("Failed to parse input")
}

fn ast_to_koopa_lines(ast: &CompUnit) -> (KoopaLines, Background) {
    if let Err(err) = ast.validate_async_syntax() {
        panic!("{}", err);
    }
    let mut bg = ast_tool::Background::new();
    scan_global_symbol(ast.clone(), &mut bg);
    let lines = ast.to_koopa_lines(&mut bg);
    (lines, bg)
}

fn ir_to_asm(koopa_ir: &str) -> String {
    let driver = koopa::front::Driver::from(koopa_ir.to_string());
    let program = driver.generate_program().unwrap();
    let asm = program_to_asm(&program, &asm::Background::new());
    asm.to_string()
}

fn main() -> Result<()> {
    let args = Args::parse_from(preprocess_args());
    match args.mode {
        Mode::Koopa => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input);
            let (koopa_lines, _) = ast_to_koopa_lines(&ast);
            std::fs::write(args.output, koopa_lines.to_wrapped_string())?;
        }
        Mode::Riscv => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input);
            let (koopa_lines, bg) = ast_to_koopa_lines(&ast);
            let koopa_ir = lower_lines(&koopa_lines, &bg);
            println!("Generated Koopa IR:\n{}", koopa_lines.to_wrapped_string());
            let asm = ir_to_asm(&koopa_ir);
            std::fs::write(args.output, asm.to_string())?;
        }
    }
    Ok(())
}
