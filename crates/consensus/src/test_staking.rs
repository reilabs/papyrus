use std::fs::read_to_string;
use std::path::Path;

use cairo_lang_starknet_classes::casm_contract_class::CasmContractClass;
use indexmap::indexmap;
use lazy_static::lazy_static;
use papyrus_execution::execution_utils::selector_from_name;
use papyrus_execution::objects::{FunctionInvocationResult, TransactionTrace};
use papyrus_execution::test_utils::{execute_simulate_transactions, TxsScenarioBuilder};
use papyrus_storage::body::BodyStorageWriter;
use papyrus_storage::compiled_class::CasmStorageWriter;
use papyrus_storage::header::HeaderStorageWriter;
use papyrus_storage::state::StateStorageWriter;
use papyrus_storage::test_utils::get_test_storage;
use papyrus_storage::StorageWriter;
use serde::de::DeserializeOwned;
use starknet_api::block::{
    BlockBody,
    BlockHash,
    BlockHeader,
    BlockNumber,
    BlockTimestamp,
    GasPrice,
    GasPricePerToken,
};
use starknet_api::core::{
    ChainId,
    ClassHash,
    CompiledClassHash,
    ContractAddress,
    Nonce,
    PatriciaKey,
    SequencerContractAddress,
};
use starknet_api::deprecated_contract_class::ContractClass as DeprecatedContractClass;
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::state::{ContractClass, StateDiff};
use starknet_api::transaction::Calldata;
use starknet_api::{calldata, class_hash, contract_address, patricia_key, stark_felt};
use test_utils::read_json_file;

lazy_static! {
    pub static ref CHAIN_ID: ChainId = ChainId(String::from("TEST_CHAIN_ID"));
    pub static ref GAS_PRICE: GasPricePerToken = GasPricePerToken{
        price_in_wei: GasPrice(100 * u128::pow(10, 9)),
        // TODO(yair): add value and tests.
        price_in_fri: GasPrice::default(),
    };
    pub static ref BLOCK_TIMESTAMP: BlockTimestamp = BlockTimestamp(1234);
    pub static ref SEQUENCER_ADDRESS: SequencerContractAddress =
        SequencerContractAddress(contract_address!("0xa"));
    pub static ref ACCOUNT_ADDRESS: ContractAddress = contract_address!("0x444");
    pub static ref ACCOUNT_CLASS_HASH: ClassHash = class_hash!("0x333");
    pub static ref CONTRACT_ADDRESS: ContractAddress = contract_address!("0x2");
}

fn get_from_out_dir_json<T: DeserializeOwned>(fpath: &str) -> T {
    let path = Path::new(&std::env::var("OUT_DIR").unwrap()).join(fpath);
    let json_str = read_to_string(path.to_str().unwrap()).unwrap();
    let json = serde_json::from_str(&json_str).unwrap();
    serde_json::from_value(json).unwrap()
}
fn get_test_resource<T: DeserializeOwned>(path_in_resource_dir: &str) -> T {
    serde_json::from_value(read_json_file(path_in_resource_dir)).unwrap()
}
fn get_staking_casm() -> CasmContractClass {
    get_from_out_dir_json("staking.casm")
}
fn get_test_account_class() -> DeprecatedContractClass {
    get_test_resource("account_class.json")
}

// Create a storage with the staking contract and an account to send transactions from. No
// contructors are called for the contract.
fn prepare_storage(mut storage_writer: StorageWriter) {
    let staking_class_hash = class_hash!("0x1");

    storage_writer
        .begin_rw_txn()
        .unwrap()
        .append_header(
            BlockNumber(0),
            &BlockHeader {
                l1_gas_price: *GAS_PRICE,
                sequencer: *SEQUENCER_ADDRESS,
                timestamp: *BLOCK_TIMESTAMP,
                ..Default::default()
            },
        )
        .unwrap()
        .append_body(BlockNumber(0), BlockBody::default())
        .unwrap()
        .append_state_diff(
            BlockNumber(0),
            StateDiff {
                deployed_contracts: indexmap!(
                    *CONTRACT_ADDRESS => staking_class_hash,
                    *ACCOUNT_ADDRESS => *ACCOUNT_CLASS_HASH,
                ),
                storage_diffs: indexmap!(),
                declared_classes: indexmap!(
                    staking_class_hash =>
                    // The class is not used in the execution, so it can be default.
                    (CompiledClassHash::default(), ContractClass::default())
                ),
                deprecated_declared_classes: indexmap!(
                    *ACCOUNT_CLASS_HASH => get_test_account_class(),
                ),
                nonces: indexmap!(
                    *CONTRACT_ADDRESS => Nonce::default(),
                    *ACCOUNT_ADDRESS => Nonce::default(),
                ),
                replaced_classes: indexmap!(),
            },
            indexmap!(),
        )
        .unwrap()
        .append_casm(&staking_class_hash, &get_staking_casm())
        .unwrap()
        .append_header(
            BlockNumber(1),
            &BlockHeader {
                l1_gas_price: *GAS_PRICE,
                sequencer: *SEQUENCER_ADDRESS,
                timestamp: *BLOCK_TIMESTAMP,
                block_hash: BlockHash(stark_felt!(1_u128)),
                parent_hash: BlockHash(stark_felt!(0_u128)),
                ..Default::default()
            },
        )
        .unwrap()
        .append_body(BlockNumber(1), BlockBody::default())
        .unwrap()
        .append_state_diff(BlockNumber(1), StateDiff::default(), indexmap![])
        .unwrap()
        .commit()
        .unwrap();
}

fn expect_validators(validators: Vec<u32>) -> Calldata {
    let mut raw_calldata = vec![
        *CONTRACT_ADDRESS.0.key(),                 // contract_address
        selector_from_name("expect_validators").0, // Function selector.
    ];
    raw_calldata.push(stark_felt!(validators.len() as u128 + 1)); // Calldata length.
    raw_calldata.push(stark_felt!(validators.len() as u128)); // Input array length.
    raw_calldata.extend(validators.into_iter().map(|v| stark_felt!(v)));
    Calldata(raw_calldata.into())
}

#[test]
fn test_main() {
    let ((storage_reader, storage_writer), _temp_dir) = get_test_storage();
    prepare_storage(storage_writer);

    let expected = expect_validators(vec![1, 2]);
    let get_validators = calldata![
        *CONTRACT_ADDRESS.0.key(),              // contract_address
        selector_from_name("get_validators").0, // Function selector.
        stark_felt!(0_u8)                       // Calldata length.
    ];
    let tx = TxsScenarioBuilder::default()
        .invoke_deprecated(*ACCOUNT_ADDRESS, *CONTRACT_ADDRESS, None, false, Some(expected))
        .invoke_deprecated(*ACCOUNT_ADDRESS, *CONTRACT_ADDRESS, None, false, Some(get_validators))
        .collect();
    let mut exec_only_results =
        execute_simulate_transactions(storage_reader.clone(), None, tx.clone(), None, false, false);
    let TransactionTrace::Invoke(invocation) = exec_only_results.remove(1).transaction_trace else {
        panic!("Expected an invoke transaction");
    };
    let res = match invocation.execute_invocation {
        FunctionInvocationResult::Ok(res) => res.result.0,
        FunctionInvocationResult::Err(err) => panic!("{:?}", err),
    };
    // [Length, Elements...]
    assert_eq!(
        vec![StarkFelt::from(2 as u128), StarkFelt::from(1 as u128), StarkFelt::from(2 as u128)],
        res
    );
}
