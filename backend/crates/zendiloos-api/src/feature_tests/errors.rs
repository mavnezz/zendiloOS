//! What each context's errors become in the API: status and `code` are the
//! contract the web interface words its messages by.

use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use serde_json::Value;
use zendiloos_apps::application::AppsError;
use zendiloos_containers::application::ContainersError;
use zendiloos_kernel::PortError;
use zendiloos_network::application::ConfigureError;
use zendiloos_notify::channels::ChannelError;
use zendiloos_sharing::application::{FilesError, SharingError};
use zendiloos_storage::application::StorageError;
use zendiloos_storage::domain as storage;
use zendiloos_system::application::commands::{PowerError, SetSshError};
use zendiloos_updates::application::UpdatesError;
use zendiloos_updates::application::ports::HostError;

use crate::error::ApiError;

/// Status, code and detail of the problem `error` is answered with.
async fn problem(error: impl Into<ApiError>) -> (StatusCode, String, Option<String>) {
    let response = error.into().into_response();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    (status, body["code"].as_str().unwrap().to_owned(), body["detail"].as_str().map(str::to_owned))
}

async fn assert_maps(error: impl Into<ApiError>, status: StatusCode, code: &str) -> Option<String> {
    let (got_status, got_code, detail) = problem(error).await;
    assert_eq!((got_status, got_code.as_str()), (status, code));
    detail
}

fn port() -> PortError {
    PortError::new("test", "broken")
}

