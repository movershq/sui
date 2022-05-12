// Copyright (c) 2021, Facebook, Inc. and its affiliates
// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    authority::AuthorityState,
    consensus_adapter::{ConsensusAdapter, ConsensusListenerMessage},
};
use async_trait::async_trait;
use futures::{stream::BoxStream, TryStreamExt};
use multiaddr::Multiaddr;
use std::{io, sync::Arc, time::Duration};
use sui_network::{
    api::{Validator, ValidatorServer},
    tonic,
};


use sui_types::{crypto::VerificationObligation, error::*, messages::*};
use tokio::sync::mpsc::Sender;

use sui_types::{messages_checkpoint::CheckpointRequest,};


use sui_types::{
    crypto::VerificationObligation,
    error::*,
    messages::*,
    messages_checkpoint::{CheckpointRequest, CheckpointResponse},
};
use tokio::{net::TcpListener, sync::mpsc::Sender};

use tracing::{info, Instrument};

#[cfg(test)]
#[path = "unit_tests/server_tests.rs"]
mod server_tests;

const MIN_BATCH_SIZE: u64 = 1000;
const MAX_DELAY_MILLIS: u64 = 5_000; // 5 sec

pub struct AuthorityServerHandle {
    tx_cancellation: tokio::sync::oneshot::Sender<()>,
    local_addr: Multiaddr,
    handle: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}

impl AuthorityServerHandle {
    pub async fn join(self) -> Result<(), std::io::Error> {
        // Note that dropping `self.complete` would terminate the server.
        self.handle
            .await?
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(())
    }

    pub async fn kill(self) -> Result<(), std::io::Error> {
        self.tx_cancellation.send(()).unwrap();
        self.handle
            .await?
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        Ok(())
    }

    pub fn address(&self) -> &Multiaddr {
        &self.local_addr
    }
}

pub struct AuthorityServer {
    address: Multiaddr,
    pub state: Arc<AuthorityState>,
    consensus_adapter: ConsensusAdapter,
    min_batch_size: u64,
    max_delay: Duration,
}

impl AuthorityServer {
    pub fn new(
        address: Multiaddr,
        state: Arc<AuthorityState>,
        consensus_address: Multiaddr,
        tx_consensus_listener: Sender<ConsensusListenerMessage>,
    ) -> Self {
        let consensus_adapter = ConsensusAdapter::new(
            consensus_address,
            state.committee.clone(),
            tx_consensus_listener,
            /* max_delay */ Duration::from_millis(2_000),
        );

        Self {
            address,
            state,
            consensus_adapter,
            min_batch_size: MIN_BATCH_SIZE,
            max_delay: Duration::from_millis(MAX_DELAY_MILLIS),
        }
    }

    /// Create a batch subsystem, register it with the authority state, and
    /// launch a task that manages it. Return the join handle of this task.
    pub async fn spawn_batch_subsystem(
        &self,
        min_batch_size: u64,
        max_delay: Duration,
    ) -> SuiResult<tokio::task::JoinHandle<SuiResult<()>>> {
        // Start the batching subsystem, and register the handles with the authority.
        let state = self.state.clone();
        let _batch_join_handle =
            tokio::task::spawn(
                async move { state.run_batch_service(min_batch_size, max_delay).await },
            );

        Ok(_batch_join_handle)
    }

    pub async fn spawn(self) -> Result<AuthorityServerHandle, io::Error> {
        let address = self.address.clone();
        self.spawn_with_bind_address(address).await
    }

    pub async fn spawn_with_bind_address(
        self,
        address: Multiaddr,
    ) -> Result<AuthorityServerHandle, io::Error> {
        // Start the batching subsystem
        let _join_handle = self
            .spawn_batch_subsystem(self.min_batch_size, self.max_delay)
            .await;

        let mut server = mysten_network::config::Config::new()
            .server_builder()
            .add_service(ValidatorServer::new(self))
            .bind(&address)
            .await
            .unwrap();
        let local_addr = server.local_addr().to_owned();
        info!("Listening to traffic on {local_addr}");
        let handle = AuthorityServerHandle {
            tx_cancellation: server.take_cancel_handle().unwrap(),
            local_addr,
            handle: tokio::spawn(server.serve()),
        };
        Ok(handle)
    }
}

