#![cfg_attr(
    not(test),
    deny(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

use bitvec::prelude as bv;
use core::marker::PhantomData;
use fastmurmur3::murmur3_x64_128;
use std::collections::{HashMap, HashSet};
use std::mem::take;

use bytes::Bytes;

use chrono::{DateTime, Utc};

const BUCKETS: u64 = 1_000_000;
const TARGETING_KEY: &str = "targeting_key";
const NULL: Value = Value { kind: None };

const MAX_NO_OF_FLAGS_TO_BATCH_RESOLVE: usize = 200;

/// Seeds the thread-local random number generator.
///
/// In WASM environments, this should be called early in each thread with
/// entropy from the host (e.g., from JavaScript's `crypto.getRandomValues()`).
/// In std environments, fastrand auto-seeds from OS entropy.
pub fn seed_rng(seed: u64) {
    fastrand::seed(seed);
}

/// Generates a random alphanumeric string of the given length.
fn random_alphanumeric(len: usize) -> String {
    (0..len).map(|_| fastrand::alphanumeric()).collect()
}

use err::Fallible;

pub mod assign_logger;
mod bounded_set;
mod err;
pub mod flag_logger;
mod gzip;
pub mod proto;
pub mod resolve_logger;
mod schema_util;
pub mod telemetry;
mod value;

use proto::confidence::flags::admin::v1 as flags_admin;
use proto::confidence::flags::resolver::v1 as flags_resolver;
use proto::confidence::flags::resolver::v1::resolve_token_v1::AssignedFlag;
use proto::confidence::flags::types::v1 as flags_types;
use proto::confidence::iam::v1 as iam;
use proto::google::{value::Kind, Struct, Timestamp, Value};
use proto::Message;

use flags_admin::flag::rule;
use flags_admin::flag::{Rule, Variant};
use flags_admin::Flag;
use flags_admin::ResolverState as ResolverStatePb;
use flags_admin::Segment;
use flags_types::expression;
use flags_types::targeting;
use flags_types::targeting::criterion;
use flags_types::targeting::Criterion;
use flags_types::Expression;
use gzip::decompress_gz;

use crate::err::{ErrorCode, OrFailExt};
use crate::proto::confidence::flags::admin::v1::flag::rule::assignment::{
    Assignment, VariantAssignment,
};
use crate::proto::confidence::flags::admin::v1::flag::rule::materialization_spec::MaterializationReadMode;
use crate::proto::confidence::flags::resolver::v1::{
    resolve_process_request, resolve_process_response, MaterializationRecord, ResolveFlagsRequest,
    ResolveFlagsResponse, ResolveProcessRequest, ResolveProcessResponse, ResolveReason,
};
use crate::proto::confidence::flags::types::v1::targeting::criterion::MaterializedSegmentCriterion;

impl TryFrom<Vec<u8>> for ResolverStatePb {
    type Error = ErrorCode;
    fn try_from(s: Vec<u8>) -> Fallible<Self> {
        ResolverStatePb::decode(&s[..]).or_fail()
    }
}

fn timestamp_to_datetime(ts: &Timestamp) -> Fallible<DateTime<Utc>> {
    DateTime::from_timestamp(ts.seconds, ts.nanos as u32).or_fail()
}
fn datetime_to_timestamp(dt: &DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

#[derive(Debug)]
pub struct Account {
    pub name: String,
}
impl Account {
    fn new(name: &str) -> Account {
        Account {
            name: name.to_string(),
        }
    }

    fn salt(&self) -> Fallible<String> {
        let id = self.name.split("/").nth(1).or_fail()?;
        Ok(format!("MegaSalt-{}", id))
    }

    fn salt_unit(&self, unit: &str) -> Fallible<String> {
        let salt = self.salt()?;
        Ok(format!("{}|{}", salt, unit))
    }
}

#[derive(Debug)]
pub struct Client {
    pub account: Account,
    pub client_name: String,
    pub client_credential_name: String,
    pub environments: Vec<String>,
}

#[derive(Debug)]
pub struct ResolverState {
    pub secrets: HashMap<String, Client>,
    pub flags: HashMap<String, Flag>,
    pub segments: HashMap<String, Segment>,
    pub bitsets: HashMap<String, bv::BitVec<u8, bv::Lsb0>>,
}
impl ResolverState {
    pub fn from_proto(state_pb: ResolverStatePb, account_id: &str) -> Fallible<Self> {
        let mut secrets = HashMap::new();
        let mut flags = HashMap::new();
        let mut segments = HashMap::new();
        let mut bitsets = HashMap::new();

        for flag in state_pb.flags {
            flags.insert(flag.name.clone(), flag);
        }
        for segment in state_pb.segments_no_bitsets {
            segments.insert(segment.name.clone(), segment);
        }
        for bitset in state_pb.bitsets {
            let Some(b) = bitset.bitset else { continue };
            match b {
                flags_admin::resolver_state::packed_bitset::Bitset::GzippedBitset(zipped_bytes) => {
                    // unzip bytes
                    let buffer = decompress_gz(&zipped_bytes[..])?;
                    let bitvec = bv::BitVec::from_slice(&buffer);
                    bitsets.insert(bitset.segment.clone(), bitvec);
                }
                // missing bitset treated as full
                flags_admin::resolver_state::packed_bitset::Bitset::FullBitset(true) => (),
                _ => fail!(),
            }
        }
        for client in state_pb.clients {
            for credential in &state_pb.client_credentials {
                if !credential.name.starts_with(client.name.as_str()) {
                    continue;
                }
                let Some(iam::client_credential::Credential::ClientSecret(client_secret)) =
                    &credential.credential
                else {
                    continue;
                };

                secrets.insert(
                    client_secret.secret.clone(),
                    Client {
                        account: Account::new(&format!("accounts/{}", account_id)),
                        client_name: client.name.clone(),
                        client_credential_name: credential.name.clone(),
                        environments: credential.environments.clone(),
                    },
                );
            }
        }

        Ok(ResolverState {
            secrets,
            flags,
            segments,
            bitsets,
        })
    }

    #[cfg(feature = "json")]
    pub fn get_resolver_with_json_context<'a, H: Host>(
        &'a self,
        client_secret: &str,
        evaluation_context: &str,
        encryption_key: &Bytes,
    ) -> Result<AccountResolver<'a, H>, String> {
        self.get_resolver(
            client_secret,
            // allow this unwrap cause it only happens in std
            #[allow(clippy::unwrap_used)]
            serde_json::from_str(evaluation_context)
                .map_err(|_| "failed to parse evaluation context".to_string())?,
            encryption_key,
        )
    }

    pub fn get_resolver<'a, H: Host>(
        &'a self,
        client_secret: &str,
        evaluation_context: Struct,
        encryption_key: &Bytes,
    ) -> Result<AccountResolver<'a, H>, String> {
        self.secrets
            .get(client_secret)
            .ok_or("client secret not found".to_string())
            .map(|client| {
                AccountResolver::new(
                    client,
                    self,
                    EvaluationContext {
                        context: evaluation_context,
                    },
                    encryption_key,
                )
            })
    }
}

pub struct EvaluationContext {
    pub context: Struct,
}
pub struct FlagToApply {
    pub assigned_flag: AssignedFlag,
    pub skew_adjusted_applied_time: Timestamp,
}

pub trait Host {
    fn log(_: &str) {
        // noop
    }

    #[cfg(not(feature = "std"))]
    fn current_time() -> Timestamp;
    #[cfg(feature = "std")]
    fn current_time() -> Timestamp {
        let now = chrono::Utc::now();
        Timestamp {
            seconds: now.timestamp(),
            nanos: now.timestamp_subsec_nanos() as i32,
        }
    }

    fn log_resolve(
        resolve_id: &str,
        evaluation_context: &Struct,
        values: &[ResolvedValue<'_>],
        client: &Client,
        sdk: &Option<flags_resolver::Sdk>,
    );

    fn log_assign(
        resolve_id: &str,
        evaluation_context: &Struct,
        assigned_flags: &[FlagToApply],
        client: &Client,
        sdk: &Option<flags_resolver::Sdk>,
    );

    fn encrypt_resolve_token(token_data: &[u8], encryption_key: &[u8]) -> Result<Vec<u8>, String> {
        #[cfg(feature = "std")]
        {
            const ENCRYPTION_WRITE_BUFFER_SIZE: usize = 4096;

            use std::io::Write;

            use crypto::{aes, blockmodes, buffer};

            let mut iv = [0u8; 16];
            for byte in &mut iv {
                *byte = fastrand::u8(..);
            }

            let mut final_encrypted_token = Vec::<u8>::new();
            final_encrypted_token
                .write(&iv)
                .map_err(|_| "Failed to write iv to encrypted resolve token buffer".to_string())?;

            let mut encryptor = aes::cbc_encryptor(
                aes::KeySize::KeySize128,
                &iv,
                encryption_key,
                blockmodes::PkcsPadding,
            );

            let token_read_buffer = &mut buffer::RefReadBuffer::new(token_data);
            let mut write_buffer = [0; ENCRYPTION_WRITE_BUFFER_SIZE];
            let token_write_buffer = &mut buffer::RefWriteBuffer::new(&mut write_buffer);

            loop {
                use crypto::buffer::{BufferResult, ReadBuffer, WriteBuffer};

                let result = encryptor
                    .encrypt(token_read_buffer, token_write_buffer, true)
                    .map_err(|_| "Failed to encrypt resolve token".to_string())?;

                final_encrypted_token.extend(
                    token_write_buffer
                        .take_read_buffer()
                        .take_remaining()
                        .iter()
                        .copied(),
                );

                match result {
                    BufferResult::BufferUnderflow => break,
                    BufferResult::BufferOverflow => {}
                }
            }

            Ok(final_encrypted_token)
        }

        #[cfg(not(feature = "std"))]
        {
            // Null encryption for no_std when key is all zeros
            if encryption_key.iter().all(|&b| b == 0) {
                Ok(token_data.to_vec())
            } else {
                Err("Encryption not available in no_std mode".to_string())
            }
        }
    }

    fn decrypt_resolve_token(
        encrypted_data: &[u8],
        encryption_key: &[u8],
    ) -> Result<Vec<u8>, String> {
        #[cfg(feature = "std")]
        {
            {
                const ENCRYPTION_WRITE_BUFFER_SIZE: usize = 4096;

                use crypto::{aes, blockmodes, buffer};

                let mut iv = [0u8; 16];
                iv.copy_from_slice(encrypted_data.get(0..16).or_fail()?);

                let mut decryptor = aes::cbc_decryptor(
                    aes::KeySize::KeySize128,
                    &iv,
                    encryption_key,
                    blockmodes::PkcsPadding,
                );

                let encrypted_token_read_buffer =
                    &mut buffer::RefReadBuffer::new(encrypted_data.get(16..).or_fail()?);
                let mut write_buffer = [0; ENCRYPTION_WRITE_BUFFER_SIZE];
                let encrypted_token_write_buffer =
                    &mut buffer::RefWriteBuffer::new(&mut write_buffer);

                let mut final_decrypted_token = Vec::<u8>::new();
                loop {
                    use crypto::buffer::{BufferResult, ReadBuffer, WriteBuffer};

                    let result = decryptor
                        .decrypt(
                            encrypted_token_read_buffer,
                            encrypted_token_write_buffer,
                            true,
                        )
                        .or_fail()?;

                    final_decrypted_token.extend(
                        encrypted_token_write_buffer
                            .take_read_buffer()
                            .take_remaining()
                            .iter()
                            .copied(),
                    );

                    match result {
                        BufferResult::BufferUnderflow => break,
                        BufferResult::BufferOverflow => {}
                    }
                }

                Ok(final_decrypted_token)
            }
            .map_err(|e: ErrorCode| format!("failed to decrypt resolve token [{}]", e.b64_str()))
        }

        #[cfg(not(feature = "std"))]
        {
            // Null decryption for no_std when key is all zeros
            if encryption_key.iter().all(|&b| b == 0) {
                Ok(encrypted_data.to_vec())
            } else {
                Err("decryption not available in no_std mode".into())
            }
        }
    }
}

pub struct AccountResolver<'a, H: Host> {
    pub client: &'a Client,
    pub state: &'a ResolverState,
    pub evaluation_context: EvaluationContext,
    pub encryption_key: Bytes,
    host: PhantomData<H>,
}

pub use crate::err::ResolveError;

/// Continuation state — serialized as opaque bytes for round-tripping between
/// Suspended/Resume cycles. Public so the wasm guest can decode it to extract
/// the resolve_request for resolver setup.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ResolveProcessState {
    #[prost(message, optional, tag = "1")]
    pub resolve_request: Option<flags_resolver::ResolveFlagsRequest>,
    // what we'd like to round trip is ResolvedValue, but that can't be easily serialized so we use AssignedFlag that can be converted to ResolvedValue
    #[prost(message, repeated, tag = "2")]
    pub resolved_flags: Vec<AssignedFlag>,
    #[prost(message, repeated, tag = "3")]
    pub materializations_to_write: Vec<MaterializationRecord>,
    #[prost(message, optional, tag = "4")]
    pub start_time: Option<Timestamp>,
}

