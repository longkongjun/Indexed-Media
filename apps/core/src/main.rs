use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use mediaflow_core::bootstrap::app::{
    assemble_automation_runtime, assemble_discovery_runtime, assemble_download_task_runtime,
    assemble_processing_runtime,
};
use mediaflow_core::bootstrap::config::AppConfig;
use mediaflow_core::platform::backup::{restore_backup, verify_database};
use mediaflow_core::platform::http::build_router_with_outbox;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxMaintenanceRuntime, OutboxNotifier};
use mediaflow_core::platform::task_runtime::TaskRuntime;
use mediaflow_core::shared::error::{AppError, ErrorCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Parser)]
#[command(name = "mediaflow-core")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
    Healthcheck,
    VerifyDatabase {
        #[arg(long)]
        database: PathBuf,
    },
    RestoreBackup {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        config_dir: PathBuf,
    },
    Argon2Calibrate,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("{}", error.safe_message());
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), AppError> {
    match cli.command {
        Command::Serve => serve().await,
        Command::Healthcheck => healthcheck().await,
        Command::VerifyDatabase { database } => {
            let report = verify_database(&database).await?;
            print_report(&report)
        }
        Command::RestoreBackup { backup, config_dir } => {
            let report = restore_backup(&backup, &config_dir).await?;
            print_report(&report)
        }
        Command::Argon2Calibrate => {
            let report = mediaflow_core::platform::password::calibrate()?;
            print_report(&report)
        }
    }
}

async fn serve() -> Result<(), AppError> {
    let config = AppConfig::load()?;
    std::fs::create_dir_all(&config.config_dir)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let task_roots = TaskRuntime::validate_roots(&config)?;
    let db = migrate_with_backup(&config).await?;
    let outbox_notifier = OutboxNotifier::new();
    let task_runtime = TaskRuntime::from_validated_with_notifier(
        &config,
        &db,
        task_roots,
        outbox_notifier.clone(),
    );
    task_runtime.prepare().await?;
    let processing_runtime = assemble_processing_runtime(&config, &db, outbox_notifier.clone())?;
    processing_runtime.prepare().await?;
    let download_runtime = assemble_download_task_runtime(&db, outbox_notifier.clone())?;
    download_runtime.prepare().await?;
    let automation_runtime = assemble_automation_runtime(&config, &db, outbox_notifier.clone())?;
    automation_runtime.prepare().await?;
    let mut discovery_runtime = assemble_discovery_runtime(&config, &db, outbox_notifier.clone())?;
    discovery_runtime.prepare().await?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;

    let task_runtime_handle = task_runtime.start();
    let processing_runtime_handle = processing_runtime.start();
    let download_runtime_handle = download_runtime.start();
    let automation_runtime_handle = automation_runtime.start();
    let discovery_runtime_handle = discovery_runtime.start();
    let outbox_runtime_handle = OutboxMaintenanceRuntime::new(db.pool().clone()).start();
    let serve_result = axum::serve(
        listener,
        build_router_with_outbox(config, Some(db), outbox_notifier)
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|error| AppError::with_source(ErrorCode::Internal, error));
    task_runtime_handle.abort();
    processing_runtime_handle.abort();
    download_runtime_handle.abort();
    automation_runtime_handle.abort();
    discovery_runtime_handle.abort();
    outbox_runtime_handle.abort();
    let _ = task_runtime_handle.await;
    let _ = processing_runtime_handle.await;
    let _ = download_runtime_handle.await;
    let _ = automation_runtime_handle.await;
    let _ = discovery_runtime_handle.await;
    let _ = outbox_runtime_handle.await;
    serve_result
}

async fn healthcheck() -> Result<(), AppError> {
    let config = AppConfig::load()?;
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        healthcheck_request(config),
    )
    .await
    .map_err(|error| AppError::with_source(ErrorCode::NotReady, error))?
}

async fn healthcheck_request(config: AppConfig) -> Result<(), AppError> {
    let address = healthcheck_address(config.listen);
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::NotReady, error))?;
    let mut host = config
        .public_origin
        .host()
        .map_or_else(|| "localhost".to_owned(), |host| host.to_string());
    if let Some(port) = config.public_origin.port() {
        host.push(':');
        host.push_str(&port.to_string());
    }
    let request =
        format!("GET /health/ready HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| AppError::with_source(ErrorCode::NotReady, error))?;
    let mut response = Vec::with_capacity(512);
    stream
        .take(4_096)
        .read_to_end(&mut response)
        .await
        .map_err(|error| AppError::with_source(ErrorCode::NotReady, error))?;
    if response.starts_with(b"HTTP/1.1 200 ") || response.starts_with(b"HTTP/1.0 200 ") {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::NotReady,
            "readiness endpoint did not return HTTP 200",
        ))
    }
}

fn healthcheck_address(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, address.port())
}

fn print_report(report: &impl serde::Serialize) -> Result<(), AppError> {
    let output = serde_json::to_string_pretty(report)
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    println!("{output}");
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    let _ = result;
                }
                _ = terminate.recv() => {}
            }
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
