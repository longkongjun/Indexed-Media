//! Durable execution ownership and atomic completion for the capability gateway.
//! No timer or process restart reclaims unknown work; domain receipts remain authoritative.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::Row;

use super::model::{CapabilityCall, CapabilityEvent, CapabilityResult};
use super::store::{CapabilityCallRecord, CapabilityStore};
use crate::shared::error::{AppError, ErrorCode};

const PRINCIPAL: &str = "capability:dsh";

pub(crate) enum ClaimOutcome {
    Available,
    Owned,
    Replay(Box<CapabilityResult>),
    Conflict,
    Pending,
}

fn fingerprint(call: &CapabilityCall) -> Result<Vec<u8>, AppError> {
    fn canonical(value: Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut entries = map.into_iter().collect::<Vec<_>>();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
            other => other,
        }
    }
    let value = canonical(serde_json::to_value(call).map_err(internal)?);
    Ok(Sha256::digest(serde_json::to_vec(&value).map_err(internal)?).to_vec())
}

impl CapabilityStore {
    /// Check a replay, or acquire the sole durable owner after read-only preflight.
    pub(crate) async fn claim(
        &self,
        call: &CapabilityCall,
        reserve: bool,
        now_us: i64,
    ) -> Result<ClaimOutcome, AppError> {
        let fingerprint = fingerprint(call)?;
        let mut tx = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let row = sqlx::query("SELECT server_principal,request_fingerprint,state FROM capability_request_claims WHERE request_id=?")
            .bind(call.request_id.as_bytes().as_slice()).fetch_optional(&mut *tx).await.map_err(internal)?;
        let outcome = if let Some(row) = row {
            if row.get::<String, _>("server_principal") != PRINCIPAL
                || row.get::<Vec<u8>, _>("request_fingerprint") != fingerprint
            {
                ClaimOutcome::Conflict
            } else if row.get::<String, _>("state") == "completed" {
                let result: String = sqlx::query_scalar(
                    "SELECT result_json FROM capability_calls WHERE request_id=?",
                )
                .bind(call.request_id.as_bytes().as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
                ClaimOutcome::Replay(Box::new(serde_json::from_str(&result).map_err(internal)?))
            } else {
                ClaimOutcome::Pending
            }
        } else {
            let occupied: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM capability_request_claims WHERE server_principal=? AND capability=? AND idempotency_key=?) OR EXISTS(SELECT 1 FROM capability_calls WHERE request_id=? OR (capability=? AND idempotency_key=?))")
                .bind(PRINCIPAL).bind(call.capability.as_str()).bind(&call.execution.idempotency_key)
                .bind(call.request_id.as_bytes().as_slice()).bind(call.capability.as_str()).bind(&call.execution.idempotency_key)
                .fetch_one(&mut *tx).await.map_err(internal)?;
            if occupied != 0 {
                ClaimOutcome::Conflict
            } else if reserve {
                sqlx::query("INSERT INTO capability_request_claims (request_id,server_principal,capability,idempotency_key,request_fingerprint,state,created_at_us) VALUES (?,?,?,?,?,'pending',?)")
                    .bind(call.request_id.as_bytes().as_slice()).bind(PRINCIPAL).bind(call.capability.as_str())
                    .bind(&call.execution.idempotency_key).bind(fingerprint).bind(now_us)
                    .execute(&mut *tx).await.map_err(internal)?;
                ClaimOutcome::Owned
            } else {
                ClaimOutcome::Available
            }
        };
        tx.commit().await.map_err(internal)?;
        Ok(outcome)
    }

    /// Commit result, event and completion together; any failure leaves the claim pending.
    pub(crate) async fn complete_claim(
        &self,
        call: &CapabilityCall,
        record: &CapabilityCallRecord,
        event: &CapabilityEvent,
        now_us: i64,
    ) -> Result<(), AppError> {
        let mut tx = self
            .pool()
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let updated = sqlx::query("UPDATE capability_request_claims SET state='completed' WHERE request_id=? AND server_principal=? AND request_fingerprint=? AND state='pending'")
            .bind(call.request_id.as_bytes().as_slice()).bind(PRINCIPAL).bind(fingerprint(call)?)
            .execute(&mut *tx).await.map_err(internal)?.rows_affected();
        if updated != 1 {
            return Err(AppError::new(
                ErrorCode::RequestConflict,
                "capability execution ownership is unavailable",
            ));
        }
        sqlx::query("INSERT INTO capability_calls (request_id,idempotency_key,capability,actor_kind,actor_id,status,result_json,error_json,created_at_us) VALUES (?,?,?,'ai','dsh',?,?,NULL,?)")
            .bind(record.request_id.as_bytes().as_slice()).bind(&record.idempotency_key).bind(&record.capability)
            .bind(serde_json::to_value(record.result.status).map_err(internal)?.as_str().ok_or_else(|| AppError::new(ErrorCode::Internal,"invalid capability status"))?)
            .bind(serde_json::to_string(&record.result).map_err(internal)?).bind(now_us)
            .execute(&mut *tx).await.map_err(internal)?;
        sqlx::query("INSERT INTO capability_events (event_id,event_type,occurred_at_us,task_id,request_id,operation_id,state,progress_json,evidence_json,error_json) VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(event.event_id.as_bytes().as_slice())
            .bind(serde_json::to_value(event.event_type).map_err(internal)?.as_str().ok_or_else(|| AppError::new(ErrorCode::Internal,"invalid capability event"))?)
            .bind(now_us).bind(event.task_id.map(|id| id.as_bytes().to_vec()))
            .bind(event.request_id.map(|id| id.as_bytes().to_vec())).bind(&event.operation_id).bind(&event.state)
            .bind(optional_json(event.progress.as_ref())?).bind(optional_json(event.evidence_refs.as_ref())?).bind(optional_json(event.error.as_ref())?)
            .execute(&mut *tx).await.map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(())
    }
}

fn optional_json<T: Serialize>(value: Option<&T>) -> Result<Option<String>, AppError> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(internal)
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
