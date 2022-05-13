// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use jsonrpsee::{
    http_client::{HttpClient, HttpClientBuilder},
    http_server::{HttpServerBuilder, HttpServerHandle},
};

use sui::{
    api::{RpcGatewayClient, RpcGatewayServer, SignedTransaction, TransactionBytes},
    config::{PersistedConfig, WalletConfig, SUI_GATEWAY_CONFIG, SUI_WALLET_CONFIG},
    keystore::{Keystore, SuiKeystore},
    rpc_gateway::{responses::ObjectResponse, RpcGatewayImpl},
    sui_commands::SuiNetwork,
};
use sui_core::gateway_state::GatewayTxSeqNumber;
use sui_core::gateway_types::{SuiObjectRead, TransactionEffectsResponse, TransactionResponse};
use sui_core::sui_json::SuiJsonValue;
use sui_framework::build_move_package_to_bytes;
use sui_types::sui_serde::Base64;
use sui_types::{
    base_types::{ObjectID, SuiAddress, TransactionDigest},
    SUI_FRAMEWORK_ADDRESS,
};

use crate::rpc_server_tests::sui_network::start_test_network;

mod sui_network;

#[tokio::test]
async fn test_get_objects() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();

    http_client.sync_account_state(*address).await?;
    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let result = result.objects;
    assert_eq!(5, result.len());
    Ok(())
}

#[tokio::test]
async fn test_transfer_coin() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();
    http_client.sync_account_state(*address).await?;
    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let objects = result.objects;

    let tx_data: TransactionBytes = http_client
        .transfer_coin(
            *address,
            objects.first().unwrap().object_id,
            Some(objects.last().unwrap().object_id),
            1000,
            *address,
        )
        .await?;

    let keystore = SuiKeystore::load_or_create(&test_network.working_dir.join("wallet.key"))?;
    let tx_bytes = tx_data.tx_bytes.to_vec()?;
    let signature = keystore.sign(address, &tx_bytes)?;

    let tx_response: TransactionResponse = http_client
        .execute_transaction(SignedTransaction::new(tx_bytes, signature))
        .await?;

    let effect = tx_response.to_effect_response()?.effects;
    assert_eq!(2, effect.mutated.len());

    Ok(())
}

#[tokio::test]
async fn test_publish() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();
    http_client.sync_account_state(*address).await?;
    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let objects = result.objects;

    let gas = objects.first().unwrap();

    let compiled_modules = build_move_package_to_bytes(
        Path::new("../sui_programmability/examples/fungible_tokens"),
        false,
    )?
    .iter()
    .map(|bytes| Base64::from_bytes(bytes))
    .collect::<Vec<_>>();

    let tx_data: TransactionBytes = http_client
        .publish(*address, compiled_modules, Some(gas.object_id), 10000)
        .await?;

    let keystore = SuiKeystore::load_or_create(&test_network.working_dir.join("wallet.key"))?;
    let tx_bytes = tx_data.tx_bytes.to_vec()?;
    let signature = keystore.sign(address, &tx_bytes)?;
    let tx_response: TransactionResponse = http_client
        .execute_transaction(SignedTransaction::new(tx_bytes, signature))
        .await?;

    let response = tx_response.to_publish_response()?;
    assert_eq!(2, response.created_objects.len());
    Ok(())
}

#[tokio::test]
async fn test_move_call() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();
    http_client.sync_account_state(*address).await?;
    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let objects = result.objects;

    let gas = objects.first().unwrap();

    let package_id = ObjectID::new(SUI_FRAMEWORK_ADDRESS.into_bytes());
    let module = "ObjectBasics".to_string();
    let function = "create".to_string();

    let json_args = vec![
        SuiJsonValue::from_str("10000")?,
        SuiJsonValue::from_str(&format!("{:#x}", address))?,
    ];

    let tx_data: TransactionBytes = http_client
        .move_call(
            *address,
            package_id,
            module,
            function,
            vec![],
            json_args,
            Some(gas.object_id),
            1000,
        )
        .await?;

    let keystore = SuiKeystore::load_or_create(&test_network.working_dir.join("wallet.key"))?;
    let tx_bytes = tx_data.tx_bytes.to_vec()?;
    let signature = keystore.sign(address, &tx_bytes)?;

    let tx_response: TransactionResponse = http_client
        .execute_transaction(SignedTransaction::new(tx_bytes, signature))
        .await?;

    let effect = tx_response.to_effect_response()?.effects;
    assert_eq!(1, effect.created.len());
    Ok(())
}