impl ResolveProcessRequest {
    pub fn deferred_materializations(request: ResolveFlagsRequest) -> Self {
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::DeferredMaterializations(
                request,
            )),
        }
    }

    pub fn static_materializations(
        request: ResolveFlagsRequest,
        materializations: Vec<MaterializationRecord>,
    ) -> Self {
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::StaticMaterializations(
                resolve_process_request::StaticMaterializations {
                    resolve_request: Some(request),
                    materializations,
                },
            )),
        }
    }

    pub fn without_materializations(request: ResolveFlagsRequest) -> Self {
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::WithoutMaterializations(
                request,
            )),
        }
    }

    pub fn resume(materializations: Vec<MaterializationRecord>, state: Vec<u8>) -> Self {
        ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::Resume(
                resolve_process_request::Resume {
                    materializations,
                    state,
                },
            )),
        }
    }
}
impl ResolveProcessResponse {
    pub fn resolved(
        response: ResolveFlagsResponse,
        to_write: Vec<MaterializationRecord>,
        start_time: Option<Timestamp>,
    ) -> Self {
        ResolveProcessResponse {
            result: Some(resolve_process_response::Result::Resolved(
                resolve_process_response::Resolved {
                    response: Some(response),
                    materializations_to_write: to_write,
                    start_time,
                },
            )),
        }
    }

    pub fn into_resolved(self) -> Option<(ResolveFlagsResponse, Vec<MaterializationRecord>)> {
        match self.result {
            Some(resolve_process_response::Result::Resolved(resolved)) => {
                Some((resolved.response?, resolved.materializations_to_write))
            }
            _ => None,
        }
    }

    pub fn into_suspended(self) -> Option<(Vec<MaterializationRecord>, Vec<u8>)> {
        match self.result {
            Some(resolve_process_response::Result::Suspended(suspended)) => {
                Some((suspended.materializations_to_read, suspended.state))
            }
            _ => None,
        }
    }

    fn suspended(to_read: Vec<MaterializationRecord>, continuation: ResolveProcessState) -> Self {
        let state = continuation.encode_to_vec();
        ResolveProcessResponse {
            result: Some(resolve_process_response::Result::Suspended(
                resolve_process_response::Suspended {
                    materializations_to_read: to_read,
                    state,
                },
            )),
        }
    }
}

/// Three-valued logic for expression evaluation during materialization discovery.
/// Known criteria return True/False; materialized segments with unknown status return Unknown.
/// Kleene logic rules:
/// - AND: False short-circuits; Unknown does not (need to keep evaluating to discover all reads)
/// - OR: True short-circuits; Unknown does not
/// - NOT: Unknown stays Unknown
type Tribool = Option<bool>;

enum MaterializationRecords {
    /// No materializations available — collect required reads for discovery.
    Discovery,
    /// Materializations not supported — accessing them is an error.
    Unsupported,
    /// A complete set of materializations (e.g. from cookie, resume, or remote fetch).
    Complete(Vec<MaterializationRecord>),
}
pub struct MaterializationContext {
    pub to_read: Vec<MaterializationRecord>,
    pub to_write: Vec<MaterializationRecord>,
    records: MaterializationRecords,
}

impl MaterializationContext {
    pub fn discovery() -> Self {
        MaterializationContext {
            to_read: Vec::new(),
            to_write: Vec::new(),
            records: MaterializationRecords::Discovery,
        }
    }
    /// Create a context with a complete set of materializations (e.g. from a cookie or resume).
    /// Missing lookups are treated as genuinely absent.
    pub fn complete(materializations: Vec<MaterializationRecord>) -> Self {
        MaterializationContext {
            to_read: Vec::new(),
            to_write: Vec::new(),
            records: MaterializationRecords::Complete(materializations),
        }
    }

    /// Create a context where materializations are not supported.
    /// Any flag that requires materializations will error.
    pub fn unsupported() -> Self {
        MaterializationContext {
            to_read: Vec::new(),
            to_write: Vec::new(),
            records: MaterializationRecords::Unsupported,
        }
    }

    /// True if we're collecting materialization requirements rather than resolving.
    pub fn has_missing_reads(&self) -> bool {
        !self.to_read.is_empty()
    }

    /// Check if a unit is included in a materialization.
    ///
    /// - Found in `records` → True (included)
    /// - Not found, but was in `requested` or `complete` → False (definitively absent)
    /// - Not found, not in `requested`, not `complete` → Unknown + record for discovery
    pub fn is_unit_in_materialization(
        &mut self,
        materialization: &str,
        unit: &str,
    ) -> Result<Tribool, ResolveError> {
        match &self.records {
            MaterializationRecords::Complete(records) => {
                let found = records
                    .iter()
                    .any(|r| r.unit == unit && r.materialization == materialization);
                Ok(Some(found))
            }
            MaterializationRecords::Discovery => {
                self.record_missing(
                    unit.to_string(),
                    materialization.to_string(),
                    "".to_string(),
                );
                Ok(None)
            }
            MaterializationRecords::Unsupported => Err(ResolveError::MaterializationsUnsupported),
        }
    }

    /// Check if a unit has a materialization record for the given rule.
    ///
    /// - Found in `records` → Some(true)
    /// - `records` is Some but not found → Some(false) (definitively absent)
    /// - `records` is None → None (Unknown) + record for discovery
    pub fn has_rule_materialization(
        &mut self,
        materialization: &str,
        unit: &str,
        rule: &str,
    ) -> Result<Tribool, ResolveError> {
        match &self.records {
            MaterializationRecords::Complete(records) => {
                let found = records.iter().any(|r| {
                    r.unit == unit && r.materialization == materialization && r.rule == rule
                });
                Ok(Some(found))
            }
            MaterializationRecords::Discovery => {
                self.record_missing(
                    unit.to_string(),
                    materialization.to_string(),
                    rule.to_string(),
                );
                Ok(None)
            }
            MaterializationRecords::Unsupported => Err(ResolveError::MaterializationsUnsupported),
        }
    }

    /// Find the assignment that matches the materialized variant for the given
    /// unit/materialization/rule.
    ///
    /// Returns None if no matching record exists or the stored variant is empty.
    /// Call `has_rule_materialization` first to handle the discovery case.
    pub fn select_assignment<'b>(
        &self,
        materialization: &str,
        unit: &str,
        rule: &str,
        assignments: &'b [rule::Assignment],
    ) -> Option<&'b rule::Assignment> {
        let records = match &self.records {
            MaterializationRecords::Complete(records) => records,
            _ => return None,
        };
        let record = records
            .iter()
            .find(|r| r.unit == unit && r.materialization == materialization && r.rule == rule)?;

        if record.variant.is_empty() {
            return None;
        }

        let variant = &record.variant;
        assignments.iter().find(|assignment| {
            matches!(
                &assignment.assignment,
                Some(Assignment::Variant(VariantAssignment { variant: v }))
                    if variant == v
            )
        })
    }

    pub fn add_to_write(
        &mut self,
        unit: String,
        materialization: String,
        rule: String,
        variant: String,
    ) -> Result<(), ResolveError> {
        if matches!(self.records, MaterializationRecords::Unsupported) {
            return Err(ResolveError::MaterializationsUnsupported);
        }
        if self
            .to_write
            .iter()
            .any(|r| r.unit == unit && r.materialization == materialization && r.rule == rule)
        {
            // cannot write multiple variants to the same rule
            fail!();
        }
        self.to_write.push(MaterializationRecord {
            unit,
            materialization,
            rule,
            variant,
        });
        Ok(())
    }

    /// Deduplicated insert into to_read.
    fn record_missing(&mut self, unit: String, materialization: String, rule: String) {
        if !self
            .to_read
            .iter()
            .any(|r| r.unit == unit && r.materialization == materialization && r.rule == rule)
        {
            self.to_read.push(MaterializationRecord {
                unit,
                materialization,
                rule,
                variant: "".to_string(),
            });
        }
    }
}