#[async_trait]
impl Validator for AuthorityServer {
    async fn transaction(
        &self,
        request: tonic::Request<Transaction>,
    ) -> Result<tonic::Response<TransactionInfoResponse>, tonic::Status> {
        let mut transaction = request.into_inner();

        let mut obligation = VerificationObligation::default();
        transaction
            .add_tx_sig_to_verification_obligation(&mut obligation)
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        obligation
            .verify_all()
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        //TODO This is really really bad, we should have different types for signature-verified transactions
        transaction.is_verified = true;

        let tx_digest = transaction.digest();

        // Enable Trace Propagation across spans/processes using tx_digest
        let span = tracing::debug_span!(
            "process_tx",
            ?tx_digest,
            tx_kind = transaction.data.kind_as_str()
        );

        let info = self
            .state
            .handle_transaction(transaction)
            .instrument(span)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(info))
    }

    async fn confirmation_transaction(
        &self,
        request: tonic::Request<CertifiedTransaction>,
    ) -> Result<tonic::Response<TransactionInfoResponse>, tonic::Status> {
        let mut transaction = request.into_inner();

        let mut obligation = VerificationObligation::default();
        transaction
            .add_to_verification_obligation(&self.state.committee, &mut obligation)
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        obligation
            .verify_all()
            .map_err(|e| tonic::Status::invalid_argument(e.to_string()))?;
        //TODO This is really really bad, we should have different types for signature verified transactions
        transaction.is_verified = true;

        let tx_digest = transaction.digest();
        let span = tracing::debug_span!(
            "process_cert",
            ?tx_digest,
            tx_kind = transaction.data.kind_as_str()
        );

        let confirmation_transaction = ConfirmationTransaction {
            certificate: transaction,
        };

        let info = self
            .state
            .handle_confirmation_transaction(confirmation_transaction)
            .instrument(span)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(info))
    }

    async fn consensus_transaction(
        &self,
        request: tonic::Request<ConsensusTransaction>,
    ) -> Result<tonic::Response<TransactionInfoResponse>, tonic::Status> {
        let transaction = request.into_inner();

        let info = self
            .consensus_adapter
            .submit(&transaction)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(info))
    }

    async fn account_info(
        &self,
        request: tonic::Request<AccountInfoRequest>,
    ) -> Result<tonic::Response<AccountInfoResponse>, tonic::Status> {
        let request = request.into_inner();

        let response = self
            .state
            .handle_account_info_request(request)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(response))
    }

    async fn object_info(
        &self,
        request: tonic::Request<ObjectInfoRequest>,
    ) -> Result<tonic::Response<ObjectInfoResponse>, tonic::Status> {
        let request = request.into_inner();

        let response = self
            .state
            .handle_object_info_request(request)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(response))
    }

    async fn transaction_info(
        &self,
        request: tonic::Request<TransactionInfoRequest>,
    ) -> Result<tonic::Response<TransactionInfoResponse>, tonic::Status> {
        let request = request.into_inner();

        let response = self
            .state
            .handle_transaction_info_request(request)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        Ok(tonic::Response::new(response))
    }

    type BatchInfoStream = BoxStream<'static, Result<BatchInfoResponseItem, tonic::Status>>;

    async fn batch_info(
        &self,
        request: tonic::Request<BatchInfoRequest>,
    ) -> Result<tonic::Response<Self::BatchInfoStream>, tonic::Status> {
        let request = request.into_inner();

        let xstream = self
            .state
            .handle_batch_streaming(request)
            .await
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        let response = xstream.map_err(|e| tonic::Status::internal(e.to_string()));

        Ok(tonic::Response::new(Box::pin(response)))
    }

    async fn checkpoint(
        &self,
        request: tonic::Request<CheckpointRequest>,
    ) -> Result<tonic::Response<CheckpointResponse>, tonic::Status> {
        if let Some(checkpoint) = &self.state.checkpoints() {
            let request = request.into_inner();

            let response = checkpoint
                .handle_checkpoint_request(&request)
                .map_err(|e| tonic::Status::internal(e.to_string()))?;

            return Ok(tonic::Response::new(response));
        }

        Err(tonic::Status::internal("Unsupported".to_string()))
    }
}
