// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0
use crate::{test_committee, test_keys};
use narwhal_config::Parameters as ConsensusParameters;
use std::{path::PathBuf, sync::Arc};
use sui::{
    config::{make_default_narwhal_committee, utils::get_available_port, AuthorityInfo},
    sui_commands::make_authority,
};
use sui_adapter::genesis;
use sui_core::{
    authority::{AuthorityState, AuthorityStore},
    authority_server::AuthorityServerHandle,
};
use sui_types::{crypto::KeyPair, object::Object};

/// The default network buffer size of a test authority.
pub const NETWORK_BUFFER_SIZE: usize = 65_000;

/// Make a test authority store in a temporary directory.
pub fn test_authority_store() -> AuthorityStore {
    let store_path = tempfile::tempdir().unwrap();
    AuthorityStore::open(store_path, None)
}

/// Make an authority config for each of the `TEST_COMMITTEE_SIZE` authorities in the test committee.
pub fn test_authority_configs() -> (Vec<AuthorityInfo>, Vec<KeyPair>) {
    let test_keys = test_keys();
    let key_pair = test_keys
        .iter()
        .map(|(_, key_pair)| key_pair.copy())
        .collect();
    let authorities = test_keys
        .into_iter()
        .map(|(address, key)| {
            let authority_port = get_available_port();
            let consensus_port = get_available_port();

            AuthorityInfo {
                address,
                public_key: *key.public_key_bytes(),
                network_address: format!("/ip4/127.0.0.1/tcp/{authority_port}/http")
                    .parse()
                    .unwrap(),
                db_path: PathBuf::new(),
                stake: 1,
                consensus_address: format!("/ip4/127.0.0.1/tcp/{consensus_port}/http")
                    .parse()
                    .unwrap(),
            }
        })
        .collect();
    (authorities, key_pair)
}

/// Make a test authority state for each committee member.
pub async fn test_authority_states<I>(objects: I) -> Vec<AuthorityState>
where
    I: IntoIterator<Item = Object> + Clone,
{
    let committee = test_committee();
    let mut authorities = Vec::new();
    for (_, key) in test_keys() {
        let state = AuthorityState::new(
            committee.clone(),
            *key.public_key_bytes(),
            Arc::pin(key),
            Arc::new(test_authority_store()),
            None,
            genesis::clone_genesis_compiled_modules(),
            &mut genesis::get_genesis_context(),
        )
        .await;

        for o in objects.clone() {
            state.insert_genesis_object(o).await;
        }

        authorities.push(state);
    }
    authorities
}

/// Spawn all authorities in the test committee into a separate tokio task.
pub async fn spawn_test_authorities<I>(
    objects: I,
    authorities: &[AuthorityInfo],
    key_pairs: &[KeyPair],
) -> Vec<AuthorityServerHandle>
where
    I: IntoIterator<Item = Object> + Clone,
{
    let states = test_authority_states(objects).await;
    let consensus_committee = make_default_narwhal_committee(authorities).unwrap();
    let mut handles = Vec::new();
    for ((state, config), key_pair) in states
        .into_iter()
        .zip(authorities.iter())
        .zip(key_pairs.iter())
    {
        let consensus_parameters = ConsensusParameters {
            max_header_delay: std::time::Duration::from_millis(200),
            header_size: 1,
            ..ConsensusParameters::default()
        };
        let handle = make_authority(
            /* authority */ config,
            key_pair,
            state,
            &consensus_committee,
            /* consensus_store_path */ tempfile::tempdir().unwrap().path(),
            &consensus_parameters,
            /* net_parameters */ None,
        )
        .await
        .unwrap()
        .spawn()
        .await
        .unwrap();
        handles.push(handle);
    }
    handles
}
