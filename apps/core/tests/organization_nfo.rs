mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::organization::organization_fixture_with_nfo;
use mediaflow_core::discovery::model::{FsBoundaryError, RelativePath, RootId};
use mediaflow_core::organization::executor::{
    ExecutionOutcome, OrganizationExecutor, RollbackCommand,
};
use mediaflow_core::organization::fs::{
    AppliedFile, CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec,
    NfoOperationSpec, ObservedFile, OrganizationFs, ProcessingStopToken, TargetCapabilitySnapshot,
    VerifiedOperation,
};
use mediaflow_core::organization::journal_store::{
    JournalKind, JournalStatus, JournalStore, LocalNfoStatus, LocalResultStatus,
};
use mediaflow_core::organization::model::{
    ConfirmedNfoInput, ConfirmedNfoMedia, ConfirmedProviderId, NfoConfidence, NfoProvider,
    OrganizationNfoPolicy, OrganizationOperation,
};
use mediaflow_core::organization::nfo::{
    MAX_NFO_BYTES, NfoDecision, NfoGenerationError, NfoGenerator,
};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use sha2::{Digest as _, Sha256};

const MOVIE_FIXTURE: &[u8] = include_bytes!("fixtures/organization-nfo/movie.xml");
const EPISODE_FIXTURE: &[u8] = include_bytes!("fixtures/organization-nfo/episode.xml");
const GENERIC_FIXTURE: &[u8] = include_bytes!("fixtures/organization-nfo/generic.xml");

#[test]
fn generates_exact_deterministic_documents_for_each_supported_kind() {
    let generator = NfoGenerator;
    let cases = [
        (
            movie_input(Some("A chosen path & a dangerous desert.".to_owned())),
            MOVIE_FIXTURE,
        ),
        (
            ConfirmedNfoInput {
                confidence: NfoConfidence::Confirmed,
                file_name: "Pilot.nfo".to_owned(),
                media: ConfirmedNfoMedia::Episode {
                    title: "Pilot & Return".to_owned(),
                    original_title: None,
                    year: None,
                    season: 1,
                    episodes: vec![2, 1, 2],
                    plot: None,
                },
                provider_id: Some(ConfirmedProviderId {
                    provider: NfoProvider::Tvdb,
                    value: "121361".to_owned(),
                }),
            },
            EPISODE_FIXTURE,
        ),
        (
            ConfirmedNfoInput {
                confidence: NfoConfidence::Confirmed,
                file_name: "Camera Roll.nfo".to_owned(),
                media: ConfirmedNfoMedia::GenericVideo {
                    title: "Camera <Roll>".to_owned(),
                    sequence: 7,
                },
                provider_id: None,
            },
            GENERIC_FIXTURE,
        ),
    ];

    for (input, expected) in cases {
        let first = generated_bytes(generator.decide(&input, None).unwrap());
        let second = generated_bytes(generator.decide(&input, None).unwrap());
        assert_eq!(first, expected);
        assert_eq!(second, expected);
    }
}

#[test]
fn omits_absent_optional_fields_and_emits_only_the_confirmed_provider_id() {
    let input = ConfirmedNfoInput {
        confidence: NfoConfidence::Confirmed,
        file_name: "Dune.nfo".to_owned(),
        media: ConfirmedNfoMedia::Movie {
            title: "Dune".to_owned(),
            original_title: None,
            year: None,
            plot: None,
        },
        provider_id: Some(ConfirmedProviderId {
            provider: NfoProvider::Tmdb,
            value: "438631".to_owned(),
        }),
    };

    let bytes = generated_bytes(NfoGenerator.decide(&input, None).unwrap());
    let xml = String::from_utf8(bytes).unwrap();
    assert!(!xml.contains("<year>"));
    assert!(!xml.contains("<plot>"));
    assert!(!xml.contains("<originaltitle>"));
    assert_eq!(xml.matches("<uniqueid ").count(), 1);
    assert!(xml.contains(">438631</uniqueid>"));
    assert!(!xml.contains("tt-secret-unconfirmed"));
    assert!(!xml.contains("tvdb-secret-unconfirmed"));
}