#[tokio::test]
async fn storage_errors() {
    use storage::PlanError as P;
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    assert_maps(StorageError::DiskNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(StorageError::Plan(P::PoolNotFound), StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(StorageError::Conflict, StatusCode::CONFLICT, "name_taken").await;
    assert_maps(StorageError::Plan(P::NameTaken), StatusCode::CONFLICT, "name_taken").await;
    let count =
        assert_maps(StorageError::Plan(P::DiskCount { minimum: 2, maximum: 8 }), unprocessable, "disk_count");
    assert!(count.await.is_some());
    let disk =
        assert_maps(StorageError::Plan(P::DiskNotFound("sdz".into())), unprocessable, "disk_not_found");
    assert_eq!(disk.await.as_deref(), Some("sdz"));
    for (reason, word) in [
        (storage::Unusable::System, "system"),
        (storage::Unusable::ArrayMember, "array_member"),
        (storage::Unusable::PoolMember, "pool_member"),
        (storage::Unusable::NoStableId, "no_stable_id"),
        (storage::Unusable::ReadOnly, "read_only"),
    ] {
        let error = StorageError::Plan(P::DiskUnusable { disk: "sda".into(), reason });
        let detail = assert_maps(error, unprocessable, "disk_unusable").await;
        assert_eq!(detail, Some(format!("sda: {word}")));
    }
    assert_maps(StorageError::Plan(P::DuplicateDisk("sda".into())), unprocessable, "duplicate_disk").await;
    assert_maps(StorageError::Plan(P::NoVolumeNumber), StatusCode::CONFLICT, "no_volume_number").await;
    assert_maps(StorageError::Plan(P::UnsupportedLayout), unprocessable, "unsupported_layout").await;
    assert_maps(StorageError::Plan(P::PoolNotManaged), StatusCode::CONFLICT, "pool_not_managed").await;
    assert_maps(StorageError::Plan(P::DeviceNotFound), unprocessable, "device_not_found").await;
    assert_maps(StorageError::Plan(P::DiskTooSmall), unprocessable, "disk_too_small").await;
    assert_maps(StorageError::PlanExpired, StatusCode::CONFLICT, "plan_expired").await;
    assert_maps(StorageError::ConfirmationMismatch, unprocessable, "confirmation_mismatch").await;
    assert_maps(StorageError::PlanChanged, StatusCode::CONFLICT, "plan_changed").await;
    assert_maps(StorageError::Refused("busy".into()), StatusCode::CONFLICT, "refused").await;
    assert_maps(StorageError::NotScrubbing, StatusCode::CONFLICT, "not_scrubbing").await;
    assert_maps(StorageError::SelfTestUnsupported, StatusCode::CONFLICT, "self_test_unsupported").await;
    assert_maps(StorageError::SelfTestRunning, StatusCode::CONFLICT, "self_test_running").await;
    assert_maps(StorageError::NotTesting, StatusCode::CONFLICT, "not_testing").await;
    let occupants = vec![
        storage::Occupant::Share("photos".into()),
        storage::Occupant::App("immich".into()),
        storage::Occupant::Homes,
        storage::Occupant::VirtualMachines,
        storage::Occupant::Docker,
    ];
    let detail = assert_maps(StorageError::PoolInUse(occupants), StatusCode::CONFLICT, "pool_in_use").await;
    assert_eq!(
        detail.as_deref(),
        Some("share photos, app immich, the personal folders, the virtual machines' storage, Docker's data")
    );
    assert_maps(StorageError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[tokio::test]
async fn container_errors() {
    assert_maps(ContainersError::NotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(ContainersError::Invalid("x".into()), StatusCode::UNPROCESSABLE_ENTITY, "invalid_input")
        .await;
    assert_maps(ContainersError::Refused("x".into()), StatusCode::CONFLICT, "refused").await;
    assert_maps(ContainersError::Failed("x".into()), StatusCode::BAD_GATEWAY, "host_failed").await;
    assert_maps(ContainersError::NameTaken, StatusCode::CONFLICT, "project_exists").await;
    assert_maps(ContainersError::AppProject, StatusCode::CONFLICT, "app_project").await;
    assert_maps(ContainersError::NotOwn, StatusCode::CONFLICT, "not_own_project").await;
    assert_maps(ContainersError::Denied, StatusCode::FORBIDDEN, "share_denied").await;
    assert_maps(ContainersError::Unconfirmed, StatusCode::CONFLICT, "project_unconfirmed").await;
    assert_maps(ContainersError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[tokio::test]
async fn app_errors() {
    let conflict = StatusCode::CONFLICT;
    assert_maps(AppsError::SourceNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(AppsError::PoolNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(AppsError::NotInstalled, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(AppsError::Duplicate, conflict, "source_exists").await;
    assert_maps(AppsError::Official, conflict, "official_source").await;
    assert_maps(AppsError::NotACatalog("x".into()), StatusCode::UNPROCESSABLE_ENTITY, "not_a_catalog").await;
    assert_maps(AppsError::Unreachable("x".into()), StatusCode::BAD_GATEWAY, "catalog_unreachable").await;
    assert_maps(AppsError::Refused("x".into()), conflict, "refused").await;
    assert_maps(AppsError::DockerInstalled, conflict, "docker_installed").await;
    let why = assert_maps(
        AppsError::Template("line 3: no".into()),
        StatusCode::UNPROCESSABLE_ENTITY,
        "template_unfillable",
    );
    assert_eq!(why.await.as_deref(), Some("line 3: no"));
    assert_maps(AppsError::Busy, conflict, "app_busy").await;
    assert_maps(AppsError::ConfirmationMismatch, StatusCode::UNPROCESSABLE_ENTITY, "confirmation_mismatch")
        .await;
    assert_maps(AppsError::HostFailed("x".into()), StatusCode::BAD_GATEWAY, "host_failed").await;
    assert_maps(AppsError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[tokio::test]
async fn sharing_and_file_errors() {
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    assert_maps(SharingError::InvalidDescription, unprocessable, "invalid_input").await;
    assert_maps(SharingError::NameTaken, StatusCode::CONFLICT, "name_taken").await;
    assert_maps(SharingError::VolumeNotFound, unprocessable, "volume_not_found").await;
    assert_maps(SharingError::ShareNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(SharingError::UnknownSubject, unprocessable, "unknown_subject").await;
    assert_maps(SharingError::Duplicate, unprocessable, "duplicate_rule").await;
    assert_maps(SharingError::ConfirmationMismatch, unprocessable, "confirmation_mismatch").await;
    assert_maps(SharingError::Refused("x".into()), StatusCode::CONFLICT, "refused").await;
    assert_maps(SharingError::UsedByApps("immich".into()), StatusCode::CONFLICT, "share_used_by_apps").await;
    assert_maps(SharingError::UsedByProjects("tesla".into()), StatusCode::CONFLICT, "share_used_by_projects")
        .await;
    assert_maps(SharingError::SnapshotNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(SharingError::SnapshotTaken, StatusCode::CONFLICT, "snapshot_taken").await;
    assert_maps(SharingError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    assert_maps(FilesError::ShareNotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(FilesError::NotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(FilesError::Denied, StatusCode::FORBIDDEN, "permission_denied").await;
    assert_maps(FilesError::Exists, StatusCode::CONFLICT, "name_taken").await;
    assert_maps(FilesError::InvalidName, unprocessable, "invalid_input").await;
    assert_maps(FilesError::Invalid("x".into()), unprocessable, "invalid_operation").await;
    assert_maps(FilesError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[tokio::test]
async fn update_network_channel_and_system_errors() {
    assert_maps(UpdatesError::NotChecked, StatusCode::CONFLICT, "not_checked").await;
    assert_maps(UpdatesError::NothingToDo, StatusCode::CONFLICT, "nothing_to_update").await;
    assert_maps(UpdatesError::Busy, StatusCode::CONFLICT, "update_busy").await;
    assert_maps(UpdatesError::JobsRunning("x".into()), StatusCode::CONFLICT, "jobs_running").await;
    assert_maps(UpdatesError::Host(HostError::Refused("x".into())), StatusCode::CONFLICT, "refused").await;
    assert_maps(UpdatesError::Host(HostError::Failed("x".into())), StatusCode::BAD_GATEWAY, "host_failed")
        .await;
    assert_maps(UpdatesError::Host(HostError::Port(port())), StatusCode::INTERNAL_SERVER_ERROR, "internal")
        .await;
    assert_maps(UpdatesError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    assert_maps(ConfigureError::Refused("x".into()), StatusCode::CONFLICT, "refused").await;
    assert_maps(ConfigureError::Pending, StatusCode::CONFLICT, "change_pending").await;
    assert_maps(ConfigureError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    assert_maps(ChannelError::NotFound, StatusCode::NOT_FOUND, "not_found").await;
    assert_maps(ChannelError::NameTaken, StatusCode::CONFLICT, "name_taken").await;
    assert_maps(ChannelError::TooMany, StatusCode::UNPROCESSABLE_ENTITY, "invalid_input").await;
    assert_maps(ChannelError::Delivery("x".into()), StatusCode::BAD_GATEWAY, "delivery_failed").await;
    assert_maps(ChannelError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;

    assert_maps(PowerError::JobsRunning("x".into()), StatusCode::CONFLICT, "jobs_running").await;
    assert_maps(PowerError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
    assert_maps(SetSshError::NotInstalled, StatusCode::CONFLICT, "not_installed").await;
    let port_in_use = assert_maps(SetSshError::PortInUse(2222), StatusCode::CONFLICT, "port_in_use").await;
    assert!(port_in_use.unwrap().contains("2222"));
    assert_maps(SetSshError::Port(port()), StatusCode::INTERNAL_SERVER_ERROR, "internal").await;
}

#[tokio::test]
async fn the_frameworks_own_errors() {
    use std::time::Duration;
    assert_maps(ApiError::MethodNotAllowed, StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed").await;
    assert_maps(ApiError::WrongPassword, StatusCode::UNPROCESSABLE_ENTITY, "wrong_password").await;
    assert_maps(ApiError::AccountDisabled, StatusCode::FORBIDDEN, "account_disabled").await;
    assert_maps(ApiError::SetupCompleted, StatusCode::CONFLICT, "setup_completed").await;
    assert_maps(ApiError::InvalidSetupCode, StatusCode::UNPROCESSABLE_ENTITY, "invalid_setup_code").await;
    assert_maps(ApiError::SetupNotLocal, StatusCode::FORBIDDEN, "setup_not_local").await;
    assert_maps(ApiError::LastAdmin, StatusCode::CONFLICT, "last_admin").await;
    assert_maps(ApiError::OwnAccount, StatusCode::CONFLICT, "own_account").await;
    let response = ApiError::TooManyAttempts { retry_after: Duration::from_millis(10) }.into_response();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "1", "at least a second");
}