impl<'a, H: Host> AccountResolver<'a, H> {
    pub fn new(
        client: &'a Client,
        state: &'a ResolverState,
        evaluation_context: EvaluationContext,
        encryption_key: &Bytes,
    ) -> AccountResolver<'a, H> {
        AccountResolver {
            client,
            state,
            evaluation_context,
            encryption_key: encryption_key.clone(),
            host: PhantomData,
        }
    }

    pub fn resolve_flags(
        &self,
        mut request: ResolveProcessRequest,
    ) -> Result<ResolveProcessResponse, String> {
        let timestamp = H::current_time();

        // Extract resolve request, materialization context, and continuation from the oneof
        let (mut state, mut materialization_context) = match request.resolve.take() {
            Some(resolve_process_request::Resolve::DeferredMaterializations(req)) => {
                let state = ResolveProcessState {
                    resolve_request: Some(req),
                    resolved_flags: Vec::new(),
                    materializations_to_write: Vec::new(),
                    start_time: Some(H::current_time()),
                };
                (state, MaterializationContext::discovery())
            }
            Some(resolve_process_request::Resolve::StaticMaterializations(
                with_materializations,
            )) => {
                let req = with_materializations.resolve_request.or_fail()?;
                let state = ResolveProcessState {
                    resolve_request: Some(req),
                    resolved_flags: Vec::new(),
                    materializations_to_write: Vec::new(),
                    start_time: Some(H::current_time()),
                };
                (
                    state,
                    MaterializationContext::complete(with_materializations.materializations),
                )
            }
            Some(resolve_process_request::Resolve::WithoutMaterializations(req)) => {
                let state = ResolveProcessState {
                    resolve_request: Some(req),
                    resolved_flags: Vec::new(),
                    materializations_to_write: Vec::new(),
                    start_time: Some(H::current_time()),
                };
                (state, MaterializationContext::unsupported())
            }
            Some(resolve_process_request::Resolve::Resume(req)) => {
                let mut state = ResolveProcessState::decode(req.state.as_slice())
                    .map_err(|e| format!("Failed to decode continuation state: {}", e))?;
                let mut materialization_context =
                    MaterializationContext::complete(req.materializations);
                materialization_context
                    .to_write
                    .extend(take(&mut state.materializations_to_write));
                (state, materialization_context)
            }
            None => fail!(),
        };

        let resolve_request = state.resolve_request.as_ref().or_fail()?;
        let flag_names = resolve_request.flags.clone();
        let flags_to_resolve = self
            .state
            .flags
            .values()
            .filter(|flag| flag.state() == flags_admin::flag::State::Active)
            .filter(|flag| flag.clients.contains(&self.client.client_name))
            .filter(|flag| flag_names.is_empty() || flag_names.contains(&flag.name))
            // Skip flags that were already resolved in a prior attempt
            .filter(|flag| !state.resolved_flags.iter().any(|rf| rf.flag == flag.name))
            .collect::<Vec<&Flag>>();

        if flags_to_resolve.len() > MAX_NO_OF_FLAGS_TO_BATCH_RESOLVE {
            return Err(format!(
                "max {} flags allowed in a single resolve request, this request would return {} flags.",
                MAX_NO_OF_FLAGS_TO_BATCH_RESOLVE,
                flags_to_resolve.len()));
        }

        for flag in flags_to_resolve {
            match self.resolve_flag(flag, &mut materialization_context) {
                Ok(resolved_value) => {
                    state.resolved_flags.push((&resolved_value).into());
                }
                Err(err) => match err {
                    ResolveError::MissingMaterializations => continue,
                    other => return Err(other.into()),
                },
            }
        }

        if !materialization_context.to_read.is_empty() {
            state.materializations_to_write = materialization_context.to_write;
            return Ok(ResolveProcessResponse::suspended(
                materialization_context.to_read,
                state,
            ));
        }

        let resolve_id = random_alphanumeric(32);

        let resolved_values: Vec<ResolvedValue<'_>> = state
            .resolved_flags
            .into_iter()
            .map(|a| ResolvedValue::from_assigned(a, self.state))
            .collect::<Fallible<Vec<_>>>()?;

        let mut response = flags_resolver::ResolveFlagsResponse {
            resolve_id: resolve_id.clone(),
            resolved_flags: resolved_values
                .iter()
                .map(|rv| rv.try_into())
                .collect::<Fallible<Vec<_>>>()?,
            ..Default::default()
        };

        let flags_to_assign: Vec<AssignedFlag> = resolved_values
            .iter()
            .filter(|rv| rv.should_apply())
            .map(|rv| rv.into())
            .collect();

        if resolve_request.apply {
            let flags_to_apply: Vec<FlagToApply> = flags_to_assign
                .iter()
                .map(|af| FlagToApply {
                    assigned_flag: af.clone(),
                    skew_adjusted_applied_time: timestamp.clone(),
                })
                .collect();

            H::log_assign(
                &resolve_id,
                &self.evaluation_context.context,
                flags_to_apply.as_slice(),
                self.client,
                &resolve_request.sdk.clone(),
            );
        } else {
            let mut resolve_token_v1 = flags_resolver::ResolveTokenV1 {
                resolve_id: resolve_id.clone(),
                evaluation_context: Some(self.evaluation_context.context.clone()),
                ..Default::default()
            };
            for assigned_flag in &flags_to_assign {
                resolve_token_v1
                    .assignments
                    .insert(assigned_flag.flag.clone(), assigned_flag.clone());
            }

            let resolve_token = flags_resolver::ResolveToken {
                resolve_token: Some(flags_resolver::resolve_token::ResolveToken::TokenV1(
                    resolve_token_v1,
                )),
            };

            let encrypted_token = self
                .encrypt_resolve_token(&resolve_token)
                .map_err(|_| "Failed to encrypt resolve token".to_string())?;

            response.resolve_token = encrypted_token;
        }

        H::log_resolve(
            &resolve_id,
            &self.evaluation_context.context,
            &resolved_values,
            self.client,
            &resolve_request.sdk.clone(),
        );

        Ok(ResolveProcessResponse::resolved(
            response,
            materialization_context.to_write,
            state.start_time.take(),
        ))
    }

    #[cfg(test)]
    pub(crate) fn resolve_flags_no_materialization(
        &self,
        request: &flags_resolver::ResolveFlagsRequest,
    ) -> Result<flags_resolver::ResolveFlagsResponse, String> {
        let response = self.resolve_flags(ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::WithoutMaterializations(
                request.clone(),
            )),
        });
        match response {
            Ok(ResolveProcessResponse {
                result: Some(resolve_process_response::Result::Resolved(resolved)),
            }) => resolved
                .response
                .ok_or_else(|| "empty resolve response".to_string()),
            Ok(_) => Err("unexpected suspended response".to_string()),
            Err(e) => Err(e),
        }
    }

    pub fn apply_flags(&self, request: &flags_resolver::ApplyFlagsRequest) -> Result<(), String> {
        let send_time_ts = request.send_time.as_ref().ok_or("send_time is required")?;
        let send_time = to_date_time_utc(send_time_ts).ok_or("invalid send_time")?;
        let receive_time: DateTime<Utc> = timestamp_to_datetime(&H::current_time())?;

        let resolve_token_outer = self.decrypt_resolve_token(&request.resolve_token)?;
        let Some(flags_resolver::resolve_token::ResolveToken::TokenV1(resolve_token)) =
            resolve_token_outer.resolve_token
        else {
            return Err("resolve token is not a V1 token".to_string());
        };

        let assignments = resolve_token.assignments;
        let evaluation_context = resolve_token
            .evaluation_context
            .as_ref()
            .ok_or("missing evaluation context")?;

        // ensure that all flags are present before we start sending events
        let mut assigned_flags: Vec<FlagToApply> = Vec::with_capacity(request.flags.len());
        for applied_flag in &request.flags {
            let Some(assigned_flag) = assignments.get(&applied_flag.flag) else {
                return Err("Flag in resolve token does not match flag in request".to_string());
            };
            let Some(apply_time) = applied_flag.apply_time.as_ref() else {
                return Err(format!("Missing apply time for flag {}", applied_flag.flag));
            };
            let apply_time = to_date_time_utc(apply_time).or_fail()?;
            let skew = send_time.signed_duration_since(apply_time);
            let adjusted_time = receive_time.checked_sub_signed(skew).or_fail()?;
            let skew_adjusted_applied_time = datetime_to_timestamp(&adjusted_time);
            assigned_flags.push(FlagToApply {
                assigned_flag: assigned_flag.clone(),
                skew_adjusted_applied_time,
            });
        }

        H::log_assign(
            &resolve_token.resolve_id,
            evaluation_context,
            assigned_flags.as_slice(),
            self.client,
            &request.sdk,
        );

        Ok(())
    }

    fn get_targeting_key(&self, targeting_key: &str) -> Result<Option<String>, String> {
        let unit_value = self.get_attribute_value(targeting_key);
        let unit = match &unit_value.kind {
            None => return Ok(None),
            Some(Kind::NullValue(_)) => return Ok(None),
            Some(Kind::StringValue(string_unit)) => string_unit.clone(),
            Some(Kind::NumberValue(num_value)) => {
                if num_value.is_finite() && num_value.fract() == 0.0 {
                    format!("{:.0}", num_value)
                } else {
                    return Err("TargetingKeyError".to_string());
                }
            }
            _ => return Err("TargetingKeyError".to_string()),
        };
        if unit.len() > 100 {
            return Err("Targeting key is too long, max 100 characters.".to_string());
        }
        Ok(Some(unit))
    }

    fn enabled_for_environment(&self, rule: &flags_admin::flag::Rule) -> bool {
        rule.environments.is_empty()
            || rule
                .environments
                .iter()
                .any(|env| self.client.environments.contains(env))
    }

    pub fn resolve_flag(
        &'a self,
        flag: &'a Flag,
        materialization_context: &mut MaterializationContext,
    ) -> Result<ResolvedValue<'a>, ResolveError> {
        match self.resolve_flag_internal(flag, materialization_context) {
            Err(ResolveError::MaterializationsUnsupported) => {
                Ok(ResolvedValue::new(flag).error(ResolveReason::MaterializationNotSupported))
            }
            Err(ResolveError::UnrecognizedRule) => {
                Ok(ResolvedValue::new(flag).error(ResolveReason::UnrecognizedTargetingRule))
            }
            other => other,
        }
    }

    pub fn resolve_flag_internal(
        &'a self,
        flag: &'a Flag,
        materialization_context: &mut MaterializationContext,
    ) -> Result<ResolvedValue<'a>, ResolveError> {
        let mut resolved_value = ResolvedValue::new(flag);

        if flag.state == flags_admin::flag::State::Archived as i32 {
            return Ok(resolved_value.error(ResolveReason::FlagArchived));
        }

        let mut has_missing_materializations = false;

        for rule in &flag.rules {
            if !rule.enabled {
                continue;
            }

            if !self.enabled_for_environment(rule) {
                continue;
            }

            let segment_name = &rule.segment;
            if !self.state.segments.contains_key(segment_name) {
                // log something? ResolveReason::SEGMENT_NOT_FOUND
                continue;
            }
            let segment = self.state.segments.get(segment_name).or_fail()?;

            let targeting_key = if !rule.targeting_key_selector.is_empty() {
                rule.targeting_key_selector.as_str()
            } else {
                TARGETING_KEY
            };
            let unit: String = match self.get_targeting_key(targeting_key) {
                Ok(Some(u)) => u,
                Ok(None) => continue,
                Err(_) => return Ok(resolved_value.error(ResolveReason::TargetingKeyError)),
            };

            let Some(spec) = &rule.assignment_spec else {
                continue;
            };

            let mut materialization_matched = Some(false);
            if let Some(materialization_spec) = &rule.materialization_spec {
                let read_materialization = &materialization_spec.read_materialization;
                if !read_materialization.is_empty() {
                    let read_mode =
                        materialization_spec
                            .mode
                            .as_ref()
                            .unwrap_or(&MaterializationReadMode {
                                segment_targeting_can_be_ignored: false,
                                materialization_must_match: false,
                            });
                    match materialization_context.has_rule_materialization(
                        read_materialization,
                        &unit,
                        &rule.name,
                    )? {
                        Some(false) => {
                            materialization_matched = Some(false);
                            if read_mode.materialization_must_match {
                                continue;
                            }
                        }
                        Some(true) => {
                            if read_mode.segment_targeting_can_be_ignored {
                                materialization_matched = Some(true);
                            } else {
                                materialization_matched = match self.targeting_match(
                                    segment,
                                    &unit,
                                    &mut HashSet::new(),
                                    materialization_context,
                                ) {
                                    Ok(matched) => matched,
                                    Err(ResolveError::UnrecognizedRule) => {
                                        continue;
                                    }
                                    Err(e) => return Err(e),
                                };
                            }
                            if materialization_matched == Some(true) {
                                if let Some(assignment) = materialization_context.select_assignment(
                                    read_materialization,
                                    &unit,
                                    &rule.name,
                                    &spec.assignments,
                                ) {
                                    return resolved_value
                                        .try_with_variant_match(rule, segment, assignment, &unit);
                                }
                            }
                        }
                        None => {
                            has_missing_materializations = true;
                        }
                    }
                }
            }

            if materialization_matched != Some(true) {
                materialization_matched =
                    match self.segment_match(segment, &unit, materialization_context) {
                        Ok(matched) => matched,
                        Err(ResolveError::UnrecognizedRule) => {
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
            }
            match materialization_matched {
                Some(true) => {
                    // Segment matches, continue with assignment
                    if has_missing_materializations {
                        continue;
                    }
                }
                Some(false) => {
                    // Segment doesn't match  — try next rule
                    // (Unknown means discoveries were recorded as side effects)
                    continue;
                }
                None => {
                    // Segment match is unknown  — try next rule
                    // (Unknown means discoveries were recorded as side effects)
                    has_missing_materializations = true;
                    continue;
                }
            }

            let bucket_count = spec.bucket_count;
            let variant_salt = segment_name.split("/").nth(1).or_fail()?;
            let key = format!("{}|{}", variant_salt, unit);
            let bucket = bucket(hash(&key), bucket_count as u64)? as i32;

            let matched_assignment = spec.assignments.iter().find(|assignment| {
                assignment
                    .bucket_ranges
                    .iter()
                    .any(|range| range.lower <= bucket && bucket < range.upper)
            });

            if let Some(rule::Assignment {
                assignment_id,
                assignment: Some(assignment),
                ..
            }) = matched_assignment
            {
                // Extract variant name from assignment if it's a variant assignment
                let variant_name = match assignment {
                    Assignment::Variant(ref variant_assignment) => {
                        variant_assignment.variant.clone()
                    }
                    _ => "".to_string(),
                };

                let write_spec = rule
                    .materialization_spec
                    .as_ref()
                    .map(|materialization_spec| &materialization_spec.write_materialization);

                // write the materialization info if write spec exists
                if let Some(materialization) = write_spec {
                    materialization_context.add_to_write(
                        unit.clone(),
                        materialization.clone(),
                        rule.name.clone(),
                        variant_name.clone(),
                    )?
                }

                match assignment {
                    rule::assignment::Assignment::Fallthrough(_) => {
                        resolved_value.attribute_fallthrough_rule(rule, assignment_id, &unit);
                        continue;
                    }
                    rule::assignment::Assignment::ClientDefault(_) => {
                        return Ok(resolved_value.with_client_default_match(
                            rule,
                            segment,
                            assignment_id,
                            &unit,
                        ));
                    }
                    rule::assignment::Assignment::Variant(_) => {
                        let variant = flag
                            .variants
                            .iter()
                            .find(|v| v.name == variant_name)
                            .or_fail()?;
                        return Ok(resolved_value.with_variant_match(
                            rule,
                            segment,
                            variant,
                            assignment_id,
                            &unit,
                        ));
                    }
                };
            }
        }

        if has_missing_materializations {
            return Err(ResolveError::MissingMaterializations);
        }

        Ok(resolved_value)
    }

    /// Get an attribute value from the [EvaluationContext] struct, addressed by a path specification.
    /// If the struct is `{user:{name:"roug",id:42}}`, then getting the `"user.name"` field will return
    /// the value `"roug"`.
    pub fn get_attribute_value(&self, field_path: &str) -> &Value {
        let mut path_parts = field_path.split('.').peekable();
        let mut s = &self.evaluation_context.context;

        while let Some(field) = path_parts.next() {
            match s.fields.get(field) {
                Some(value) => {
                    if path_parts.peek().is_none() {
                        // we are at the end of the path, return the value
                        return value;
                    } else if let Some(Kind::StructValue(struct_value)) = &value.kind {
                        // if we are not at the end of the path, and the value is a struct, continue
                        s = struct_value;
                    } else {
                        // if we are not at the end of the path, but the value is not a struct, return null
                        return &NULL;
                    }
                }
                None => {
                    // non-struct value addressed with .-operator
                    return &NULL;
                }
            }
        }

        &NULL
    }

    #[cfg(test)]
    pub(crate) fn segment_match_no_materialization(
        &self,
        segment: &Segment,
        unit: &str,
    ) -> Result<bool, ResolveError> {
        let result = self.segment_match_internal(
            segment,
            unit,
            &mut HashSet::new(),
            &mut MaterializationContext::unsupported(),
        )?;
        Ok(result == Some(true))
    }

    pub fn segment_match(
        &self,
        segment: &Segment,
        unit: &str,
        materialization_context: &mut MaterializationContext,
    ) -> Result<Tribool, ResolveError> {
        self.segment_match_internal(segment, unit, &mut HashSet::new(), materialization_context)
    }

    fn segment_match_internal(
        &self,
        segment: &Segment,
        unit: &str,
        visited: &mut HashSet<String>,
        materializations: &mut MaterializationContext,
    ) -> Result<Tribool, ResolveError> {
        if visited.contains(&segment.name) {
            fail!("circular segment dependency found");
        }
        visited.insert(segment.name.clone());

        let targeting_result = self.targeting_match(segment, unit, visited, materializations)?;
        if targeting_result == Some(false) {
            return Ok(Some(false));
        }

        // check bitset
        let Some(bitset) = self.state.bitsets.get(&segment.name) else {
            return Ok(targeting_result); // preserve Unknown if targeting was Unknown
        };
        let salted_unit = self.client.account.salt_unit(unit)?;
        let unit_hash = bucket(hash(&salted_unit), BUCKETS)?;
        if unit_hash >= bitset.len() {
            return Ok(Some(false));
        }
        if bitset[unit_hash] {
            Ok(targeting_result) // preserve Unknown if targeting was Unknown
        } else {
            Ok(Some(false))
        }
    }

    fn targeting_match(
        &self,
        segment: &Segment,
        unit: &str,
        visited: &mut HashSet<String>,
        materialization_context: &mut MaterializationContext,
    ) -> Result<Tribool, ResolveError> {
        let Some(targeting) = &segment.targeting else {
            return Ok(Some(true));
        };
        let mut criterion_evaluator = |id: &String| -> Result<Tribool, ResolveError> {
            let Some(Criterion {
                criterion: Some(criterion),
            }) = targeting.criteria.get(id)
            else {
                return Ok(Some(false));
            };
            match &criterion {
                criterion::Criterion::Attribute(attribute_criterion) => {
                    let expected_value_type = value::expected_value_type(attribute_criterion);
                    let attribute_value =
                        self.get_attribute_value(&attribute_criterion.attribute_name);
                    let converted =
                        value::convert_to_targeting_value(attribute_value, expected_value_type)?;
                    let wrapped = list_wrapper(&converted);

                    Ok(Some(value::evaluate_criterion(
                        attribute_criterion,
                        &wrapped,
                    )?))
                }
                criterion::Criterion::Segment(segment_criterion) => {
                    let Some(ref_segment) = self.state.segments.get(&segment_criterion.segment)
                    else {
                        return Ok(Some(false));
                    };

                    self.segment_match_internal(ref_segment, unit, visited, materialization_context)
                }
                criterion::Criterion::MaterializedSegment(MaterializedSegmentCriterion {
                    materialized_segment,
                }) => {
                    materialization_context.is_unit_in_materialization(materialized_segment, unit)
                }
            }
        };

        let Some(expression) = &targeting.expression else {
            return Ok(Some(true));
        };
        evaluate_expression(expression, &mut criterion_evaluator)
    }

    fn encrypt_resolve_token(
        &self,
        resolve_token: &flags_resolver::ResolveToken,
    ) -> Result<Vec<u8>, String> {
        let mut token_buf = Vec::with_capacity(resolve_token.encoded_len());
        resolve_token.encode(&mut token_buf).or_fail()?;

        H::encrypt_resolve_token(&token_buf, &self.encryption_key)
    }

    fn decrypt_resolve_token(
        &self,
        encrypted_token: &[u8],
    ) -> Result<flags_resolver::ResolveToken, String> {
        let decrypted_data = H::decrypt_resolve_token(encrypted_token, &self.encryption_key)?;

        let t = flags_resolver::ResolveToken::decode(&decrypted_data[..]).or_fail()?;
        Ok(t)
    }
}

