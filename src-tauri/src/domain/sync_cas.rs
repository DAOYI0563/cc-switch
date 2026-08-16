use serde::{Deserialize, Serialize};

use super::{
    merge_sync_records, DomainError, DomainErrorCode, Sha256Digest, SyncEtag, SyncMergeBatch,
    SyncMergeInput, SyncRecord, SyncRecordBaseline, SyncRecordIndexEntry, SyncSchemaVersion,
    SyncV3Manifest, SyncWriteCondition,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCasAttemptKind {
    Initial,
    RemergeOnce,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncCasRemoteGuard {
    pub schema_version: SyncSchemaVersion,
    pub generation: u64,
    pub manifest_sha256: Sha256Digest,
    pub etag: SyncEtag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncCasAttemptInput {
    pub schema_version: SyncSchemaVersion,
    pub attempt: SyncCasAttemptKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_failed_guard: Option<SyncCasRemoteGuard>,
    pub manifest: SyncV3Manifest,
    pub etag: SyncEtag,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
    pub remote_records: Vec<SyncRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncCasAttemptPlan {
    pub schema_version: SyncSchemaVersion,
    pub attempt: SyncCasAttemptKind,
    pub remote_guard: SyncCasRemoteGuard,
    pub write_condition: SyncWriteCondition,
    pub merge_batch: SyncMergeBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCasFailureKind {
    Authentication,
    LimitExceeded,
    Timeout,
    Connection,
    RemoteRejected,
    InvalidResponse,
    Transport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncCasWriteOutcome {
    Committed {
        #[serde(skip_serializing_if = "Option::is_none")]
        etag: Option<SyncEtag>,
    },
    PreconditionFailed,
    Failed {
        failure: SyncCasFailureKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCasStopReason {
    ConcurrentWriteAfterRemerge,
    TransportFailure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncCasDecision {
    ApplyLocalAfterRemoteCommit {
        #[serde(skip_serializing_if = "Option::is_none")]
        committed_etag: Option<SyncEtag>,
        merge_batch: SyncMergeBatch,
    },
    RefetchAndRemergeOnce {
        failed_guard: SyncCasRemoteGuard,
    },
    Stop {
        reason: SyncCasStopReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure: Option<SyncCasFailureKind>,
    },
}

pub fn plan_sync_cas_attempt(
    input: SyncCasAttemptInput,
) -> Result<SyncCasAttemptPlan, DomainError> {
    input.manifest.validate()?;
    validate_remote_snapshot(&input.manifest, &input.remote_records)?;

    let manifest_sha256 = Sha256Digest::of_bytes(&input.manifest.to_canonical_json_bytes()?);
    let remote_guard = SyncCasRemoteGuard {
        schema_version: SyncSchemaVersion::V1,
        generation: input.manifest.generation,
        manifest_sha256,
        etag: input.etag.clone(),
    };
    validate_attempt_progression(
        input.attempt,
        input.previous_failed_guard.as_ref(),
        &remote_guard,
    )?;

    let merge_batch = merge_sync_records(SyncMergeInput {
        schema_version: input.schema_version,
        baselines: input.baselines,
        local_records: input.local_records,
        remote_records: input.remote_records,
    })?;

    Ok(SyncCasAttemptPlan {
        schema_version: SyncSchemaVersion::V1,
        attempt: input.attempt,
        write_condition: SyncWriteCondition::Match(input.etag),
        remote_guard,
        merge_batch,
    })
}

pub fn resolve_sync_cas_write(
    plan: SyncCasAttemptPlan,
    outcome: SyncCasWriteOutcome,
) -> SyncCasDecision {
    match outcome {
        SyncCasWriteOutcome::Committed { etag } => SyncCasDecision::ApplyLocalAfterRemoteCommit {
            committed_etag: etag,
            merge_batch: plan.merge_batch,
        },
        SyncCasWriteOutcome::PreconditionFailed if plan.attempt == SyncCasAttemptKind::Initial => {
            SyncCasDecision::RefetchAndRemergeOnce {
                failed_guard: plan.remote_guard,
            }
        }
        SyncCasWriteOutcome::PreconditionFailed => SyncCasDecision::Stop {
            reason: SyncCasStopReason::ConcurrentWriteAfterRemerge,
            failure: None,
        },
        SyncCasWriteOutcome::Failed { failure } => SyncCasDecision::Stop {
            reason: SyncCasStopReason::TransportFailure,
            failure: Some(failure),
        },
    }
}

fn validate_attempt_progression(
    attempt: SyncCasAttemptKind,
    previous: Option<&SyncCasRemoteGuard>,
    current: &SyncCasRemoteGuard,
) -> Result<(), DomainError> {
    match (attempt, previous) {
        (SyncCasAttemptKind::Initial, None) => Ok(()),
        (SyncCasAttemptKind::RemergeOnce, Some(previous))
            if current.generation > previous.generation
                && current.etag != previous.etag
                && current.manifest_sha256 != previous.manifest_sha256 =>
        {
            Ok(())
        }
        (SyncCasAttemptKind::Initial, Some(_)) => Err(invalid_cas(
            "an initial CAS attempt cannot carry a previous failure",
        )),
        (SyncCasAttemptKind::RemergeOnce, None) => Err(invalid_cas(
            "a remerge attempt must be bound to the first failed guard",
        )),
        (SyncCasAttemptKind::RemergeOnce, Some(_)) => Err(invalid_cas(
            "a remerge attempt must use a freshly fetched newer manifest and ETag",
        )),
    }
}

fn validate_remote_snapshot(
    manifest: &SyncV3Manifest,
    records: &[SyncRecord],
) -> Result<(), DomainError> {
    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    if indexes != manifest.records {
        return Err(invalid_cas(
            "CAS merge records do not match the observed manifest",
        ));
    }
    Ok(())
}

fn invalid_cas(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