#[tokio::test]
async fn test_get_object_info() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();
    http_client.sync_account_state(*address).await?;
    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let result = result.objects;

    for oref in result {
        let result: SuiObjectRead = http_client.get_object_info(oref.object_id).await?;
        assert!(
            matches!(result, SuiObjectRead::Exists(object) if oref.object_id == object.id() && &object.owner.get_owner_address()? == address)
        );
    }
    Ok(())
}

#[tokio::test]
async fn test_get_transaction() -> Result<(), anyhow::Error> {
    let test_network = setup_test_network().await?;
    let http_client = test_network.http_client;
    let address = test_network.accounts.first().unwrap();

    http_client.sync_account_state(*address).await?;

    let result: ObjectResponse = http_client.get_owned_objects(*address).await?;
    let objects = result.objects;

    let gas_id = objects.last().unwrap().object_id;

    // Make some transactions
    let mut tx_responses = Vec::new();
    for oref in &objects[..objects.len() - 1] {
        let tx_data: TransactionBytes = http_client
            .transfer_coin(*address, oref.object_id, Some(gas_id), 1000, *address)
            .await?;

        let keystore = SuiKeystore::load_or_create(&test_network.working_dir.join("wallet.key"))?;
        let tx_bytes = tx_data.tx_bytes.to_vec()?;
        let signature = keystore.sign(address, &tx_bytes)?;

        let response: TransactionResponse = http_client
            .execute_transaction(SignedTransaction::new(tx_bytes, signature))
            .await?;

        if let TransactionResponse::EffectResponse(effects) = response {
            tx_responses.push(effects);
        }
    }
    // test get_transactions_in_range
    let tx: Vec<(GatewayTxSeqNumber, TransactionDigest)> =
        http_client.get_transactions_in_range(0, 10).await?;
    assert_eq!(4, tx.len());

    // test get_transactions_in_range with smaller range
    let tx: Vec<(GatewayTxSeqNumber, TransactionDigest)> =
        http_client.get_transactions_in_range(1, 3).await?;
    assert_eq!(2, tx.len());

    // test get_recent_transactions with smaller range
    let tx: Vec<(GatewayTxSeqNumber, TransactionDigest)> =
        http_client.get_recent_transactions(3).await?;
    assert_eq!(3, tx.len());

    // test get_recent_transactions
    let tx: Vec<(GatewayTxSeqNumber, TransactionDigest)> =
        http_client.get_recent_transactions(10).await?;
    assert_eq!(4, tx.len());

    // test get_transaction
    for (_, tx_digest) in tx {
        let response: TransactionEffectsResponse = http_client.get_transaction(tx_digest).await?;
        assert!(tx_responses.iter().any(
            |effects| effects.effects.transaction_digest == response.effects.transaction_digest
        ))
    }

    Ok(())
}

async fn setup_test_network() -> Result<TestNetwork, anyhow::Error> {
    let working_dir = tempfile::tempdir()?.path().to_path_buf();
    let _network = start_test_network(&working_dir, None, None).await?;
    let (server_addr, rpc_server_handle) =
        start_rpc_gateway(&working_dir.join(SUI_GATEWAY_CONFIG)).await?;
    let wallet_conf: WalletConfig = PersistedConfig::read(&working_dir.join(SUI_WALLET_CONFIG))?;
    let http_client = HttpClientBuilder::default().build(format!("http://{}", server_addr))?;
    Ok(TestNetwork {
        _network,
        _rpc_server: rpc_server_handle,
        accounts: wallet_conf.accounts,
        http_client,
        working_dir,
    })
}

struct TestNetwork {
    _network: SuiNetwork,
    _rpc_server: HttpServerHandle,
    accounts: Vec<SuiAddress>,
    http_client: HttpClient,
    working_dir: PathBuf,
}

async fn start_rpc_gateway(
    config_path: &Path,
) -> Result<(SocketAddr, HttpServerHandle), anyhow::Error> {
    let server = HttpServerBuilder::default().build("127.0.0.1:0").await?;
    let addr = server.local_addr()?;
    let handle = server.start(RpcGatewayImpl::new(config_path)?.into_rpc())?;
    Ok((addr, handle))
}
