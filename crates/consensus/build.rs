use std::env;
use std::io::Result;
use std::path::PathBuf;

use cairo_lang_compiler::CompilerConfig;
use cairo_lang_starknet_classes::casm_contract_class::CasmContractClass;

fn main() -> Result<()> {
    let cairo_file = PathBuf::from("src/staking.cairo");
    let casm_output = PathBuf::from(env::var("OUT_DIR").unwrap()).join("staking.casm");
    println!("cargo::rerun-if-changed={}", cairo_file.to_str().unwrap());
    println!("cargo::rerun-if-changed=Scarb.toml");

    let sierra_program =
        cairo_lang_starknet::compile::compile_path(&cairo_file, None, CompilerConfig::default())
            .expect("Failed compiling sierra.");
    let casm_contract = CasmContractClass::from_contract_class(
        sierra_program,
        false,  // default from starknet-sierra-compile
        180000, // default from starknet-sierra-compile
    )
    .expect("Failed compiling casm");

    let casm_ser =
        serde_json::to_string_pretty(&casm_contract).expect("Casm contract Serialization failed.");

    std::fs::write(casm_output, casm_ser).expect("Failed writing casm file.");
    Ok(())
}