fn to_date_time_utc(timestamp: &Timestamp) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(timestamp.seconds, timestamp.nanos as u32)
}

fn evaluate_expression(
    expression: &Expression,
    criterion_evaluator: &mut dyn FnMut(&String) -> Result<Tribool, ResolveError>,
) -> Result<Tribool, ResolveError> {
    let Some(expression) = &expression.expression else {
        return Ok(Some(false));
    };
    match expression {
        expression::Expression::Ref(ref_) => criterion_evaluator(ref_),
        expression::Expression::Not(not) => {
            Ok(evaluate_expression(not, criterion_evaluator)?.map(|b| !b))
        }
        expression::Expression::And(and) => {
            let mut has_unknown = false;
            for op in &and.operands {
                match evaluate_expression(op, criterion_evaluator)? {
                    Some(false) => return Ok(Some(false)), // definitive short-circuit
                    Some(true) => {}                       // keep evaluating
                    None => has_unknown = true,            // keep evaluating
                }
            }
            Ok(if has_unknown { None } else { Some(true) })
        }
        expression::Expression::Or(or) => {
            let mut has_unknown = false;
            for op in &or.operands {
                match evaluate_expression(op, criterion_evaluator)? {
                    Some(true) => return Ok(Some(true)), // definitive short-circuit
                    Some(false) => {}                    // keep evaluating
                    None => has_unknown = true,          // keep evaluating
                }
            }
            Ok(if has_unknown { None } else { Some(false) })
        }
    }
}

fn list_wrapper(value: &targeting::value::Value) -> targeting::ListValue {
    match value {
        targeting::value::Value::ListValue(list_value) => list_value.clone(),
        _ => targeting::ListValue {
            values: vec![targeting::Value {
                value: Some(value.clone()),
            }],
        },
    }
}

/// Resolved flag value — wraps an `AssignedFlag` (serializable) with a reference
/// to the `Flag` (for variant value / schema lookups when converting to `ResolvedFlag`).
///
/// The `AssignedFlag` captures all string-based resolution data. The `&'a Flag`
/// provides access to variant values and flag schema needed for the response.
#[derive(Debug, Clone)]
pub struct ResolvedValue<'a> {
    pub flag: &'a Flag,
    pub(crate) inner: AssignedFlag,
}

impl<'a> ResolvedValue<'a> {
    pub(crate) fn from_assigned(
        assigned: AssignedFlag,
        state: &'a ResolverState,
    ) -> Fallible<Self> {
        let flag = state.flags.get(&assigned.flag).or_fail()?;
        Ok(ResolvedValue {
            flag,
            inner: assigned,
        })
    }

    pub(crate) fn new(flag: &'a Flag) -> Self {
        ResolvedValue {
            flag,
            inner: AssignedFlag {
                flag: flag.name.clone(),
                reason: ResolveReason::NoSegmentMatch as i32,
                ..Default::default()
            },
        }
    }

    fn error(&self, reason: ResolveReason) -> Self {
        let mut inner = self.inner.clone();
        inner.reason = reason as i32;
        inner.variant = String::new();
        inner.rule = String::new();
        inner.segment = String::new();
        inner.assignment_id = String::new();
        inner.targeting_key = String::new();
        ResolvedValue {
            flag: self.flag,
            inner,
        }
    }

    pub(crate) fn attribute_fallthrough_rule(
        &mut self,
        rule: &Rule,
        assignment_id: &str,
        unit: &str,
    ) {
        self.inner
            .fallthrough_assignments
            .push(flags_resolver::events::FallthroughAssignment {
                rule: rule.name.clone(),
                assignment_id: assignment_id.to_string(),
                targeting_key: unit.to_string(),
                targeting_key_selector: rule.targeting_key_selector.clone(),
            });
    }

    pub(crate) fn with_client_default_match(
        &self,
        rule: &Rule,
        segment: &Segment,
        assignment_id: &str,
        unit: &str,
    ) -> Self {
        let mut inner = self.inner.clone();
        inner.reason = ResolveReason::Match as i32;
        inner.rule = rule.name.clone();
        inner.segment = segment.name.clone();
        inner.assignment_id = assignment_id.to_string();
        inner.targeting_key = unit.to_string();
        inner.targeting_key_selector = rule.targeting_key_selector.clone();
        ResolvedValue {
            flag: self.flag,
            inner,
        }
    }

    pub(crate) fn with_variant_match(
        &self,
        rule: &Rule,
        segment: &Segment,
        variant: &Variant,
        assignment_id: &str,
        unit: &str,
    ) -> Self {
        let mut inner = self.inner.clone();
        inner.reason = ResolveReason::Match as i32;
        inner.rule = rule.name.clone();
        inner.segment = segment.name.clone();
        inner.variant = variant.name.clone();
        inner.assignment_id = assignment_id.to_string();
        inner.targeting_key = unit.to_string();
        inner.targeting_key_selector = rule.targeting_key_selector.clone();
        ResolvedValue {
            flag: self.flag,
            inner,
        }
    }

    fn try_with_variant_match(
        &self,
        rule: &Rule,
        segment: &Segment,
        assignment: &rule::Assignment,
        unit: &str,
    ) -> Result<Self, ResolveError> {
        let Some(Assignment::Variant(VariantAssignment {
            variant: variant_name,
        })) = &assignment.assignment
        else {
            fail!();
        };
        let Some(variant) = self.flag.variants.iter().find(|v| &v.name == variant_name) else {
            fail!();
        };
        Ok(self.with_variant_match(rule, segment, variant, &assignment.assignment_id, unit))
    }

    // Accessors for resolve_logger compatibility
    pub fn reason(&self) -> ResolveReason {
        self.inner.reason()
    }

    pub fn should_apply(&self) -> bool {
        self.reason() == ResolveReason::Match || !self.inner.fallthrough_assignments.is_empty()
    }
}

impl<'a> TryFrom<&ResolvedValue<'a>> for flags_resolver::ResolvedFlag {
    type Error = ErrorCode;

    fn try_from(value: &ResolvedValue<'a>) -> Fallible<Self> {
        let variant_name = &value.inner.variant;
        let mut resolved_flag = flags_resolver::ResolvedFlag {
            flag: value.flag.name.clone(),
            reason: value.inner.reason,
            should_apply: value.should_apply(),
            ..Default::default()
        };

        if !value.inner.rule.is_empty() || !value.inner.segment.is_empty() {
            if !variant_name.is_empty() {
                let variant = value
                    .flag
                    .variants
                    .iter()
                    .find(|v| &v.name == variant_name)
                    .or_fail()?;
                resolved_flag.variant = variant.name.clone();
                resolved_flag.value = variant.value.clone();
                resolved_flag.flag_schema = value.flag.schema.clone();
            } else {
                resolved_flag.variant = "".to_string();
                resolved_flag.value = Some(Struct::default());
                resolved_flag.flag_schema =
                    Some(flags_types::flag_schema::StructFlagSchema::default());
            }
        }

        Ok(resolved_flag)
    }
}

impl<'a> From<&ResolvedValue<'a>> for AssignedFlag {
    fn from(value: &ResolvedValue<'a>) -> Self {
        value.inner.clone()
    }
}

pub fn hash(key: &str) -> u128 {
    murmur3_x64_128(key.as_bytes(), 0)
}

#[allow(clippy::arithmetic_side_effects)] // buckets != 0 checked above
pub fn bucket(hash: u128, buckets: u64) -> Fallible<usize> {
    if buckets == 0 {
        fail!(":bucket.zero_buckets");
    }
    // convert u128 to u64 to match what we do in the java resolver
    let hash_long: u64 = hash as u64;

    // don't ask me why
    Ok(((hash_long >> 4) % buckets) as usize)
}

#[cfg(test)]
mod materialization_tests;

