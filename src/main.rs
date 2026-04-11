use compile_sysy::asm::{Background, GenerateAsm};
use lalrpop_util::lalrpop_mod;
use compile_sysy::ast::*;
use std::env;
use std::{fs::read_to_string, path::PathBuf};
use std::io::Result;
use clap::{Parser, ValueEnum};

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

    /// 输入文件路径
    input: PathBuf,

    /// 输出文件路径
    #[arg(short, long)]
    output: PathBuf,
}

// 引用 lalrpop 生成的解析器
// 因为我们刚刚创建了 sysy.lalrpop, 所以模块名是 sysy
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

fn main() -> Result<()> {
    let args = Args::parse_from(preprocess_args());
    match args.mode {
        Mode::Koopa => {
            let input = read_to_string(args.input)?;
            let ast = sysy::CompUnitParser::new().parse(&input).expect("Failed to parse input");
            let koopa_lines = ast.to_koopa_lines(&mut compile_sysy::ast_tool::Background::new());
            std::fs::write(args.output, koopa_lines.to_string())?;
        }
        Mode::Riscv => {
            let input = read_to_string(args.input)?;
            let ast = sysy::CompUnitParser::new().parse(&input).expect("Failed to parse input");
            let koopa_ir = ast
                .to_koopa_lines(&mut compile_sysy::ast_tool::Background::new())
                .to_string();
            println!("Generated Koopa IR:\n{}", koopa_ir);
            let driver = koopa::front::Driver::from(koopa_ir);
            let program = driver.generate_program().unwrap();
            let asm = program.to_asm(&Background::new());
            std::fs::write(args.output, asm.to_string())?;
        }
    }
    Ok(())
}