#[test]
fn preserves_any_existing_bytes_without_parsing_or_regenerating() {
    for existing in [
        b"<movie><custom>keep me</custom></movie>".as_slice(),
        b"\0not xml & still user-owned".as_slice(),
    ] {
        let decision = NfoGenerator
            .decide(&movie_input(None), Some(existing))
            .unwrap();
        assert_eq!(
            decision,
            NfoDecision::Preserve {
                sha256: Sha256::digest(existing).into(),
            }
        );
    }
}

#[test]
fn skips_low_confidence_or_unconfirmed_inputs_and_redacts_debug_output() {
    for confidence in [NfoConfidence::LowConfidence, NfoConfidence::Unconfirmed] {
        let input = ConfirmedNfoInput {
            confidence,
            file_name: "secret-title.nfo".to_owned(),
            media: ConfirmedNfoMedia::Movie {
                title: "secret-title".to_owned(),
                original_title: Some("secret-original".to_owned()),
                year: Some(2024),
                plot: Some("secret-provider-body".to_owned()),
            },
            provider_id: Some(ConfirmedProviderId {
                provider: NfoProvider::Imdb,
                value: "tt-secret-provider-id".to_owned(),
            }),
        };
        let decision = NfoGenerator.decide(&input, None).unwrap();
        assert_eq!(decision, NfoDecision::Skip);
        let debug = format!("{input:?} {decision:?}");
        for secret in [
            "secret-title",
            "secret-original",
            "secret-provider-body",
            "tt-secret-provider-id",
        ] {
            assert!(!debug.contains(secret));
        }
    }
}

#[test]
fn enforces_the_one_mebibyte_generated_document_boundary() {
    let empty = generated_bytes(
        NfoGenerator
            .decide(&movie_input(Some(String::new())), None)
            .unwrap(),
    );
    let overhead = empty.len();
    let exact_plot = "x".repeat(MAX_NFO_BYTES - overhead);
    let exact = generated_bytes(
        NfoGenerator
            .decide(&movie_input(Some(exact_plot)), None)
            .unwrap(),
    );
    assert_eq!(exact.len(), MAX_NFO_BYTES);

    let too_large = "x".repeat(MAX_NFO_BYTES - overhead + 1);
    assert_eq!(
        NfoGenerator.decide(&movie_input(Some(too_large)), None),
        Err(NfoGenerationError::TooLarge)
    );
}

#[test]
fn rejects_unbounded_or_unsafe_confirmed_fields_without_echoing_them() {
    let mut unsafe_input = movie_input(None);
    unsafe_input.file_name = "../escape.nfo".to_owned();
    assert_eq!(
        NfoGenerator.decide(&unsafe_input, None),
        Err(NfoGenerationError::InvalidInput)
    );

    let mut invalid_provider = movie_input(None);
    invalid_provider.provider_id = Some(ConfirmedProviderId {
        provider: NfoProvider::Tmdb,
        value: "438631<provider-raw>".to_owned(),
    });
    let error = NfoGenerator.decide(&invalid_provider, None).unwrap_err();
    assert_eq!(error, NfoGenerationError::InvalidInput);
    assert!(!format!("{error:?} {invalid_provider:?}").contains("provider-raw"));
}