#[cfg(test)]
mod resolver_spec_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::confidence::flags::resolver::v1::{ResolveFlagsResponse, Sdk};

    const EXAMPLE_STATE: &[u8] = include_bytes!("../test-payloads/resolver_state.pb");
    const EXAMPLE_STATE_2: &[u8] =
        include_bytes!("../test-payloads/resolver_state_with_custom_targeting_flag.pb");
    const MULTIPLE_STICKY_FLAGS_STATE: &[u8] =
        include_bytes!("../test-payloads/resolver_state_with_multiple_sticky_flags.pb");
    const SECRET: &str = "mkjJruAATQWjeY7foFIWfVAcBWnci2YF";

    const ENCRYPTION_KEY: Bytes = Bytes::from_static(&[0; 16]);

    struct L;

    impl Host for L {
        fn log_resolve(
            _resolve_id: &str,
            _evaluation_context: &Struct,
            _values: &[ResolvedValue<'_>],
            _client: &Client,
            _sdk: &Option<Sdk>,
        ) {
            // In tests, we don't need to print anything
        }

        fn log_assign(
            _resolve_id: &str,
            _evaluation_context: &Struct,
            _assigned_flag: &[FlagToApply],
            _client: &Client,
            _sdk: &Option<Sdk>,
        ) {
            // In tests, we don't need to print anything
        }
    }

    #[test]
    fn test_random_alphanumeric() {
        let rnd = random_alphanumeric(32);
        let re = regex::Regex::new(r"^[a-zA-Z0-9]{32}$").unwrap();
        assert!(re.is_match(&rnd));
    }

    #[test]
    fn test_parse_state_bitsets() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let bitvec = state.bitsets.get("segments/qnbpewfufewyn5rpsylm").unwrap();
        let bitvec2 = state.bitsets.get("segments/h2f3kemn2nqbnc7k5lk2").unwrap();

        assert_eq!(bitvec.count_ones(), 555600);
        assert_eq!(bitvec2.count_ones(), 555600);

        // assert that we read the bytes in LSB order
        let first_bits: Vec<bool> = (0..16).map(|i| bitvec[i]).collect();
        let expected_first_bits = vec![
            false, false, false, true, false, false, true, false, true, true, false, true, true,
            true, true, true,
        ];
        assert_eq!(first_bits, expected_first_bits);
    }

    #[test]
    fn test_parse_state_secrets() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let account_client = state
            .secrets
            .get("mkjJruAATQWjeY7foFIWfVAcBWnci2YF")
            .unwrap();
        assert_eq!(account_client.client_name, "clients/cqzy4juldrvnz0z1uedj");
        assert_eq!(
            account_client.client_credential_name,
            "clients/cqzy4juldrvnz0z1uedj/clientCredentials/yejholwrnjfewftakun8"
        );
    }

    #[test]
    fn test_hash() {
        let account = Account {
            name: "accounts/confidence-test".to_string(),
        };
        let bucket = bucket(hash(&account.salt_unit("roug").unwrap()), BUCKETS).unwrap();
        assert_eq!(bucket, 567493); // test matching bucketing result from the java randomizer
    }

    #[test]
    fn test_bucket_zero() {
        let account = Account {
            name: "accounts/confidence-test".to_string(),
        };
        let result = bucket(hash(&account.salt_unit("roug").unwrap()), 0);
        assert!(result.is_err()); // bucket count of 0 should return error
    }

    #[test]
    fn test_account_salt() {
        let account = Account {
            name: "accounts/test".to_string(),
        };

        assert_eq!(account.salt(), Ok("MegaSalt-test".into()));
    }

    #[test]
    fn test_resolve_flag() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        {
            let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
            let resolved_value = resolver
                .resolve_flag(flag, &mut MaterializationContext::discovery())
                .unwrap();

            assert_eq!(
                resolved_value.inner.rule,
                "flags/tutorial-feature/rules/tutorial-visitor-override"
            );
            assert_eq!(
                resolved_value.inner.variant,
                "flags/tutorial-feature/variants/exciting-welcome"
            );
            assert!(resolved_value.should_apply());
        }

        {
            let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
            let resolved = resolver
                .resolve_flag(flag, &mut MaterializationContext::discovery())
                .unwrap();

            assert_eq!(
                resolved.inner.rule,
                "flags/tutorial-feature/rules/tutorial-visitor-override"
            );
            assert_eq!(
                resolved.inner.variant,
                "flags/tutorial-feature/variants/exciting-welcome"
            );
        }
    }
    #[test]
    fn test_resolve_flags() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        {
            let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/tutorial-feature".to_string()],
                apply: false,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            assert_eq!(response.resolved_flags.len(), 1);
            let flag = response.resolved_flags.get(0).unwrap();

            let decrypted_token = resolver
                .decrypt_resolve_token(&response.resolve_token)
                .unwrap();
            match decrypted_token.resolve_token {
                Some(flags_resolver::resolve_token::ResolveToken::TokenV1(token)) => {
                    assert_eq!(token.resolve_id, response.resolve_id);
                    assert_eq!(token.assignments.len(), response.resolved_flags.len());

                    let assignment = token.assignments.get("flags/tutorial-feature").unwrap();

                    assert_eq!(assignment.flag, "flags/tutorial-feature");
                    assert_eq!(
                        assignment.assignment_id,
                        "flags/tutorial-feature/variants/exciting-welcome"
                    );
                    assert_eq!(
                        assignment.variant,
                        "flags/tutorial-feature/variants/exciting-welcome"
                    );
                    assert_eq!(
                        assignment.rule,
                        "flags/tutorial-feature/rules/tutorial-visitor-override"
                    );

                    assert_eq!(assignment.flag, flag.flag);
                    assert_eq!(assignment.variant, flag.variant);
                }
                _ => panic!("Unexpected resolve token type"),
            }

            assert!(resolver.state.flags.contains_key("flags/tutorial-feature"));
            assert_eq!(true, flag.should_apply);
        }
    }

    #[test]
    fn test_resolve_flags_fallthrough() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Single rule
        {
            let context_json = r#"{"visitor_id": "57"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/fallthrough-test-1".to_string()],
                apply: false,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            assert_eq!(response.resolved_flags.len(), 1);
            let flag = response.resolved_flags.get(0).unwrap();

            let decrypted_token = resolver
                .decrypt_resolve_token(&response.resolve_token)
                .unwrap();
            match decrypted_token.resolve_token {
                Some(flags_resolver::resolve_token::ResolveToken::TokenV1(token)) => {
                    assert_eq!(token.resolve_id, response.resolve_id);
                    assert_eq!(token.assignments.len(), response.resolved_flags.len());

                    let assignment = token.assignments.get("flags/fallthrough-test-1").unwrap();
                    assert_eq!(assignment.flag, "flags/fallthrough-test-1");
                    assert_eq!(assignment.targeting_key, "");
                    assert_eq!(assignment.targeting_key_selector, "");
                    assert_eq!(assignment.segment, "");
                    assert_eq!(assignment.variant, "");
                    assert_eq!(assignment.rule, "");
                    assert_eq!(ResolveReason::NoSegmentMatch as i32, flag.reason);
                    assert_eq!(assignment.assignment_id, "");

                    let expected_fallthrough = flags_resolver::events::FallthroughAssignment {
                        rule: "flags/fallthrough-test-1/rules/gdbiknjycxvmc6wu7zzz".to_string(),
                        assignment_id: "control".to_string(),
                        targeting_key: "57".to_string(),
                        targeting_key_selector: "visitor_id".to_string(),
                    };

                    assert_eq!(assignment.fallthrough_assignments.len(), 1);
                    assert_eq!(assignment.fallthrough_assignments[0], expected_fallthrough);
                }
                _ => panic!("Unexpected resolve token type"),
            }

            assert_eq!(true, flag.should_apply);
        }

        // Fallthrough to second rule
        {
            let context_json = r#"{"visitor_id": "26"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/fallthrough-test-2".to_string()],
                apply: false,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            assert_eq!(response.resolved_flags.len(), 1);
            let flag = response.resolved_flags.get(0).unwrap();

            let decrypted_token = resolver
                .decrypt_resolve_token(&response.resolve_token)
                .unwrap();
            match decrypted_token.resolve_token {
                Some(flags_resolver::resolve_token::ResolveToken::TokenV1(token)) => {
                    assert_eq!(token.resolve_id, response.resolve_id);
                    assert_eq!(token.assignments.len(), response.resolved_flags.len());

                    let assignment = token.assignments.get("flags/fallthrough-test-2").unwrap();
                    assert_eq!(assignment.flag, "flags/fallthrough-test-2");
                    assert_eq!(assignment.targeting_key, "26");
                    assert_eq!(assignment.targeting_key_selector, "visitor_id");
                    assert_eq!(assignment.segment, "segments/dvlllobhnpxcojqn6vfa");
                    assert_eq!(
                        assignment.variant,
                        "flags/fallthrough-test-2/variants/enabled"
                    );
                    assert_eq!(
                        assignment.rule,
                        "flags/fallthrough-test-2/rules/oxl1yqqjj1aqyiuvf9al"
                    );
                    assert_eq!(ResolveReason::Match as i32, flag.reason);
                    assert_eq!(assignment.assignment_id, "");

                    let expected_fallthrough = flags_resolver::events::FallthroughAssignment {
                        rule: "flags/fallthrough-test-2/rules/wwzea3vq89gwtcufe9ou".to_string(),
                        assignment_id: "control".to_string(),
                        targeting_key: "26".to_string(),
                        targeting_key_selector: "visitor_id".to_string(),
                    };

                    assert_eq!(assignment.fallthrough_assignments.len(), 1);
                    assert_eq!(assignment.fallthrough_assignments[0], expected_fallthrough);
                }
                _ => panic!("Unexpected resolve token type"),
            }

            assert_eq!(true, flag.should_apply);
        }
    }

    #[test]
    fn test_resolve_flags_no_match() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        {
            let context_json = r#"{}"#; // NO CONTEXT
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/tutorial-feature".to_string()],
                apply: false,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            assert_eq!(response.resolved_flags.len(), 1);
            assert!(resolver.state.flags.contains_key("flags/tutorial-feature"));

            let flag = response.resolved_flags.get(0).unwrap();
            assert_eq!(false, flag.should_apply);
            assert_eq!(ResolveReason::NoSegmentMatch as i32, flag.reason);
        }
    }

    #[test]
    fn test_resolve_flags_apply_logging() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Custom logger that tracks what gets logged
        struct TestLogger {
            assign_logs: std::sync::Mutex<Vec<String>>,
        }

        impl Host for TestLogger {
            fn log_resolve(
                _resolve_id: &str,
                _evaluation_context: &Struct,
                _values: &[ResolvedValue<'_>],
                _client: &Client,
                _sdk: &Option<Sdk>,
            ) {
                // Do nothing for resolve logs
            }

            fn log_assign(
                resolve_id: &str,
                _evaluation_context: &Struct,
                assigned_flag: &[FlagToApply],
                _client: &Client,
                _sdk: &Option<Sdk>,
            ) {
                let mut logs = TestLogger::get_instance()
                    .assign_logs
                    .try_lock()
                    .expect("mutex is locked or poisoned");
                assigned_flag.iter().for_each(|f| {
                    let log_entry = format!("{}:{}", resolve_id, f.assigned_flag.flag);
                    logs.push(log_entry);
                });
            }
        }

        impl TestLogger {
            fn get_instance() -> &'static TestLogger {
                static INSTANCE: std::sync::OnceLock<TestLogger> = std::sync::OnceLock::new();
                INSTANCE.get_or_init(|| TestLogger {
                    assign_logs: std::sync::Mutex::new(Vec::new()),
                })
            }

            fn clear_logs() {
                if let Ok(mut logs) = TestLogger::get_instance().assign_logs.lock() {
                    logs.clear();
                }
            }

            fn get_logs() -> Vec<String> {
                TestLogger::get_instance()
                    .assign_logs
                    .lock()
                    .unwrap()
                    .clone()
            }
        }

        // Test 1: NO_MATCH case with apply=true should NOT log assignments
        {
            TestLogger::clear_logs();
            let context_json = r#"{}"#; // NO CONTEXT
            let resolver: AccountResolver<'_, TestLogger> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/tutorial-feature".to_string()],
                apply: true,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            let flag = response.resolved_flags.get(0).unwrap();
            assert_eq!(false, flag.should_apply);
            assert_eq!(ResolveReason::NoSegmentMatch as i32, flag.reason);

            // Verify that no assignment was logged
            let logs = TestLogger::get_logs();
            assert_eq!(
                logs.len(),
                0,
                "NO_MATCH flags should not be logged when apply=true"
            );
        }

        // Test 2: MATCH case with apply=true SHOULD log assignments
        {
            TestLogger::clear_logs();
            let context_json = r#"{"visitor_id": "tutorial_visitor"}"#; // This should match
            let resolver: AccountResolver<'_, TestLogger> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: SECRET.to_string(),
                flags: vec!["flags/tutorial-feature".to_string()],
                apply: true,
                sdk: Some(Sdk {
                    sdk: None,
                    version: "0.1.0".to_string(),
                }),
            };

            let response: ResolveFlagsResponse = resolver
                .resolve_flags_no_materialization(&resolve_flag_req)
                .unwrap();
            let flag = response.resolved_flags.get(0).unwrap();
            assert_eq!(true, flag.should_apply);
            assert_eq!(ResolveReason::Match as i32, flag.reason);

            // Verify that assignment was logged
            let logs = TestLogger::get_logs();
            assert_eq!(
                logs.len(),
                1,
                "MATCH flags should be logged when apply=true"
            );
            assert!(
                logs[0].contains("flags/tutorial-feature"),
                "Log should contain the flag name"
            );
        }
    }

    #[test]
    fn test_resolve_with_apply_false_then_apply_flags() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Custom logger that tracks what gets logged, including evaluation context
        struct AssignLogEntry {
            resolve_id: String,
            flag: String,
            evaluation_context: Struct,
        }

        struct TestLogger {
            assign_logs: std::sync::Mutex<Vec<AssignLogEntry>>,
        }

        impl Host for TestLogger {
            fn log_resolve(
                _resolve_id: &str,
                _evaluation_context: &Struct,
                _values: &[ResolvedValue<'_>],
                _client: &Client,
                _sdk: &Option<Sdk>,
            ) {
                // Do nothing for resolve logs
            }

            fn log_assign(
                resolve_id: &str,
                evaluation_context: &Struct,
                assigned_flag: &[FlagToApply],
                _client: &Client,
                _sdk: &Option<Sdk>,
            ) {
                let mut logs = TestLogger::get_instance()
                    .assign_logs
                    .try_lock()
                    .expect("mutex is locked or poisoned");
                assigned_flag.iter().for_each(|f| {
                    logs.push(AssignLogEntry {
                        resolve_id: resolve_id.to_string(),
                        flag: f.assigned_flag.flag.clone(),
                        evaluation_context: evaluation_context.clone(),
                    });
                });
            }
        }

        impl TestLogger {
            fn get_instance() -> &'static TestLogger {
                static INSTANCE: std::sync::OnceLock<TestLogger> = std::sync::OnceLock::new();
                INSTANCE.get_or_init(|| TestLogger {
                    assign_logs: std::sync::Mutex::new(Vec::new()),
                })
            }

            fn clear_logs() {
                if let Ok(mut logs) = TestLogger::get_instance().assign_logs.lock() {
                    logs.clear();
                }
            }

            fn get_logs() -> Vec<AssignLogEntry> {
                TestLogger::get_instance()
                    .assign_logs
                    .lock()
                    .unwrap()
                    .drain(..)
                    .collect()
            }
        }

        // Step 1: Resolve flags with apply=false - no assignment should be logged
        TestLogger::clear_logs();
        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver: AccountResolver<'_, TestLogger> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
            evaluation_context: Some(Struct::default()),
            client_secret: SECRET.to_string(),
            flags: vec!["flags/tutorial-feature".to_string()],
            apply: false,
            sdk: Some(Sdk {
                sdk: None,
                version: "0.1.0".to_string(),
            }),
        };

        let response: ResolveFlagsResponse = resolver
            .resolve_flags_no_materialization(&resolve_flag_req)
            .unwrap();
        let flag = response.resolved_flags.get(0).unwrap();
        assert_eq!(true, flag.should_apply);
        assert_eq!(ResolveReason::Match as i32, flag.reason);

        // Verify that no assignment was logged yet (because apply=false)
        let logs = TestLogger::get_logs();
        assert_eq!(
            logs.len(),
            0,
            "No assignments should be logged when apply=false"
        );

        // Verify we got a resolve token
        assert!(
            !response.resolve_token.is_empty(),
            "resolve_token should be set when apply=false"
        );

        // Step 2: Call apply_flags with the resolve token
        let now = Timestamp {
            seconds: 1704067200, // 2024-01-01 00:00:00 UTC
            nanos: 0,
        };

        let apply_request = flags_resolver::ApplyFlagsRequest {
            flags: vec![flags_resolver::AppliedFlag {
                flag: "flags/tutorial-feature".to_string(),
                apply_time: Some(now.clone()),
            }],
            client_secret: SECRET.to_string(),
            resolve_token: response.resolve_token,
            send_time: Some(now),
            sdk: Some(Sdk {
                sdk: None,
                version: "0.1.0".to_string(),
            }),
        };

        let apply_result = resolver.apply_flags(&apply_request);
        assert!(apply_result.is_ok(), "apply_flags should succeed");

        // Verify that assignment was logged after apply_flags
        let logs = TestLogger::get_logs();
        assert_eq!(
            logs.len(),
            1,
            "Assignment should be logged after apply_flags"
        );

        let log_entry = &logs[0];

        // Verify the flag name
        assert_eq!(
            log_entry.flag, "flags/tutorial-feature",
            "Log should contain the correct flag name"
        );

        // Verify that the resolve_id from the original resolve is used in apply_flags logging
        assert_eq!(
            log_entry.resolve_id, response.resolve_id,
            "Log should contain the resolve_id from the original resolve"
        );

        // Verify that the evaluation context used in apply_flags logging matches the original resolve
        // The context should contain the visitor_id that was used during resolve
        let visitor_id_value = log_entry
            .evaluation_context
            .fields
            .get("visitor_id")
            .expect("evaluation_context should contain visitor_id");
        assert_eq!(
            visitor_id_value.kind,
            Some(Kind::StringValue("tutorial_visitor".to_string())),
            "evaluation_context should contain the original visitor_id from resolve"
        );
    }

    #[test]
    fn test_apply_flags_with_wrong_flag_fails() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // First resolve to get a valid token
        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        let resolve_flag_req = flags_resolver::ResolveFlagsRequest {
            evaluation_context: Some(Struct::default()),
            client_secret: SECRET.to_string(),
            flags: vec!["flags/tutorial-feature".to_string()],
            apply: false,
            sdk: Some(Sdk {
                sdk: None,
                version: "0.1.0".to_string(),
            }),
        };

        let response: ResolveFlagsResponse = resolver
            .resolve_flags_no_materialization(&resolve_flag_req)
            .unwrap();

        // Try to apply a different flag than what was resolved
        let now = Timestamp {
            seconds: 1704067200,
            nanos: 0,
        };

        let apply_request = flags_resolver::ApplyFlagsRequest {
            flags: vec![flags_resolver::AppliedFlag {
                flag: "flags/wrong-flag".to_string(), // This flag wasn't resolved
                apply_time: Some(now.clone()),
            }],
            client_secret: SECRET.to_string(),
            resolve_token: response.resolve_token,
            send_time: Some(now),
            sdk: None,
        };

        let apply_result = resolver.apply_flags(&apply_request);
        assert!(
            apply_result.is_err(),
            "apply_flags should fail when applying a flag that wasn't resolved"
        );
        assert!(
            apply_result
                .unwrap_err()
                .contains("Flag in resolve token does not match"),
            "Error message should indicate flag mismatch"
        );
    }

    #[test]
    fn test_targeting_key_integer_supported() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Using integer for visitor_id should be treated as string and work
        let context_json = r#"{"visitor_id": 26}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        let flag = resolver
            .state
            .flags
            .get("flags/fallthrough-test-2")
            .unwrap();
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::discovery())
            .unwrap();

        assert_eq!(resolved_value.inner.reason, ResolveReason::Match as i32);
        assert_eq!(resolved_value.inner.targeting_key, "26");
    }

    #[test]
    fn test_targeting_key_fractional_rejected() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Fractional number for visitor_id should be rejected
        let context_json = r#"{"visitor_id": 26.5}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        let flag = resolver
            .state
            .flags
            .get("flags/fallthrough-test-2")
            .unwrap();
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::discovery())
            .unwrap();

        assert_eq!(
            resolved_value.inner.reason,
            ResolveReason::TargetingKeyError as i32
        );
        assert!(resolved_value.inner.rule.is_empty());
    }

    #[test]
    fn test_targeting_key_exceeding_100_chars_rejected() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        // Targeting key exceeding 100 characters should be rejected
        let long_key = "a".repeat(101);
        let context_json = format!(r#"{{"visitor_id": "{}"}}"#, long_key);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, &context_json, &ENCRYPTION_KEY)
            .unwrap();

        let flag = resolver
            .state
            .flags
            .get("flags/fallthrough-test-2")
            .unwrap();
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::discovery())
            .unwrap();

        assert_eq!(
            resolved_value.inner.reason,
            ResolveReason::TargetingKeyError as i32
        );
        assert!(resolved_value.inner.rule.is_empty());
    }

    // eq rules

    #[test]
    fn test_segment_match_eq_bool_t() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "eqRule": {
                "value": { "boolValue": true }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": true
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_bool_f() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "eqRule": {
                "value": { "boolValue": true }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": false
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_bool_l() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "eqRule": {
                "value": { "boolValue": true }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": [true, false]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_bool_from_string_l() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "eqRule": {
                "value": { "boolValue": true }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": ["true", "false"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_number_t() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "eqRule": {
                "value": { "numberValue": 42.1 }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": 42.1
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_number_f() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "eqRule": {
                "value": { "numberValue": 42.1 }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": 41.0
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_number_l() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "eqRule": {
                "value": { "numberValue": 42.1 }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": [41.0, 42.1]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_string_t() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "eqRule": {
                "value": { "stringValue": "Bob" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": "Bob"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_string_f() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "eqRule": {
                "value": { "stringValue": "Bob" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": "Alice"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_string_l() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "eqRule": {
                "value": { "stringValue": "Bob" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": ["Alice", "Bob"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_timestamp_t() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "eqRule": {
                "value": { "timestampValue": "2022-11-17T15:16:17.118Z" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": "2022-11-17T15:16:17.118Z"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_timestamp_f() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "eqRule": {
                "value": { "timestampValue": "2022-11-17T15:16:17.118Z" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": "2022-11-17T00:00:00Z"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_timestamp_l() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "eqRule": {
                "value": { "timestampValue": "2022-11-17T15:16:17.118Z" }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": ["2022-11-17T00:00:00Z", "2022-11-17T15:16:17.118Z"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_version_t() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "eqRule": {
                "value": { "versionValue": { "version": "1.4.2" } }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": "1.4.2"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_version_f() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "eqRule": {
                "value": { "versionValue": { "version": "1.4.2" } }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": "1.4.1"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_eq_version_l() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "eqRule": {
                "value": { "versionValue": { "version": "1.4.2" } }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": ["1.4.3", "1.4.2"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    // set rules

    #[test]
    fn test_segment_match_set_bool_t() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "setRule": {
                "values": [{ "boolValue": true }, { "boolValue": false }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": true
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_bool_f() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "setRule": {
                "values": [{ "boolValue": true }, { "boolValue": false }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "not": "the field you are looking for"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_bool_l() {
        let rule_json = r#"{
            "attributeName": "client.mobile",
            "setRule": {
                "values": [{ "boolValue": true }, { "boolValue": false }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "mobile": [true, false]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_number_t() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "setRule": {
                "values": [{ "numberValue": 42.1 }, { "numberValue": 41.0 }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": 41.0
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_number_f() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "setRule": {
                "values": [{ "numberValue": 42.1 }, { "numberValue": 41.0 }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": 40.0
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_number_l() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "setRule": {
                "values": [{ "numberValue": 42.1 }, { "numberValue": 41.0 }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": [40.0, 42.1]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_string_t() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "setRule": {
                "values": [{ "stringValue": "Alice" }, { "stringValue": "Bob" }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": "Bob"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_string_f() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "setRule": {
                "values": [{ "stringValue": "Alice" }, { "stringValue": "Bob" }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": "Joe"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_string_l() {
        let rule_json = r#"{
            "attributeName": "client.name",
            "setRule": {
                "values": [{ "stringValue": "Alice" }, { "stringValue": "Bob" }]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "name": ["Bob", "Joe"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_timestamp_t() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "setRule": {
                "values": [
                    { "timestampValue": "2022-11-17T15:16:17.118Z" },
                    { "timestampValue": "2022-11-17T00:00:00Z" }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": "2022-11-17T15:16:17.118Z"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_timestamp_f() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "setRule": {
                "values": [
                    { "timestampValue": "2022-11-17T15:16:17.118Z" },
                    { "timestampValue": "2022-11-17T00:00:00Z" }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": "2022-11-17T01:00:00Z"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_timestamp_l() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "setRule": {
                "values": [
                    { "timestampValue": "2022-11-17T15:16:17.118Z" },
                    { "timestampValue": "2022-11-17T00:00:00Z" }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "buildDate": ["2022-11-17T00:00:00Z", "2022-11-17T01:00:00Z"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_version_t() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "setRule": {
                "values": [
                    { "versionValue": { "version": "1.4.2" } },
                    { "versionValue": { "version": "1.4.3" } }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": "1.4.2"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_version_f() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "setRule": {
                "values": [
                    { "versionValue": { "version": "1.4.2" } },
                    { "versionValue": { "version": "1.4.3" } }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": "1.4.1"
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(!resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_set_version_l() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "setRule": {
                "values": [
                    { "versionValue": { "version": "1.4.2" } },
                    { "versionValue": { "version": "1.4.3" } }
                ]
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "version": ["1.4.3", "1.4.7"]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    // range rules

    #[test]
    fn test_segment_match_range_number_si_ei() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "rangeRule": {
                "startInclusive": { "numberValue": 42.1 },
                "endInclusive": { "numberValue": 43.0 }
            }
        }"#;

        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(r#"{"client": { "score": 42.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.1 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 42.5 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.0 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.1 }, "user_id": "test"}"#, false);
    }

    #[test]
    fn test_segment_match_range_number_si_ee() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "rangeRule": {
                "startInclusive": { "numberValue": 42.1 },
                "endExclusive": { "numberValue": 43.0 }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(r#"{"client": { "score": 42.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.1 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 42.5 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 43.1 }, "user_id": "test"}"#, false);
    }

    #[test]
    fn test_segment_match_range_number_se_ei() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "rangeRule": {
                "startExclusive": { "numberValue": 42.1 },
                "endInclusive": { "numberValue": 43.0 }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(r#"{"client": { "score": 42.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.1 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.5 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.0 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.1 }, "user_id": "test"}"#, false);
    }

    #[test]
    fn test_segment_match_range_number_se_ee() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "rangeRule": {
                "startExclusive": { "numberValue": 42.1 },
                "endExclusive": { "numberValue": 43.0 }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(r#"{"client": { "score": 42.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.1 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 42.5 }, "user_id": "test"}"#, true);
        assert_case(r#"{"client": { "score": 43.0 }, "user_id": "test"}"#, false);
        assert_case(r#"{"client": { "score": 43.1 }, "user_id": "test"}"#, false);
    }

    #[test]
    fn test_segment_match_range_number_l() {
        let rule_json = r#"{
            "attributeName": "client.score",
            "rangeRule": {
                "startInclusive": { "numberValue": 42.1 },
                "endInclusive": { "numberValue": 43.0 }
            }
        }"#;
        let context_json = r#"{
            "user_id": "test",
            "client": {
                "score": [40.1, 42.5, 44.1]
            }
        }"#;
        let (segment, state) = parse_segment(rule_json);
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();

        assert!(resolver
            .segment_match_no_materialization(&segment, "test")
            .unwrap());
    }

    #[test]
    fn test_segment_match_range_timestamp_si_ei() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "rangeRule": {
                "startInclusive": { "timestampValue": "2022-11-17T15:16:17.118Z" },
                "endInclusive": { "timestampValue": "2022-11-18T00:00:00Z" }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:17.118Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:30.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T00:00:00.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_timestamp_si_ee() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "rangeRule": {
                "startInclusive": { "timestampValue": "2022-11-17T15:16:17.118Z" },
                "endExclusive": { "timestampValue": "2022-11-18T00:00:00Z" }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:17.118Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:30.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T00:00:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_timestamp_se_ei() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "rangeRule": {
                "startExclusive": { "timestampValue": "2022-11-17T15:16:17.118Z" },
                "endInclusive": { "timestampValue": "2022-11-18T00:00:00Z" }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:30.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T00:00:00.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_timestamp_se_ee() {
        let rule_json = r#"{
            "attributeName": "client.buildDate",
            "rangeRule": {
                "startExclusive": { "timestampValue": "2022-11-17T15:16:17.118Z" },
                "endExclusive": { "timestampValue": "2022-11-18T00:00:00Z" }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-17T15:16:30.000Z" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T00:00:00.000Z" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "buildDate": "2022-11-18T15:16:17.118Z" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_version_si_ei() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "rangeRule": {
                "startInclusive": { "versionValue": { "version": "1.4.0" } },
                "endInclusive": { "versionValue": { "version": "1.4.5" } }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "version": "1.3.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.0" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.2" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.5" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.5.1" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_version_si_ee() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "rangeRule": {
                "startInclusive": { "versionValue": { "version": "1.4.0" } },
                "endExclusive": { "versionValue": { "version": "1.4.5" } }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "version": "1.3.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.0" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.2" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.5" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.5.1" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_version_se_ei() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "rangeRule": {
                "startExclusive": { "versionValue": { "version": "1.4.0" } },
                "endInclusive": { "versionValue": { "version": "1.4.5" } }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "version": "1.3.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.2" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.5" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.5.1" }, "user_id": "test"}"#,
            false,
        );
    }

    #[test]
    fn test_segment_match_range_version_se_ee() {
        let rule_json = r#"{
            "attributeName": "client.version",
            "rangeRule": {
                "startExclusive": { "versionValue": { "version": "1.4.0" } },
                "endExclusive": { "versionValue": { "version": "1.4.5" } }
            }
        }"#;
        let assert_case = |context_json: &str, expected: bool| {
            let (segment, state) = parse_segment(rule_json);
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
                .unwrap();
            assert_eq!(
                resolver.segment_match_no_materialization(&segment, "test"),
                Ok(expected)
            );
        };

        assert_case(
            r#"{"client": { "version": "1.3.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.0" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.4.2" }, "user_id": "test"}"#,
            true,
        );
        assert_case(
            r#"{"client": { "version": "1.4.5" }, "user_id": "test"}"#,
            false,
        );
        assert_case(
            r#"{"client": { "version": "1.5.1" }, "user_id": "test"}"#,
            false,
        );
    }

    fn parse_segment(rule_json: &str) -> (Segment, ResolverState) {
        let segment_json = format!(
            r#"{{
            "targeting": {{
                "criteria": {{
                    "c": {{
                        "attribute": {rule}
                    }}
                }},
                "expression": {{
                    "ref": "c"
                }}
            }},
            "allocation": {{
                "proportion": {{
                    "value": "1.0"
                }},
                "exclusivityTags": [],
                "exclusiveTo": []
            }}
        }}"#,
            rule = rule_json
        );
        let segment: Segment = serde_json::from_str(segment_json.as_str()).unwrap();

        let mut segments = HashMap::new();
        segments.insert(segment.name.clone(), segment.clone());

        let mut secrets = HashMap::new();
        secrets.insert(
            SECRET.to_string(),
            Client {
                account: Account::new("accounts/test"),
                client_name: "clients/test".to_string(),
                client_credential_name: "clients/test/clientCredentials/abcdef".to_string(),
                environments: vec![],
            },
        );

        let state = ResolverState {
            secrets,
            flags: HashMap::new(),
            segments,
            bitsets: HashMap::new(),
        };

        (segment, state)
    }

    #[test]
    fn test_segment_match_with_materialization_included() {
        let segment_json = r#"{
            "name": "segments/mat-segment",
            "targeting": {
                "criteria": {
                    "mat-crit": {
                        "materializedSegment": {
                            "materializedSegment": "materializedSegments/test-mat"
                        }
                    }
                },
                "expression": {
                    "ref": "mat-crit"
                }
            },
            "allocation": {
                "proportion": {
                    "value": "1.0"
                },
                "exclusivityTags": [],
                "exclusiveTo": []
            }
        }"#;
        let segment: Segment = serde_json::from_str(segment_json).unwrap();

        let mut segments = HashMap::new();
        segments.insert(segment.name.clone(), segment.clone());

        let mut bitsets = HashMap::new();
        // Create a full bitset for the segment
        bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));

        let state = ResolverState {
            secrets: HashMap::new(),
            flags: HashMap::new(),
            segments,
            bitsets,
        };

        let client = Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        };

        let resolver: AccountResolver<'_, L> = AccountResolver::new(
            &client,
            &state,
            EvaluationContext {
                context: Struct::default(),
            },
            &ENCRYPTION_KEY,
        );

        // Create materialization data showing the unit is included
        let mut mat_ctx = MaterializationContext::complete(vec![MaterializationRecord {
            unit: "test-user".to_string(),
            materialization: "materializedSegments/test-mat".to_string(),
            rule: "".to_string(),
            variant: "".to_string(),
        }]);

        let matches = resolver
            .segment_match(&segment, "test-user", &mut mat_ctx)
            .unwrap();

        assert_eq!(
            matches,
            Some(true),
            "Segment should match when unit is included in materialization"
        );
    }

    #[test]
    fn test_segment_match_with_materialization_not_included() {
        let segment_json = r#"{
            "name": "segments/mat-segment",
            "targeting": {
                "criteria": {
                    "mat-crit": {
                        "materializedSegment": {
                            "materializedSegment": "materializedSegments/test-mat"
                        }
                    }
                },
                "expression": {
                    "ref": "mat-crit"
                }
            },
            "allocation": {
                "proportion": {
                    "value": "1.0"
                },
                "exclusivityTags": [],
                "exclusiveTo": []
            }
        }"#;
        let segment: Segment = serde_json::from_str(segment_json).unwrap();

        let mut segments = HashMap::new();
        segments.insert(segment.name.clone(), segment.clone());

        let mut bitsets = HashMap::new();
        bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));

        let state = ResolverState {
            secrets: HashMap::new(),
            flags: HashMap::new(),
            segments,
            bitsets,
        };

        let client = Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        };

        let resolver: AccountResolver<'_, L> = AccountResolver::new(
            &client,
            &state,
            EvaluationContext {
                context: Struct::default(),
            },
            &ENCRYPTION_KEY,
        );

        // Materializations provided but unit is NOT in them (empty records)
        let mut mat_ctx = MaterializationContext::complete(vec![]);

        let matches = resolver
            .segment_match(&segment, "test-user", &mut mat_ctx)
            .unwrap();

        assert_eq!(
            matches,
            Some(false),
            "Segment should not match when unit is not included in materialization"
        );
    }

    #[test]
    fn test_segment_match_without_materialization_data() {
        let segment_json = r#"{
            "name": "segments/mat-segment",
            "targeting": {
                "criteria": {
                    "mat-crit": {
                        "materializedSegment": {
                            "materializedSegment": "materializedSegments/test-mat"
                        }
                    }
                },
                "expression": {
                    "ref": "mat-crit"
                }
            },
            "allocation": {
                "proportion": {
                    "value": "1.0"
                },
                "exclusivityTags": [],
                "exclusiveTo": []
            }
        }"#;
        let segment: Segment = serde_json::from_str(segment_json).unwrap();

        let mut segments = HashMap::new();
        segments.insert(segment.name.clone(), segment.clone());

        let mut bitsets = HashMap::new();
        bitsets.insert(segment.name.clone(), bv::BitVec::repeat(true, 1_000_000));

        let state = ResolverState {
            secrets: HashMap::new(),
            flags: HashMap::new(),
            segments,
            bitsets,
        };

        let client = Client {
            account: Account::new("accounts/test"),
            client_name: "clients/test".to_string(),
            client_credential_name: "clients/test/credentials/test".to_string(),
            environments: vec![],
        };

        let resolver: AccountResolver<'_, L> = AccountResolver::new(
            &client,
            &state,
            EvaluationContext {
                context: Struct::default(),
            },
            &ENCRYPTION_KEY,
        );

        // No materialization data provided (records is None)
        let mut mat_ctx = MaterializationContext::discovery();

        // When no materializations are available, returns Unknown and records the missing inclusion
        let result = resolver.segment_match(&segment, "test-user", &mut mat_ctx);

        assert_eq!(
            result.unwrap(),
            None,
            "Should return Unknown when no materialization data is available"
        );
        assert!(
            mat_ctx.has_missing_reads(),
            "Should have recorded missing materialization"
        );
        assert_eq!(mat_ctx.to_read.len(), 1);
        assert_eq!(
            mat_ctx.to_read[0].materialization,
            "materializedSegments/test-mat"
        );
    }

    #[test]
    fn test_resolve_custom_target_flag() {
        let state = ResolverState::from_proto(
            EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
            "confidence-test",
        )
        .unwrap();

        let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";

        // Test 1: Without materialization data (simple Resolve) - should suspend
        // requesting the missing materialized segment inclusion
        {
            let context_json = r#"{"user_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(secret, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flags_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: secret.to_string(),
                flags: vec!["flags/custom-targeted-flag".to_string()],
                apply: false,
                sdk: None,
            };

            let process_req = ResolveProcessRequest {
                resolve: Some(resolve_process_request::Resolve::DeferredMaterializations(
                    resolve_flags_req,
                )),
            };

            let result = resolver
                .resolve_flags(process_req)
                .expect("resolve process failed");

            // Should suspend requesting the materialized segment inclusion
            match result.result {
                Some(resolve_process_response::Result::Suspended(suspended)) => {
                    assert_eq!(suspended.materializations_to_read.len(), 1);
                    let record = &suspended.materializations_to_read[0];
                    assert_eq!(record.unit, "tutorial_visitor");
                    assert_eq!(
                        record.materialization,
                        "materializedSegments/nicklas-custom-targeting"
                    );
                    assert!(
                        record.rule.is_empty(),
                        "Inclusion requests should have empty rule"
                    );
                    assert!(
                        !suspended.state.is_empty(),
                        "Expected non-empty continuation state"
                    );
                }
                other => panic!("Expected Suspended, got: {:?}", other),
            }
        }

        // Test 2: With materialization data where unit IS included - should succeed with cake variant
        {
            let context_json = r#"{"user_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(secret, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flags_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: secret.to_string(),
                flags: vec!["flags/custom-targeted-flag".to_string()],
                apply: false,
                sdk: None,
            };

            // Provide materialization data indicating user IS in the materialized segment
            let materializations = vec![MaterializationRecord {
                unit: "tutorial_visitor".to_string(),
                materialization: "materializedSegments/nicklas-custom-targeting".to_string(),
                rule: "".to_string(),
                variant: "".to_string(),
            }];

            let process_req = ResolveProcessRequest {
                resolve: Some(resolve_process_request::Resolve::StaticMaterializations(
                    resolve_process_request::StaticMaterializations {
                        resolve_request: Some(resolve_flags_req),
                        materializations,
                    },
                )),
            };

            let result = resolver
                .resolve_flags(process_req)
                .expect("resolve process failed");

            // Should succeed with the cake variant
            match result.result {
                Some(resolve_process_response::Result::Resolved(resolved)) => {
                    let response = resolved.response.expect("Expected response");
                    assert_eq!(response.resolved_flags.len(), 1);
                    let flag = &response.resolved_flags[0];
                    assert_eq!(flag.flag, "flags/custom-targeted-flag");
                    assert_eq!(
                        flag.variant,
                        "flags/custom-targeted-flag/variants/cake-exclamation"
                    );
                }
                other => panic!("Expected Resolved, got: {:?}", other),
            }
        }

        // Test 3: With empty materialization data (unit NOT included) - should fall through to default
        {
            let context_json = r#"{"user_id": "tutorial_visitor"}"#;
            let resolver: AccountResolver<'_, L> = state
                .get_resolver_with_json_context(secret, context_json, &ENCRYPTION_KEY)
                .unwrap();

            let resolve_flags_req = flags_resolver::ResolveFlagsRequest {
                evaluation_context: Some(Struct::default()),
                client_secret: secret.to_string(),
                flags: vec!["flags/custom-targeted-flag".to_string()],
                apply: false,
                sdk: None,
            };

            // Provide empty materializations — unit is not in the materialized segment
            let process_req = ResolveProcessRequest {
                resolve: Some(resolve_process_request::Resolve::StaticMaterializations(
                    resolve_process_request::StaticMaterializations {
                        resolve_request: Some(resolve_flags_req),
                        materializations: vec![],
                    },
                )),
            };

            let result = resolver
                .resolve_flags(process_req)
                .expect("resolve process failed");

            // Should succeed but fall through to the default variant
            match result.result {
                Some(resolve_process_response::Result::Resolved(resolved)) => {
                    let response = resolved.response.expect("Expected response");
                    assert_eq!(response.resolved_flags.len(), 1);
                    let flag = &response.resolved_flags[0];
                    assert_eq!(flag.flag, "flags/custom-targeted-flag");
                    assert_eq!(flag.variant, "flags/custom-targeted-flag/variants/default");
                }
                other => panic!("Expected Resolved, got: {:?}", other),
            }
        }
    }

    #[test]
    fn test_resolve_flag_early_rule_match_skips_materialization() {
        // This test verifies that if an earlier rule matches without needing materializations,
        // we don't request materializations for later rules.
        let state = ResolverState::from_proto(
            EXAMPLE_STATE_2.to_owned().try_into().unwrap(),
            "confidence-test",
        )
        .unwrap();

        let secret = "Ip7lGcBeGA4Le9MI8md4i5LkUOnLnyFx";
        let flag = state.flags.get("flags/custom-targeted-flag").unwrap();

        let mut modified_flag = flag.clone();
        modified_flag.rules.reverse();

        let context_json = r#"{"user_id": "tutorial_visitor"}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(secret, context_json, &ENCRYPTION_KEY)
            .unwrap();

        let resolve_result =
            resolver.resolve_flag(&modified_flag, &mut MaterializationContext::discovery());

        match resolve_result {
            Ok(resolved_value) => {
                assert_eq!(resolved_value.inner.flag, "flags/custom-targeted-flag");
                assert_eq!(
                    resolved_value.inner.variant,
                    "flags/custom-targeted-flag/variants/default"
                );
                assert_eq!(resolved_value.reason(), ResolveReason::Match);
            }
            Err(err) => panic!("Expected success, got error: {:?}", err),
        }
    }

    #[test]
    fn test_resolve_process_multiple_flags_returns_suspended() {
        // This test verifies that when resolving multiple flags that each have rules
        // requiring materializations, the resolver returns Suspended with
        // materializations_to_read for each flag's materialization requirements.

        let state = ResolverState::from_proto(
            MULTIPLE_STICKY_FLAGS_STATE.to_owned().try_into().unwrap(),
            "test",
        )
        .unwrap();

        let context_json = r#"{"user_id": "test-user-456"}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context("test-secret", context_json, &ENCRYPTION_KEY)
            .unwrap();

        let resolve_flags_req = flags_resolver::ResolveFlagsRequest {
            evaluation_context: Some(Struct::default()),
            client_secret: "test-secret".to_string(),
            flags: vec![
                "flags/sticky-flag-1".to_string(),
                "flags/sticky-flag-2".to_string(),
                "flags/sticky-flag-3".to_string(),
            ],
            apply: false,
            sdk: None,
        };

        let process_req = ResolveProcessRequest {
            resolve: Some(resolve_process_request::Resolve::DeferredMaterializations(
                resolve_flags_req,
            )),
        };

        let result = resolver
            .resolve_flags(process_req)
            .expect("resolve process failed");

        // Should return Suspended with materializations_to_read for all three flags
        match result.result {
            Some(resolve_process_response::Result::Suspended(suspended)) => {
                assert_eq!(
                    suspended.materializations_to_read.len(),
                    3,
                    "Expected 3 materializations to read (one per flag), got {}",
                    suspended.materializations_to_read.len()
                );

                let mut materializations_seen: HashSet<String> = HashSet::new();
                let expected_materializations: HashSet<String> = [
                    "experiment_1".to_string(),
                    "experiment_2".to_string(),
                    "experiment_3".to_string(),
                ]
                .into_iter()
                .collect();

                for record in &suspended.materializations_to_read {
                    assert!(
                        expected_materializations.contains(&record.materialization),
                        "Unexpected materialization '{}'",
                        record.materialization
                    );
                    assert!(
                        !materializations_seen.contains(&record.materialization),
                        "Duplicate materialization '{}'",
                        record.materialization
                    );
                    materializations_seen.insert(record.materialization.clone());
                    assert_eq!(record.unit, "test-user-456");
                }

                for mat in &expected_materializations {
                    assert!(
                        materializations_seen.contains(mat),
                        "Missing expected materialization '{}'",
                        mat
                    );
                }

                // Verify continuation state is present
                assert!(
                    !suspended.state.is_empty(),
                    "Expected non-empty continuation state"
                );
            }
            other => panic!("Expected Suspended, got: {:?}", other),
        }
    }

    /// Helper to create a resolver with a custom Client (to set environments).
    fn make_resolver_with_environments<'a>(
        state: &'a ResolverState,
        client: &'a Client,
        context_json: &str,
    ) -> AccountResolver<'a, L> {
        #[allow(clippy::unwrap_used)]
        let context: Struct = serde_json::from_str(context_json).unwrap();
        AccountResolver::new(
            client,
            state,
            EvaluationContext { context },
            &ENCRYPTION_KEY,
        )
    }

    #[test]
    fn test_environment_filtering_no_environments_on_rule_or_credential() {
        // Rule with no environments + credential with no environments → rule evaluated
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let client = Client {
            account: Account::new("accounts/confidence-demo-june"),
            client_name: "clients/test".into(),
            client_credential_name: "clients/test/clientCredentials/test".into(),
            environments: vec![],
        };

        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver = make_resolver_with_environments(&state, &client, context_json);

        let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::unsupported())
            .unwrap();

        // Rules have no environments set so they should all be evaluated
        assert_eq!(resolved_value.reason(), ResolveReason::Match);
    }

    #[test]
    fn test_environment_filtering_matching_environment() {
        // Rule with environments ["production"] + credential with ["production"] → rule evaluated
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let client = Client {
            account: Account::new("accounts/confidence-demo-june"),
            client_name: "clients/test".into(),
            client_credential_name: "clients/test/clientCredentials/test".into(),
            environments: vec!["environments/production".into()],
        };

        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver = make_resolver_with_environments(&state, &client, context_json);

        let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
        let mut modified_flag = flag.clone();
        for rule in &mut modified_flag.rules {
            rule.environments = vec!["environments/production".into()];
        }

        let resolved_value = resolver
            .resolve_flag(&modified_flag, &mut MaterializationContext::unsupported())
            .unwrap();

        assert_eq!(resolved_value.reason(), ResolveReason::Match);
    }

    #[test]
    fn test_environment_filtering_mismatched_environment() {
        // Rule with environments ["production"] + credential with ["staging"] → rule skipped
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let client = Client {
            account: Account::new("accounts/confidence-demo-june"),
            client_name: "clients/test".into(),
            client_credential_name: "clients/test/clientCredentials/test".into(),
            environments: vec!["environments/staging".into()],
        };

        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver = make_resolver_with_environments(&state, &client, context_json);

        let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
        let mut modified_flag = flag.clone();
        for rule in &mut modified_flag.rules {
            rule.environments = vec!["environments/production".into()];
        }

        let resolved_value = resolver
            .resolve_flag(&modified_flag, &mut MaterializationContext::unsupported())
            .unwrap();

        // All rules should be skipped due to environment mismatch
        assert_eq!(resolved_value.reason(), ResolveReason::NoSegmentMatch);
    }

    #[test]
    fn test_environment_filtering_rule_no_env_credential_has_env() {
        // Rule with no environments + credential with ["production"] → rule evaluated
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let client = Client {
            account: Account::new("accounts/confidence-demo-june"),
            client_name: "clients/test".into(),
            client_credential_name: "clients/test/clientCredentials/test".into(),
            environments: vec!["environments/production".into()],
        };

        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver = make_resolver_with_environments(&state, &client, context_json);

        let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
        // Don't set environments on rules — they should be evaluated for any credential
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::unsupported())
            .unwrap();

        assert_eq!(resolved_value.reason(), ResolveReason::Match);
    }

    #[test]
    fn test_environment_filtering_intersection_match() {
        // Rule with environments ["production", "staging"] + credential with ["staging"] → rule evaluated
        let state = ResolverState::from_proto(
            EXAMPLE_STATE.to_owned().try_into().unwrap(),
            "confidence-demo-june",
        )
        .unwrap();

        let client = Client {
            account: Account::new("accounts/confidence-demo-june"),
            client_name: "clients/test".into(),
            client_credential_name: "clients/test/clientCredentials/test".into(),
            environments: vec!["environments/staging".into()],
        };

        let context_json = r#"{"visitor_id": "tutorial_visitor"}"#;
        let resolver = make_resolver_with_environments(&state, &client, context_json);

        let flag = resolver.state.flags.get("flags/tutorial-feature").unwrap();
        let mut modified_flag = flag.clone();
        for rule in &mut modified_flag.rules {
            rule.environments = vec![
                "environments/production".into(),
                "environments/staging".into(),
            ];
        }

        let resolved_value = resolver
            .resolve_flag(&modified_flag, &mut MaterializationContext::unsupported())
            .unwrap();

        assert_eq!(resolved_value.reason(), ResolveReason::Match);
    }

    #[test]
    fn test_resolve_flag_skips_unrecognized_targeting_rule() {
        let segment_json = r#"{
            "name": "segments/unrecognized-rule",
            "targeting": {
                "criteria": {
                    "c": {
                        "attribute": {
                            "attributeName": "user.email"
                        }
                    }
                },
                "expression": {
                    "ref": "c"
                }
            },
            "allocation": {
                "proportion": { "value": "1.0" },
                "exclusivityTags": [],
                "exclusiveTo": []
            }
        }"#;
        let segment: Segment = serde_json::from_str(segment_json).unwrap();

        let flag_json = r#"{
            "name": "flags/unrecognized-rule-flag",
            "state": "ACTIVE",
            "variants": [
                {
                    "name": "flags/unrecognized-rule-flag/variants/on",
                    "value": { "data": "on" }
                }
            ],
            "clients": ["clients/test"],
            "rules": [
                {
                    "name": "flags/unrecognized-rule-flag/rules/rule1",
                    "segment": "segments/unrecognized-rule",
                    "enabled": true,
                    "assignmentSpec": {
                        "bucketCount": 1,
                        "assignments": [
                            {
                                "assignmentId": "flags/unrecognized-rule-flag/variants/on",
                                "variant": {
                                    "variant": "flags/unrecognized-rule-flag/variants/on"
                                },
                                "bucketRanges": [{ "lower": 0, "upper": 1 }]
                            }
                        ]
                    }
                }
            ]
        }"#;
        let flag: Flag = serde_json::from_str(flag_json).unwrap();

        let mut segments = HashMap::new();
        segments.insert(segment.name.clone(), segment);
        let mut flags = HashMap::new();
        flags.insert(flag.name.clone(), flag);

        let mut secrets = HashMap::new();
        secrets.insert(
            SECRET.to_string(),
            Client {
                account: Account::new("accounts/test"),
                client_name: "clients/test".to_string(),
                client_credential_name: "clients/test/clientCredentials/abcdef".to_string(),
                environments: vec![],
            },
        );

        let state = ResolverState {
            secrets,
            flags,
            segments,
            bitsets: HashMap::new(),
        };

        let context_json = r#"{"targeting_key": "roug", "user": {"email": "test@example.com"}}"#;
        let resolver: AccountResolver<'_, L> = state
            .get_resolver_with_json_context(SECRET, context_json, &ENCRYPTION_KEY)
            .unwrap();
        let flag = resolver
            .state
            .flags
            .get("flags/unrecognized-rule-flag")
            .unwrap();
        let resolved_value = resolver
            .resolve_flag(flag, &mut MaterializationContext::discovery())
            .unwrap();

        assert_eq!(
            resolved_value.reason(),
            ResolveReason::NoSegmentMatch,
            "Unrecognized targeting rule should cause the rule to be skipped, not fail the flag"
        );
    }
}
