// Copyright (c) 2021, Facebook, Inc. and its affiliates
// Copyright (c) 2022, Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    authority_batch::{BroadcastReceiver, BroadcastSender},
    execution_engine, transaction_input_checker,
};
use async_trait::async_trait;
use itertools::Itertools;
use move_binary_format::CompiledModule;
use move_bytecode_utils::module_cache::ModuleCache;
use move_core_types::{
    language_storage::{ModuleId, StructTag},
    resolver::{ModuleResolver, ResourceResolver},
};
use move_vm_runtime::{move_vm::MoveVM, native_functions::NativeFunctionTable};
use narwhal_executor::{ExecutionIndices, ExecutionState};
use once_cell::sync::Lazy;
use prometheus_exporter::prometheus::{
    register_histogram, register_int_counter, Histogram, IntCounter,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use sui_adapter::adapter;
use sui_types::{
    base_types::*,
    batch::{TxSequenceNumber, UpdateItem},
    committee::Committee,
    crypto::AuthoritySignature,
    error::{SuiError, SuiResult},
    fp_bail, fp_ensure,
    gas::SuiGasStatus,
    messages::*,
    object::{Data, Object},
    storage::{BackingPackageStore, DeleteKind, Storage},
    MOVE_STDLIB_ADDRESS, SUI_FRAMEWORK_ADDRESS,
};
use tracing::{debug, instrument, log};

#[cfg(test)]
#[path = "unit_tests/authority_tests.rs"]
pub mod authority_tests;

#[cfg(test)]
#[path = "unit_tests/batch_transaction_tests.rs"]
mod batch_transaction_tests;

#[cfg(test)]
#[path = "unit_tests/move_integration_tests.rs"]
pub mod move_integration_tests;

#[cfg(test)]
#[path = "unit_tests/gas_tests.rs"]
mod gas_tests;

mod temporary_store;
pub use temporary_store::AuthorityTemporaryStore;

mod authority_store;
pub use authority_store::{AuthorityStore, GatewayStore, SuiDataStore};

pub mod authority_notifier;

const MAX_ITEMS_LIMIT: u64 = 100_000;
const BROADCAST_CAPACITY: usize = 10_000;

/// Prometheus metrics which can be displayed in Grafana, queried and alerted on
pub struct AuthorityMetrics {
    tx_orders: IntCounter,
    total_certs: IntCounter,
    total_effects: IntCounter,
    total_events: IntCounter,
    signature_errors: IntCounter,
    pub shared_obj_tx: IntCounter,
    tx_already_processed: IntCounter,
    num_input_objs: Histogram,
    num_shared_objects: Histogram,
    batch_size: Histogram,
}

// Override default Prom buckets for positive numbers in 0-50k range
const POSITIVE_INT_BUCKETS: &[f64] = &[
    1., 2., 5., 10., 20., 50., 100., 200., 500., 1000., 2000., 5000., 10000., 20000., 50000.,
];

impl AuthorityMetrics {
    pub fn new() -> AuthorityMetrics {
        Self {
            tx_orders: register_int_counter!(
                "total_transaction_orders",
                "Total number of transaction orders"
            )
            .unwrap(),
            total_certs: register_int_counter!(
                "total_transaction_certificates",
                "Total number of transaction certificates handled"
            )
            .unwrap(),
            // total_effects == total transactions finished
            total_effects: register_int_counter!(
                "total_transaction_effects",
                "Total number of transaction effects produced"
            )
            .unwrap(),
            total_events: register_int_counter!("total_events", "Total number of events produced")
                .unwrap(),
            signature_errors: register_int_counter!(
                "total_signature_errors",
                "Number of transaction signature errors"
            )
            .unwrap(),
            shared_obj_tx: register_int_counter!(
                "num_shared_obj_tx",
                "Number of transactions involving shared objects"
            )
            .unwrap(),
            tx_already_processed: register_int_counter!(
                "num_tx_already_processed",
                "Number of transaction orders already processed previously"
            )
            .unwrap(),
            num_input_objs: register_histogram!(
                "num_input_objects",
                "Distribution of number of input TX objects per TX",
                POSITIVE_INT_BUCKETS.to_vec()
            )
            .unwrap(),
            num_shared_objects: register_histogram!(
                "num_shared_objects",
                "Number of shared input objects per TX",
                POSITIVE_INT_BUCKETS.to_vec()
            )
            .unwrap(),
            batch_size: register_histogram!(
                "batch_size",
                "Distribution of size of transaction batch",
                POSITIVE_INT_BUCKETS.to_vec()
            )
            .unwrap(),
        }
    }
}

impl Default for AuthorityMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// One cannot register a metric multiple times.  We protect initialization with lazy_static
// for cases such as local tests or "sui start" which starts multiple authorities in one process.
pub static METRICS: Lazy<AuthorityMetrics> = Lazy::new(AuthorityMetrics::new);

/// a Trait object for `signature::Signer` that is:
/// - Pin, i.e. confined to one place in memory (we don't want to copy private keys).
/// - Sync, i.e. can be safely shared between threads.
///
/// Typically instantiated with Box::pin(keypair) where keypair is a `KeyPair`
///
pub type StableSyncAuthoritySigner =
    Pin<Arc<dyn signature::Signer<AuthoritySignature> + Send + Sync>>;

pub struct AuthorityState {
    // Fixed size, static, identity of the authority
    /// The name of this authority.
    pub name: AuthorityName,
    /// Committee of this Sui instance.
    pub committee: Committee,
    /// The signature key of the authority.
    pub secret: StableSyncAuthoritySigner,

    /// Move native functions that are available to invoke
    _native_functions: NativeFunctionTable,
    move_vm: Arc<MoveVM>,

    /// The database
    pub(crate) _database: Arc<AuthorityStore>, // TODO: remove pub

    // Structures needed for handling batching and notifications.
    /// The sender to notify of new transactions
    /// and create batches for this authority.
    /// Keep as None if there is no need for this.
    pub(crate) batch_channels: BroadcastSender, // TODO: remove pub

    // The Transaction notifier ticketing engine.
    pub(crate) batch_notifier: Arc<authority_notifier::TransactionNotifier>, // TODO: remove pub

    /// Ensures there can only be a single consensus client is updating the state.
    pub consensus_guardrail: AtomicUsize,

    pub metrics: &'static AuthorityMetrics,
}

/// The authority state encapsulates all state, drives execution, and ensures safety.
///
/// Note the authority operations can be accessed through a read ref (&) and do not
/// require &mut. Internally a database is synchronized through a mutex lock.
///
/// Repeating valid commands should produce no changes and return no error.
impl AuthorityState {
    /// Get a broadcast receiver for updates
    pub fn subscribe_batch(&self) -> BroadcastReceiver {
        self.batch_channels.subscribe()
    }

    async fn handle_transaction_impl(
        &self,
        transaction: Transaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        let transaction_digest = *transaction.digest();
        // Ensure an idempotent answer.
        if self._database.transaction_exists(&transaction_digest)? {
            self.metrics.tx_already_processed.inc();
            let transaction_info = self.make_transaction_info(&transaction_digest).await?;
            return Ok(transaction_info);
        }

        let (_gas_status, all_objects) = transaction_input_checker::check_transaction_input(
            &self._database,
            &transaction,
            &self.metrics.shared_obj_tx,
        )
        .await?;

        let owned_objects = transaction_input_checker::filter_owned_objects(&all_objects);

        let signed_transaction =
            SignedTransaction::new(self.committee.epoch, transaction, self.name, &*self.secret);

        // Check and write locks, to signed transaction, into the database
        // The call to self.set_transaction_lock checks the lock is not conflicting,
        // and returns ConflictingTransaction error in case there is a lock on a different
        // existing transaction.
        self.set_transaction_lock(&owned_objects, signed_transaction)
            .await?;

        // Return the signed Transaction or maybe a cert.
        self.make_transaction_info(&transaction_digest).await
    }

    /// Initiate a new transaction.
    pub async fn handle_transaction(
        &self,
        transaction: Transaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        self.metrics.tx_orders.inc();
        // Check the sender's signature.
        transaction.verify_signature().map_err(|e| {
            self.metrics.signature_errors.inc();
            e
        })?;
        let transaction_digest = *transaction.digest();

        let response = self.handle_transaction_impl(transaction).await;
        match response {
            Ok(r) => Ok(r),
            // If we see an error, it is possible that a certificate has already been processed.
            // In that case, we could still return Ok to avoid showing confusing errors.
            Err(err) => {
                if self._database.effects_exists(&transaction_digest)? {
                    self.metrics.tx_already_processed.inc();
                    Ok(self.make_transaction_info(&transaction_digest).await?)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Confirm a transfer.
    pub async fn handle_confirmation_transaction(
        &self,
        confirmation_transaction: ConfirmationTransaction,
    ) -> SuiResult<TransactionInfoResponse> {
        self.metrics.total_certs.inc();
        let transaction_digest = *confirmation_transaction.certificate.digest();

        // Ensure an idempotent answer.
        if self._database.effects_exists(&transaction_digest)? {
            let info = self.make_transaction_info(&transaction_digest).await?;
            debug!("Transaction {transaction_digest:?} already executed");
            return Ok(info);
        }

        // Check the certificate and retrieve the transfer data.
        tracing::trace_span!("cert_check_signature")
            .in_scope(|| confirmation_transaction.certificate.verify(&self.committee))
            .map_err(|e| {
                self.metrics.signature_errors.inc();
                e
            })?;

        self.process_certificate(confirmation_transaction).await
    }

    #[instrument(level = "trace", skip_all)]
    async fn check_shared_locks(
        &self,
        transaction_digest: &TransactionDigest,
        // inputs: &[(InputObjectKind, Object)],
        shared_object_refs: &[ObjectRef],
    ) -> Result<(), SuiError> {
        debug!("Validating shared object sequence numbers from consensus...");

        // Internal consistency check
        debug_assert!(
            !shared_object_refs.is_empty(),
            "we just checked that there are share objects yet none found?"
        );

        let shared_locks: HashMap<_, _> = self
            ._database
            .all_shared_locks(transaction_digest)?
            .into_iter()
            .collect();

        // Check whether the shared objects have already been assigned a sequence number by
        // the consensus. Bail if the transaction contains even one shared object that either:
        // (i) was not assigned a sequence number, or
        // (ii) has a different sequence number than the current one.

        let lock_errors: Vec<_> = shared_object_refs
            .iter()
            .filter_map(|(object_id, version, _)| {
                if !shared_locks.contains_key(object_id) {
                    Some(SuiError::SharedObjectLockNotSetObject)
                } else if shared_locks[object_id] != *version {
                    Some(SuiError::UnexpectedSequenceNumber {
                        object_id: *object_id,
                        // This sequence number is the one attributed by consensus.
                        expected_sequence: shared_locks[object_id],
                        // This sequence number is the one we currently have in the database.
                        given_sequence: *version,
                    })
                } else {
                    None
                }
            })
            .collect();

        fp_ensure!(
            lock_errors.is_empty(),
            // NOTE: the error message here will say 'Error acquiring lock' but what it means is
            // 'error checking lock'.
            SuiError::LockErrors {
                errors: lock_errors
            }
        );

        Ok(())
    }

    #[instrument(level = "debug", name = "process_cert_inner", skip_all)]
    async fn process_certificate(
        &self,
        confirmation_transaction: ConfirmationTransaction,
    ) -> Result<TransactionInfoResponse, SuiError> {
        let certificate = confirmation_transaction.certificate;
        let transaction_digest = *certificate.digest();

        let (gas_status, objects_by_kind) = transaction_input_checker::check_transaction_input(
            &self._database,
            &certificate,
            &self.metrics.shared_obj_tx,
        )
        .await?;

        // At this point we need to check if any shared objects need locks,
        // and whether they have them.
        let shared_object_refs: Vec<_> = objects_by_kind
            .iter()
            .filter(|(kind, _)| matches!(kind, InputObjectKind::SharedMoveObject(_)))
            .map(|(_, obj)| obj.compute_object_reference())
            .sorted()
            .collect();
        if !shared_object_refs.is_empty() {
            // If the transaction contains shared objects, we need to ensure they have been scheduled
            // for processing by the consensus protocol.
            self.check_shared_locks(&transaction_digest, &shared_object_refs)
                .await?;
        }

        self.metrics
            .num_input_objs
            .observe(objects_by_kind.len() as f64);
        self.metrics
            .num_shared_objects
            .observe(shared_object_refs.len() as f64);
        self.metrics
            .batch_size
            .observe(certificate.data.kind.batch_size() as f64);
        debug!(
            num_inputs = objects_by_kind.len(),
            "Read inputs for transaction from DB"
        );

        let transaction_dependencies = objects_by_kind
            .iter()
            .map(|(_, obj)| obj.previous_transaction)
            .collect();
        let mut temporary_store = AuthorityTemporaryStore::new(
            self._database.clone(),
            objects_by_kind,
            transaction_digest,
        );
        let effects = execution_engine::execute_transaction_to_effects(
            shared_object_refs,
            &mut temporary_store,
            certificate.data.clone(),
            transaction_digest,
            transaction_dependencies,
            &self.move_vm,
            &self._native_functions,
            gas_status,
        )?;

        self.metrics.total_effects.inc();
        self.metrics
            .total_events
            .inc_by(effects.events.len() as u64);

        // TODO: Distribute gas charge and rebate, which can be retrieved from effects.
        let signed_effects =
            effects.to_sign_effects(self.committee.epoch, &self.name, &*self.secret);

        // Update the database in an atomic manner
        self.update_state(temporary_store, &certificate, &signed_effects)
            .await?;

        Ok(TransactionInfoResponse {
            signed_transaction: self._database.get_transaction(&transaction_digest)?,
            certified_transaction: Some(certificate),
            signed_effects: Some(signed_effects),
        })
    }

    /// Process certificates coming from the consensus. It is crucial that this function is only
    /// called by a single task (ie. the task handling consensus outputs).
    pub async fn handle_consensus_certificate(
        &self,
        certificate: CertifiedTransaction,
        last_consensus_index: ExecutionIndices,
    ) -> SuiResult<()> {
        // Ensure it is a shared object certificate
        if !certificate.contains_shared_object() {
            log::debug!(
                "Transaction without shared object has been sequenced: {:?}",
                certificate
            );
            return Ok(());
        }

        // Ensure it is the first time we see this certificate.
        let transaction_digest = *certificate.digest();
        if self
            ._database
            .sequenced(&transaction_digest, certificate.shared_input_objects())?[0]
            .is_some()
        {
            return Ok(());
        }

        // Check the certificate.
        certificate.verify(&self.committee)?;

        // Persist the certificate since we are about to lock one or more shared object.
        // We thus need to make sure someone (if not the client) can continue the protocol.
        // Also atomically lock the shared objects for this particular transaction and
        // increment the last consensus index. Note that a single process can ever call
        // this function and that the last consensus index is also kept in memory. It is
        // thus ok to only persist now (despite this function may have returned earlier).
        // In the worst case, the synchronizer of the consensus client will catch up.
        self._database
            .persist_certificate_and_lock_shared_objects(certificate, last_consensus_index)
    }

    pub async fn handle_transaction_info_request(
        &self,
        request: TransactionInfoRequest,
    ) -> Result<TransactionInfoResponse, SuiError> {
        self.make_transaction_info(&request.transaction_digest)
            .await
    }

    pub async fn handle_account_info_request(
        &self,
        request: AccountInfoRequest,
    ) -> Result<AccountInfoResponse, SuiError> {
        self.make_account_info(request.account)
    }

    pub async fn handle_object_info_request(
        &self,
        request: ObjectInfoRequest,
    ) -> Result<ObjectInfoResponse, SuiError> {
        let ref_and_digest = match request.request_kind {
            ObjectInfoRequestKind::PastObjectInfo(seq) => {
                // Get the Transaction Digest that created the object
                self.get_parent_iterator(request.object_id, Some(seq))
                    .await?
                    .next()
            }
            ObjectInfoRequestKind::LatestObjectInfo(_) => {
                // Or get the latest object_reference and transaction entry.
                self.get_latest_parent_entry(request.object_id).await?
            }
        };

        let (requested_object_reference, parent_certificate) = match ref_and_digest {
            Some((object_ref, transaction_digest)) => (
                Some(object_ref),
                if transaction_digest == TransactionDigest::genesis() {
                    None
                } else {
                    // Get the cert from the transaction digest
                    Some(self.read_certificate(&transaction_digest).await?.ok_or(
                        SuiError::CertificateNotfound {
                            certificate_digest: transaction_digest,
                        },
                    )?)
                },
            ),
            None => (None, None),
        };

        // Return the latest version of the object and the current lock if any, if requested.
        let object_and_lock = match request.request_kind {
            ObjectInfoRequestKind::LatestObjectInfo(request_layout) => {
                match self.get_object(&request.object_id).await {
                    Ok(Some(object)) => {
                        let lock = if object.is_immutable() {
                            // Read only objects have no locks.
                            None
                        } else {
                            self.get_transaction_lock(&object.compute_object_reference())
                                .await?
                        };
                        let layout = match request_layout {
                            Some(format) => {
                                let resolver = ModuleCache::new(&self);
                                object.get_layout(format, &resolver)?
                            }
                            None => None,
                        };

                        Some(ObjectResponse {
                            object,
                            lock,
                            layout,
                        })
                    }
                    Err(e) => return Err(e),
                    _ => None,
                }
            }
            ObjectInfoRequestKind::PastObjectInfo(_) => None,
        };

        Ok(ObjectInfoResponse {
            parent_certificate,
            requested_object_reference,
            object_and_lock,
        })
    }

    /// Handles a request for a batch info. It returns a sequence of
    /// [batches, transactions, batches, transactions] as UpdateItems, and a flag
    /// that if true indicates the request goes beyond the last batch in the
    /// database.
    pub async fn handle_batch_info_request(
        &self,
        request: BatchInfoRequest,
    ) -> Result<
        (
            VecDeque<UpdateItem>,
            // Should subscribe, computer start, computed end
            (bool, TxSequenceNumber, TxSequenceNumber),
        ),
        SuiError,
    > {
        // Ensure the range contains some elements and end > start
        if request.length == 0 {
            return Err(SuiError::InvalidSequenceRangeError);
        };

        // Ensure we are not doing too much work per request
        if request.length > MAX_ITEMS_LIMIT {
            return Err(SuiError::TooManyItemsError(MAX_ITEMS_LIMIT));
        }

        // If we do not have a start, pick the low watermark from the notifier.
        let start = match request.start {
            Some(start) => start,
            None => {
                self.last_batch()?
                    .expect("Authority is always initialized with a batch")
                    .batch
                    .next_sequence_number
            }
        };
        let end = start + request.length;

        let (batches, transactions) = self._database.batches_and_transactions(start, end)?;

        let mut dq_batches = std::collections::VecDeque::from(batches);
        let mut dq_transactions = std::collections::VecDeque::from(transactions);
        let mut items = VecDeque::with_capacity(dq_batches.len() + dq_transactions.len());
        let mut last_batch_next_seq = 0;

        // Send full historical data as [Batch - Transactions - Batch - Transactions - Batch].
        while let Some(current_batch) = dq_batches.pop_front() {
            // Get all transactions belonging to this batch and send them
            loop {
                // No more items or item too large for this batch
                if dq_transactions.is_empty()
                    || dq_transactions[0].0 >= current_batch.batch.next_sequence_number
                {
                    break;
                }

                let current_transaction = dq_transactions.pop_front().unwrap();
                items.push_back(UpdateItem::Transaction(current_transaction));
            }

            // Now send the batch
            last_batch_next_seq = current_batch.batch.next_sequence_number;
            items.push_back(UpdateItem::Batch(current_batch));
        }

        // whether we have sent everything requested, or need to start
        // live notifications.
        let should_subscribe = end > last_batch_next_seq;

        // If any transactions are left they must be outside a batch
        while let Some(current_transaction) = dq_transactions.pop_front() {
            // Remember the last sequence sent
            items.push_back(UpdateItem::Transaction(current_transaction));
        }

        Ok((items, (should_subscribe, start, end)))
    }

    pub async fn new(
        committee: Committee,
        name: AuthorityName,
        secret: StableSyncAuthoritySigner,
        store: Arc<AuthorityStore>,
        genesis_packages: Vec<Vec<CompiledModule>>,
        genesis_ctx: &mut TxContext,
    ) -> Self {
        let state =
            AuthorityState::new_without_genesis(committee, name, secret, store.clone()).await;

        // Only initialize an empty database.
        if store
            .database_is_empty()
            .expect("Database read should not fail.")
        {
            for genesis_modules in genesis_packages {
                state
                    .store_package_and_init_modules_for_genesis(genesis_ctx, genesis_modules)
                    .await
                    .expect("We expect publishing the Genesis packages to not fail");
            }
        }

        state
    }

    pub async fn new_without_genesis(
        committee: Committee,
        name: AuthorityName,
        secret: StableSyncAuthoritySigner,
        store: Arc<AuthorityStore>,
    ) -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(BROADCAST_CAPACITY);
        let native_functions =
            sui_framework::natives::all_natives(MOVE_STDLIB_ADDRESS, SUI_FRAMEWORK_ADDRESS);

        let mut state = AuthorityState {
            committee,
            name,
            secret,
            _native_functions: native_functions.clone(),
            move_vm: Arc::new(
                adapter::new_move_vm(native_functions)
                    .expect("We defined natives to not fail here"),
            ),
            _database: store.clone(),
            batch_channels: tx,
            batch_notifier: Arc::new(
                authority_notifier::TransactionNotifier::new(store)
                    .expect("Notifier cannot start."),
            ),
            consensus_guardrail: AtomicUsize::new(0),
            metrics: &METRICS,
        };

        state
            .init_batches_from_database()
            .expect("Init batches failed!");

        state
    }

    pub(crate) fn db(&self) -> Arc<AuthorityStore> {
        self._database.clone()
    }

    async fn get_object(&self, object_id: &ObjectID) -> Result<Option<Object>, SuiError> {
        self._database.get_object(object_id)
    }

    pub async fn insert_genesis_object(&self, object: Object) {
        self._database
            .insert_genesis_object(object)
            .expect("TODO: propagate the error")
    }

    pub async fn insert_genesis_objects_bulk_unsafe(&self, objects: &[&Object]) {
        self._database
            .bulk_object_insert(objects)
            .expect("TODO: propagate the error")
    }

    /// Persist the Genesis package to DB along with the side effects for module initialization
    async fn store_package_and_init_modules_for_genesis(
        &self,
        ctx: &mut TxContext,
        modules: Vec<CompiledModule>,
    ) -> SuiResult {
        let inputs = Transaction::input_objects_in_compiled_modules(&modules);
        let ids: Vec<_> = inputs.iter().map(|kind| kind.object_id()).collect();
        let input_objects = self.get_objects(&ids[..]).await?;
        // When publishing genesis packages, since the std framework packages all have
        // non-zero addresses, [`Transaction::input_objects_in_compiled_modules`] will consider
        // them as dependencies even though they are not. Hence input_objects contain objects
        // that don't exist on-chain because they are yet to be published.
        #[cfg(debug_assertions)]
        {
            let to_be_published_addresses: HashSet<_> = modules
                .iter()
                .map(|module| *module.self_id().address())
                .collect();
            assert!(
                // An object either exists on-chain, or is one of the packages to be published.
                inputs
                    .iter()
                    .zip(input_objects.iter())
                    .all(|(kind, obj_opt)| obj_opt.is_some()
                        || to_be_published_addresses.contains(&kind.object_id()))
            );
        }
        let filtered = inputs
            .into_iter()
            .zip(input_objects.into_iter())
            .filter_map(|(input, object_opt)| object_opt.map(|object| (input, object)))
            .collect::<Vec<_>>();

        debug_assert!(ctx.digest() == TransactionDigest::genesis());
        let mut temporary_store =
            AuthorityTemporaryStore::new(self._database.clone(), filtered, ctx.digest());
        let package_id = ObjectID::from(*modules[0].self_id().address());
        let natives = self._native_functions.clone();
        let mut gas_status = SuiGasStatus::new_unmetered();
        let vm = adapter::verify_and_link(
            &temporary_store,
            &modules,
            package_id,
            natives,
            &mut gas_status,
        )?;
        adapter::store_package_and_init_modules(
            &mut temporary_store,
            &vm,
            modules,
            ctx,
            &mut gas_status,
        )?;
        self.db()
            .update_objects_state_for_genesis(temporary_store, ctx.digest())
    }

    /// Make an information response for a transaction
    pub(crate) async fn make_transaction_info(
        &self,
        transaction_digest: &TransactionDigest,
    ) -> Result<TransactionInfoResponse, SuiError> {
        self._database
            .get_signed_transaction_info(transaction_digest)
    }

    fn make_account_info(&self, account: SuiAddress) -> Result<AccountInfoResponse, SuiError> {
        self._database
            .get_account_objects(account)
            .map(|object_ids| AccountInfoResponse {
                object_ids,
                owner: account,
            })
    }

    // Helper function to manage transaction_locks

    /// Set the transaction lock to a specific transaction
    #[instrument(name = "db_set_transaction_lock", level = "trace", skip_all)]
    pub async fn set_transaction_lock(
        &self,
        mutable_input_objects: &[ObjectRef],
        signed_transaction: SignedTransaction,
    ) -> Result<(), SuiError> {
        self._database
            .set_transaction_lock(mutable_input_objects, signed_transaction)
    }

    /// Update state and signals that a new transactions has been processed
    /// to the batch maker service.
    #[instrument(name = "db_update_state", level = "debug", skip_all)]
    async fn update_state(
        &self,
        temporary_store: AuthorityTemporaryStore<AuthorityStore>,
        certificate: &CertifiedTransaction,
        signed_effects: &SignedTransactionEffects,
    ) -> SuiResult {
        let notifier_ticket = self.batch_notifier.ticket()?;
        self._database.update_state(
            temporary_store,
            certificate,
            signed_effects,
            Some(notifier_ticket.seq()),
        )
        // implicitly we drop the ticket here and that notifies the batch manager
    }

    /// Get a read reference to an object/seq lock
    pub async fn get_transaction_lock(
        &self,
        object_ref: &ObjectRef,
    ) -> Result<Option<SignedTransaction>, SuiError> {
        self._database.get_transaction_lock(object_ref)
    }

    // Helper functions to manage certificates

    /// Read from the DB of certificates
    pub async fn read_certificate(
        &self,
        digest: &TransactionDigest,
    ) -> Result<Option<CertifiedTransaction>, SuiError> {
        self._database.read_certificate(digest)
    }

    pub async fn parent(&self, object_ref: &ObjectRef) -> Option<TransactionDigest> {
        self._database
            .parent(object_ref)
            .expect("TODO: propagate the error")
    }

    pub async fn get_objects(
        &self,
        _objects: &[ObjectID],
    ) -> Result<Vec<Option<Object>>, SuiError> {
        self._database.get_objects(_objects)
    }

    /// Returns all parents (object_ref and transaction digests) that match an object_id, at
    /// any object version, or optionally at a specific version.
    pub async fn get_parent_iterator(
        &self,
        object_id: ObjectID,
        seq: Option<SequenceNumber>,
    ) -> Result<impl Iterator<Item = (ObjectRef, TransactionDigest)> + '_, SuiError> {
        {
            self._database.get_parent_iterator(object_id, seq)
        }
    }

    pub async fn get_latest_parent_entry(
        &self,
        object_id: ObjectID,
    ) -> Result<Option<(ObjectRef, TransactionDigest)>, SuiError> {
        self._database.get_latest_parent_entry(object_id)
    }
}

impl ModuleResolver for AuthorityState {
    type Error = SuiError;

    fn get_module(&self, module_id: &ModuleId) -> Result<Option<Vec<u8>>, Self::Error> {
        self._database.get_module(module_id)
    }
}

#[async_trait]
impl ExecutionState for AuthorityState {
    type Transaction = ConsensusTransaction;
    type Error = SuiError;

    async fn handle_consensus_transaction(
        &self,
        execution_indices: ExecutionIndices,
        transaction: Self::Transaction,
    ) -> Result<Vec<u8>, Self::Error> {
        let ConsensusTransaction::UserTransaction(certificate) = transaction;

        // Ensure an idempotent answer.
        let digest = certificate.digest();
        if self._database.effects_exists(digest)? {
            let info = self.make_transaction_info(digest).await?;
            debug!("Shared-object transaction {digest:?} already executed");
            return Ok(bincode::serialize(&info).unwrap());
        }

        // Assign locks to shared objects.
        self.handle_consensus_certificate(certificate.clone(), execution_indices)
            .await?;
        debug!("Shared objects locks successfully attributed to transaction {digest:?}");

        // Attempt to execute the transaction. This will only succeed if the authority
        // already executed all its dependencies.
        let confirmation_transaction = ConfirmationTransaction {
            certificate: certificate.clone(),
        };
        let info = self
            .handle_confirmation_transaction(confirmation_transaction.clone())
            .await?;
        debug!("Executed transaction {digest:?}");

        // Return a serialized transaction info response. This will be sent back to the client.
        Ok(bincode::serialize(&info).unwrap())
    }

    fn ask_consensus_write_lock(&self) -> bool {
        self.consensus_guardrail.fetch_add(1, Ordering::SeqCst) == 0
    }

    fn release_consensus_write_lock(&self) {
        self.consensus_guardrail.fetch_sub(0, Ordering::SeqCst);
    }

    async fn load_execution_indices(&self) -> Result<ExecutionIndices, Self::Error> {
        self._database.last_consensus_index()
    }
}
