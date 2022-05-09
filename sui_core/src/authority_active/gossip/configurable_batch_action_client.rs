use crate::authority::AuthorityState;
use crate::authority_client::{AuthorityAPI, BatchInfoResponseItemStream};
use async_trait::async_trait;
use std::sync::Arc;
use sui_types::base_types::TransactionDigest;
use sui_types::batch::{AuthorityBatch, SignedBatch, TxSequenceNumber, UpdateItem};
use sui_types::error::SuiError;
use sui_types::messages::{
    AccountInfoRequest, AccountInfoResponse, BatchInfoRequest, BatchInfoResponseItem,
    ConfirmationTransaction, ConsensusTransaction, ObjectInfoRequest, ObjectInfoResponse,
    Transaction, TransactionInfoRequest, TransactionInfoResponse,
};
use tokio::time::{Duration, Instant};

#[derive(Clone)]
pub enum BatchAction {
    DoNothing(Duration),
    EmitBatch(TxSequenceNumber),
}

pub struct ConfigurableBatchActionClient {
    state: Arc<AuthorityState>,
    pub batch_size: i32,
    pub action_sequence: Vec<BatchAction>,
    test_time: Instant,
}

impl ConfigurableBatchActionClient {
    pub fn new(state: Arc<AuthorityState>, batch_size: i32) -> Self {
        ConfigurableBatchActionClient {
            state,
            batch_size,
            action_sequence: Vec::new(),
            test_time: Instant::now(),
        }
    }
    pub fn register_action_sequence(mut self, action_sequence: Vec<BatchAction>) {
        self.action_sequence = action_sequence;
    }
}

#[async_trait]
impl AuthorityAPI for ConfigurableBatchActionClient {
    async fn handle_transaction(
        &self,
        _transaction: Transaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        Ok(TransactionInfoResponse {
            signed_transaction: None,
            certified_transaction: None,
            signed_effects: None,
        })
    }

    async fn handle_confirmation_transaction(
        &self,
        _transaction: ConfirmationTransaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        Ok(TransactionInfoResponse {
            signed_transaction: None,
            certified_transaction: None,
            signed_effects: None,
        })
    }

    async fn handle_consensus_transaction(
        &self,
        _transaction: ConsensusTransaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        Ok(TransactionInfoResponse {
            signed_transaction: None,
            certified_transaction: None,
            signed_effects: None,
        })
    }

    async fn handle_account_info_request(
        &self,
        _request: AccountInfoRequest,
    ) -> Result<AccountInfoResponse, SuiError> {
        Ok(AccountInfoResponse {
            object_ids: vec![],
            owner: Default::default(),
        })
    }

    async fn handle_object_info_request(
        &self,
        _request: ObjectInfoRequest,
    ) -> Result<ObjectInfoResponse, SuiError> {
        Ok(ObjectInfoResponse {
            parent_certificate: None,
            requested_object_reference: None,
            object_and_lock: None,
        })
    }

    /// Handle Object information requests for this account.
    async fn handle_transaction_info_request(
        &self,
        _request: TransactionInfoRequest,
    ) -> Result<TransactionInfoResponse, SuiError> {
        Ok(TransactionInfoResponse {
            signed_transaction: None,
            certified_transaction: None,
            signed_effects: None,
        })
    }

    /// Handle Batch information requests for this authority.
    async fn handle_batch_stream(
        &self,
        _request: BatchInfoRequest,
    ) -> Result<BatchInfoResponseItemStream, SuiError> {
        let mut last_batch = AuthorityBatch::initial();
        let actions = &self.action_sequence;
        let secret = self.state.secret.clone();
        let name = self.state.name;
        let mut items: Vec<Result<BatchInfoResponseItem, SuiError>> = Vec::new();

        let _ = actions.into_iter().for_each(|action| {
            match action {
                BatchAction::EmitBatch(start_seq) => {
                    let mut seq = start_seq.clone();
                    let mut transactions = Vec::new();
                    for _i in 0..self.batch_size {
                        let rnd = TransactionDigest::random();
                        transactions.push((seq, rnd));
                        items.push(Ok(BatchInfoResponseItem(UpdateItem::Transaction((
                            seq, rnd,
                        )))));
                        seq += 1;
                    }
                    let new_batch = AuthorityBatch::make_next(&last_batch, &transactions).unwrap();
                    last_batch = new_batch;
                    items.push({
                        let item = SignedBatch::new(last_batch.clone(), &*secret, name);
                        Ok(BatchInfoResponseItem(UpdateItem::Batch(item)))
                    });
                }
                BatchAction::DoNothing(_d) => {}
            };
        });

        Ok(Box::pin(tokio_stream::iter(items)))
    }
}