#[tokio::test]
async fn executor_generates_after_media_verification_and_rolls_back_both_created_files() {
    let fixture = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    let executor = executor(&fixture, fixture.fs.clone());

    let ExecutionOutcome::Completed(result) = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("file and NFO must complete");
    };
    assert_eq!(result.status, LocalResultStatus::Completed);
    assert_eq!(result.nfo_status, LocalNfoStatus::Generated);
    assert!(fixture.destination_path().exists());
    let generated = std::fs::read(fixture.nfo_path()).unwrap();
    assert!(
        String::from_utf8(generated)
            .unwrap()
            .contains("<title>Arrival</title>")
    );
    let journals = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(journals.len(), 2);
    assert!(
        journals
            .iter()
            .all(|journal| journal.status == JournalStatus::Verified)
    );
    assert!(
        journals
            .iter()
            .any(|journal| journal.kind == JournalKind::Nfo)
    );

    let compensated = executor
        .rollback(RollbackCommand {
            task_id: fixture.lease.task.id,
            result_version: result.version,
            idempotency_key: "rollback-generated-nfo".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(compensated.status, LocalResultStatus::Compensated);
    assert!(!fixture.destination_path().exists());
    assert!(!fixture.nfo_path().exists());
    assert!(fixture.source_path().exists());
}

#[tokio::test]
async fn executor_preserves_arbitrary_existing_nfo_bytes_and_never_compensates_them() {
    let fixture = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    let existing = b"\0not valid XML <custom>user owned</custom>";
    std::fs::create_dir_all(fixture.nfo_path().parent().unwrap()).unwrap();
    std::fs::write(fixture.nfo_path(), existing).unwrap();
    let executor = executor(&fixture, fixture.fs.clone());

    let ExecutionOutcome::Completed(result) = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("existing NFO must be preserved");
    };
    assert_eq!(result.nfo_status, LocalNfoStatus::Preserved);
    assert_eq!(std::fs::read(fixture.nfo_path()).unwrap(), existing);

    let compensated = executor
        .rollback(RollbackCommand {
            task_id: fixture.lease.task.id,
            result_version: result.version,
            idempotency_key: "rollback-preserved-nfo".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(compensated.status, LocalResultStatus::Compensated);
    assert_eq!(std::fs::read(fixture.nfo_path()).unwrap(), existing);
    assert!(!fixture.destination_path().exists());
}

#[tokio::test]
async fn retry_after_nfo_failure_does_not_repeat_the_verified_media_operation() {
    let fixture = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    let counts = Arc::new(FailureCounts::default());
    let fs: Arc<dyn OrganizationFs> = Arc::new(FailFirstNfoFs {
        inner: fixture.fs.clone(),
        counts: counts.clone(),
        fail_nfo: AtomicBool::new(true),
        write_before_failure: false,
    });
    let executor = executor(&fixture, fs);

    let first = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(first, ExecutionOutcome::RecoveryPending { .. }));
    let partial = JournalStore::new(fixture.db.pool().clone())
        .result_for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(partial.status, LocalResultStatus::PartialSuccess);
    assert_eq!(partial.nfo_status, LocalNfoStatus::Failed);
    assert_eq!(partial.remaining_actions, vec!["nfo"]);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);

    let ExecutionOutcome::Completed(completed) = executor.recover(&fixture.lease).await.unwrap()
    else {
        panic!("NFO-only recovery must complete");
    };
    assert_eq!(completed.nfo_status, LocalNfoStatus::Generated);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);
    assert_eq!(counts.nfo_writes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recovery_recognizes_a_generated_nfo_after_write_response_loss() {
    let fixture = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    let counts = Arc::new(FailureCounts::default());
    let fs: Arc<dyn OrganizationFs> = Arc::new(FailFirstNfoFs {
        inner: fixture.fs.clone(),
        counts: counts.clone(),
        fail_nfo: AtomicBool::new(true),
        write_before_failure: true,
    });
    let executor = executor(&fixture, fs);

    assert!(matches!(
        executor
            .run_next(&fixture.lease, ProcessingStopToken::default())
            .await
            .unwrap(),
        ExecutionOutcome::RecoveryPending { .. }
    ));
    assert!(fixture.nfo_path().exists());
    let executing = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap()
        .into_iter()
        .find(|journal| journal.kind == JournalKind::Nfo)
        .unwrap();
    assert_eq!(executing.status, JournalStatus::Executing);
    assert_eq!(executing.nfo_preexisting, Some(false));

    let ExecutionOutcome::Completed(result) = executor.recover(&fixture.lease).await.unwrap()
    else {
        panic!("matching generated bytes must recover without a second write");
    };
    assert_eq!(result.nfo_status, LocalNfoStatus::Generated);
    assert_eq!(counts.file_copies.load(Ordering::SeqCst), 1);
    assert_eq!(counts.nfo_writes.load(Ordering::SeqCst), 1);
}

fn movie_input(plot: Option<String>) -> ConfirmedNfoInput {
    ConfirmedNfoInput {
        confidence: NfoConfidence::Confirmed,
        file_name: "Dune.nfo".to_owned(),
        media: ConfirmedNfoMedia::Movie {
            title: "Dune & Arrakis <Part One>".to_owned(),
            original_title: Some("Dune: Part One".to_owned()),
            year: Some(2021),
            plot,
        },
        provider_id: Some(ConfirmedProviderId {
            provider: NfoProvider::Tmdb,
            value: "438631".to_owned(),
        }),
    }
}

fn generated_bytes(decision: NfoDecision) -> Vec<u8> {
    match decision {
        NfoDecision::Generate { file_name, bytes } => {
            assert_eq!(
                std::path::Path::new(&file_name).extension(),
                Some(std::ffi::OsStr::new("nfo"))
            );
            bytes
        }
        unexpected => panic!("expected generated NFO, got {unexpected:?}"),
    }
}

fn executor(
    fixture: &common::organization::OrganizationFixture,
    fs: Arc<dyn OrganizationFs>,
) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fs,
        Arc::new(ManualTaskClock::new(7_000)),
    )
}

