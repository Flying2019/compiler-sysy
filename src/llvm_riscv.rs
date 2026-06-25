use std::path::Path;
use std::process::Command;

pub fn compile_llvm_to_riscv_asm(llvm_ir: &str, output: &Path) -> std::io::Result<()> {
    let tmp_dir = std::env::temp_dir();
    let unique = format!("compile_sysy_{}_{}", std::process::id(), unique_suffix());
    let input = tmp_dir.join(format!("{}.ll", unique));
    std::fs::write(&input, llvm_ir)?;
    let clang = std::env::var("SYSY_CLANG").unwrap_or_else(|_| default_clang());
    let target =
        std::env::var("SYSY_RISCV_TARGET").unwrap_or_else(|_| "riscv64-unknown-elf".to_string());
    let march = std::env::var("SYSY_RISCV_MARCH").unwrap_or_else(|_| "rv64im".to_string());
    let abi = std::env::var("SYSY_RISCV_ABI").unwrap_or_else(|_| "lp64".to_string());
    compile_riscv_asm_with_clang(&clang, &target, &march, &abi, &input, output)?;

    let _ = std::fs::remove_file(&input);
    Ok(())
}

fn compile_riscv_asm_with_clang(
    clang: &str,
    target: &str,
    march: &str,
    abi: &str,
    input: &Path,
    output: &Path,
) -> std::io::Result<()> {
    let status = Command::new(clang)
        .arg("-target")
        .arg(target)
        .arg(format!("-march={}", march))
        .arg(format!("-mabi={}", abi))
        .arg("-msmall-data-limit=0")
        .arg("-S")
        .arg("-ffreestanding")
        .arg("-fno-addrsig")
        .arg("-nostdlib")
        .arg("-O0")
        .arg(input)
        .arg("-o")
        .arg(output)
        .status()?;
    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "{} failed to compile {} to RISC-V assembly",
                clang,
                input.display()
            ),
        ));
    }
    Ok(())
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn default_clang() -> String {
    if Command::new("clang-18")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        "clang-18".to_string()
    } else {
        "clang".to_string()
    }
}
