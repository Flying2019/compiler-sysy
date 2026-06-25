use clap::{Parser, ValueEnum};
use compile_sysy::lalr::CompUnit;
use compile_sysy::lexer::{LexError, Lexer, Tok};
use compile_sysy::llvm_ir::{
    compile_llvm_to_riscv_asm, try_compile_to_llvm, try_compile_to_llvm_with_target,
    DEFAULT_RISCV_TARGET,
};
use lalrpop_util::lalrpop_mod;
use lalrpop_util::ParseError;
use std::env;
use std::io::{self, Result};
use std::path::Path;
use std::{fs::read_to_string, path::PathBuf};

#[derive(ValueEnum, Clone, Debug)]
enum Mode {
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

    #[arg(long, default_value = DEFAULT_RISCV_TARGET)]
    target: String,
}

lalrpop_mod!(sysy);

// 为了支持非标准的 -llvm 和 -riscv 形式进行的预处理
fn preprocess_args() -> Vec<String> {
    let mut raw_args: Vec<String> = env::args().collect();
    for arg in raw_args.iter_mut() {
        if arg == "-llvm" {
            *arg = "--mode=llvm".to_string();
        } else if arg == "-riscv" {
            *arg = "--mode=riscv".to_string();
        }
    }
    raw_args
}

fn str_to_ast(input: &str, path: &Path) -> Result<CompUnit> {
    sysy::CompUnitParser::new()
        .parse(Lexer::new(input))
        .map_err(|err| {
            let (location, reason) = describe_parse_error(err);
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format_source_error(path, input, location, reason),
            )
        })
}

fn main() -> Result<()> {
    let args = Args::parse_from(preprocess_args());
    match args.mode {
        Mode::Llvm => {
            let input = read_to_string(&args.input)?;
            let ast = str_to_ast(&input, &args.input)?;
            let target = resolve_llvm_target(&args.target);
            let llvm_ir = try_compile_to_llvm_with_target(&ast, &target).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format_source_error(&args.input, &input, 0, err),
                )
            })?;
            std::fs::write(args.output, llvm_ir)?;
        }
        Mode::Riscv => {
            let input = read_to_string(&args.input)?;
            let ast = str_to_ast(&input, &args.input)?;
            let llvm_ir = try_compile_to_llvm(&ast).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format_source_error(&args.input, &input, 0, err),
                )
            })?;
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
        _ => DEFAULT_RISCV_TARGET.to_string(),
    }
}

fn describe_parse_error(err: ParseError<usize, Tok, LexError>) -> (usize, String) {
    match err {
        ParseError::InvalidToken { location } => {
            (location, "parse error: invalid token".to_string())
        }
        ParseError::UnrecognizedEof { location, expected } => (
            location,
            format!(
                "parse error: unexpected end of file{}",
                format_expected_tokens(&expected)
            ),
        ),
        ParseError::UnrecognizedToken { token, expected } => (
            token.0,
            format!(
                "parse error: unexpected token {:?}{}",
                token.1,
                format_expected_tokens(&expected)
            ),
        ),
        ParseError::ExtraToken { token } => {
            (token.0, format!("parse error: extra token {:?}", token.1))
        }
        ParseError::User { error } => (0, format!("parse error: lexer error: {error:?}")),
    }
}

fn format_expected_tokens(expected: &[String]) -> String {
    if expected.is_empty() {
        String::new()
    } else {
        format!(", expected one of: {}", expected.join(", "))
    }
}

fn format_source_error(path: &Path, input: &str, byte_offset: usize, reason: String) -> String {
    let (line, column) = line_column(input, byte_offset);
    format!("{}:{}:{}: {}", path.display(), line, column, reason)
}

fn line_column(input: &str, byte_offset: usize) -> (usize, usize) {
    let clamped = byte_offset.min(input.len());
    let mut line = 1;
    let mut line_start = 0;
    for (idx, ch) in input.char_indices() {
        if idx >= clamped {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = idx + ch.len_utf8();
        }
    }
    let column = input[line_start..clamped].chars().count() + 1;
    (line, column)
}