#[derive(Default)]
struct FailureCounts {
    file_copies: AtomicUsize,
    nfo_writes: AtomicUsize,
}

struct FailFirstNfoFs {
    inner: Arc<mediaflow_core::platform::capability_fs::OsCapabilityFs>,
    counts: Arc<FailureCounts>,
    fail_nfo: AtomicBool,
    write_before_failure: bool,
}

impl OrganizationFs for FailFirstNfoFs {
    fn preflight_target(
        &self,
        root: &RootId,
        path: &RelativePath,
    ) -> Result<TargetCapabilitySnapshot, FsBoundaryError> {
        self.inner.preflight_target(root, path)
    }

    fn observe(&self, locator: &FileLocator) -> Result<Option<ObservedFile>, FsBoundaryError> {
        self.inner.observe(locator)
    }

    fn inspect(&self, locator: &FileLocator) -> Result<Option<AppliedFile>, FsBoundaryError> {
        self.inner.inspect(locator)
    }

    fn copy_no_clobber(
        &self,
        operation: &FileOperationSpec,
        stop: &ProcessingStopToken,
    ) -> Result<AppliedFile, FileOperationError> {
        self.counts.file_copies.fetch_add(1, Ordering::SeqCst);
        self.inner.copy_no_clobber(operation, stop)
    }

    fn move_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.move_no_clobber(operation)
    }

    fn hardlink_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.inner.hardlink_no_clobber(operation)
    }

    fn write_new_nfo(
        &self,
        operation: &NfoOperationSpec,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError> {
        self.counts.nfo_writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_nfo.swap(false, Ordering::SeqCst) {
            if self.write_before_failure {
                self.inner.write_new_nfo(operation, bytes)?;
            }
            Err(FileOperationError::IoTemporary)
        } else {
            self.inner.write_new_nfo(operation, bytes)
        }
    }

    fn remove_verified_source(
        &self,
        operation_id: uuid::Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError> {
        self.inner
            .remove_verified_source(operation_id, source, expected_sha256)
    }

    fn compensate(
        &self,
        operation: &VerifiedOperation,
    ) -> Result<CompensationOutcome, FileOperationError> {
        self.inner.compensate(operation)
    }
}
