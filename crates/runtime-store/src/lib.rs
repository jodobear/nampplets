//! Persistent runtime metadata separate from NMP canonical state.
//!
//! This schema intentionally has no Nostr event, replacement, deletion,
//! routing, pending-row, or receipt-fact tables. Receipt identifiers are kept
//! only as workspace recovery references for NMP reattachment.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nmp_native_runtime_core::{BoundedJson, Capability, GrantDecision, Principal, WriteReceiptId};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    pub maximum_installs: usize,
    pub maximum_install_title_bytes: usize,
    pub maximum_grants_per_principal: usize,
    pub maximum_kv_keys_per_scope: usize,
    pub maximum_kv_bytes_per_scope: usize,
    pub maximum_value_bytes: usize,
    pub maximum_workspaces: usize,
    pub maximum_workspace_bytes: usize,
    pub maximum_retained_receipts_per_workspace: usize,
    pub maximum_retained_receipt_bytes_per_workspace: usize,
    pub maximum_activity_facts: usize,
    pub maximum_activity_string_bytes: usize,
    pub maximum_activity_record_bytes: usize,
    pub maximum_activity_total_bytes: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            maximum_installs: 512,
            maximum_install_title_bytes: 512,
            maximum_grants_per_principal: 64,
            maximum_kv_keys_per_scope: 1_024,
            maximum_kv_bytes_per_scope: 8 * 1024 * 1024,
            maximum_value_bytes: 512 * 1024,
            maximum_workspaces: 64,
            maximum_workspace_bytes: 512 * 1024,
            maximum_retained_receipts_per_workspace: 256,
            maximum_retained_receipt_bytes_per_workspace: 64 * 1024,
            maximum_activity_facts: 10_000,
            maximum_activity_string_bytes: 512,
            maximum_activity_record_bytes: 1_024,
            maximum_activity_total_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct RuntimeStore {
    path: PathBuf,
    limits: StoreLimits,
    connection: Mutex<Connection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledBuild {
    pub principal: Principal,
    pub title: Arc<str>,
    pub manifest_metadata: BoundedJson,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: Arc<str>,
    pub definition: BoundedJson,
    pub retained_receipts: Vec<WriteReceiptId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityRecord {
    pub principal: Principal,
    pub category: Arc<str>,
    pub operation: Arc<str>,
    pub outcome: Arc<str>,
    pub occurred_at_millis: u64,
}

impl RuntimeStore {
    pub fn open(path: impl AsRef<Path>, limits: StoreLimits) -> Result<Self, StoreError> {
        validate_limits(limits)?;
        let path = path.as_ref().to_path_buf();
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&connection)?;
        Ok(Self {
            path,
            limits,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn install(&self, build: &InstalledBuild) -> Result<(), StoreError> {
        validate_install_title(&build.title, self.limits.maximum_install_title_bytes)?;
        if build.manifest_metadata.byte_len() > self.limits.maximum_value_bytes {
            return Err(StoreError::ManifestMetadataTooLarge {
                actual: build.manifest_metadata.byte_len(),
                maximum: self.limits.maximum_value_bytes,
            });
        }
        let connection = self.connection.lock();
        let exists: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM installations
                WHERE author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
            )",
            principal_params(&build.principal),
            |row| row.get(0),
        )?;
        if !exists {
            let count: usize =
                connection.query_row("SELECT COUNT(*) FROM installations", [], |row| row.get(0))?;
            if count >= self.limits.maximum_installs {
                return Err(StoreError::InstallCapacity {
                    capacity: self.limits.maximum_installs,
                });
            }
        }
        connection.execute(
            "INSERT INTO installations
                (author, d_tag, aggregate_hash, title, manifest_metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(author, d_tag, aggregate_hash) DO UPDATE SET
                title = excluded.title,
                manifest_metadata = excluded.manifest_metadata",
            params![
                build.principal.manifest_author(),
                build.principal.d_tag(),
                build.principal.aggregate_hash(),
                build.title.as_ref(),
                build.manifest_metadata.as_str(),
            ],
        )?;
        Ok(())
    }

    pub fn installed_builds(&self) -> Result<Vec<InstalledBuild>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT author, d_tag, aggregate_hash, title, manifest_metadata
             FROM installations ORDER BY author, d_tag, aggregate_hash
             LIMIT ?1",
        )?;
        let rows =
            statement.query_map([self.limits.maximum_installs.saturating_add(1)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
        let builds = rows
            .map(|row| {
                let (author, d_tag, aggregate_hash, title, metadata) = row?;
                validate_install_title(&title, self.limits.maximum_install_title_bytes)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                Ok(InstalledBuild {
                    principal: Principal::new(author, d_tag, aggregate_hash)
                        .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                    title: Arc::from(title),
                    manifest_metadata: BoundedJson::from_raw(
                        metadata,
                        self.limits.maximum_value_bytes,
                    )
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        if builds.len() > self.limits.maximum_installs {
            return Err(StoreError::Corrupt(format!(
                "installation count exceeds {}",
                self.limits.maximum_installs
            )));
        }
        Ok(builds)
    }

    pub fn set_grant(
        &self,
        principal: &Principal,
        capability: &Capability,
        decision: GrantDecision,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock();
        let exists: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM grants WHERE
                author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3 AND capability = ?4
            )",
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                capability.as_str()
            ],
            |row| row.get(0),
        )?;
        if !exists {
            let count: usize = connection.query_row(
                "SELECT COUNT(*) FROM grants WHERE
                 author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
                principal_params(principal),
                |row| row.get(0),
            )?;
            if count >= self.limits.maximum_grants_per_principal {
                return Err(StoreError::GrantCapacity {
                    capacity: self.limits.maximum_grants_per_principal,
                });
            }
        }
        connection.execute(
            "INSERT INTO grants
                (author, d_tag, aggregate_hash, capability, decision)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(author, d_tag, aggregate_hash, capability) DO UPDATE SET
                decision = excluded.decision",
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                capability.as_str(),
                grant_decision_text(decision),
            ],
        )?;
        Ok(())
    }

    pub fn grant(
        &self,
        principal: &Principal,
        capability: &Capability,
    ) -> Result<GrantDecision, StoreError> {
        let connection = self.connection.lock();
        let decision = connection
            .query_row(
                "SELECT decision FROM grants WHERE
                 author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3 AND capability = ?4",
                params![
                    principal.manifest_author(),
                    principal.d_tag(),
                    principal.aggregate_hash(),
                    capability.as_str()
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        decision.map_or(Ok(GrantDecision::Denied), |value| {
            parse_grant_decision(&value)
        })
    }

    pub fn put_component_value(
        &self,
        principal: &Principal,
        domain: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), StoreError> {
        validate_scope_name("domain", domain)?;
        validate_scope_name("key", key)?;
        if value.len() > self.limits.maximum_value_bytes {
            return Err(StoreError::ValueTooLarge {
                actual: value.len(),
                maximum: self.limits.maximum_value_bytes,
            });
        }

        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing_bytes: Option<usize> = transaction
            .query_row(
                "SELECT length(value) FROM component_kv WHERE
                 author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
                 AND domain = ?4 AND key = ?5",
                params![
                    principal.manifest_author(),
                    principal.d_tag(),
                    principal.aggregate_hash(),
                    domain,
                    key
                ],
                |row| row.get(0),
            )
            .optional()?;
        let (count, total): (usize, usize) = transaction.query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(value)), 0)
             FROM component_kv WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3 AND domain = ?4",
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                domain
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if existing_bytes.is_none() && count >= self.limits.maximum_kv_keys_per_scope {
            return Err(StoreError::KeyCapacity {
                capacity: self.limits.maximum_kv_keys_per_scope,
            });
        }
        let next_total = total
            .saturating_sub(existing_bytes.unwrap_or_default())
            .saturating_add(value.len());
        if next_total > self.limits.maximum_kv_bytes_per_scope {
            return Err(StoreError::ScopeBytes {
                actual: next_total,
                maximum: self.limits.maximum_kv_bytes_per_scope,
            });
        }
        transaction.execute(
            "INSERT INTO component_kv
                (author, d_tag, aggregate_hash, domain, key, value)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(author, d_tag, aggregate_hash, domain, key) DO UPDATE SET
                value = excluded.value",
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                domain,
                key,
                value
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn component_value(
        &self,
        principal: &Principal,
        domain: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        validate_scope_name("domain", domain)?;
        validate_scope_name("key", key)?;
        self.connection
            .lock()
            .query_row(
                "SELECT value FROM component_kv WHERE
                 author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
                 AND domain = ?4 AND key = ?5",
                params![
                    principal.manifest_author(),
                    principal.d_tag(),
                    principal.aggregate_hash(),
                    domain,
                    key
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn remove_component_value(
        &self,
        principal: &Principal,
        domain: &str,
        key: &str,
    ) -> Result<bool, StoreError> {
        validate_scope_name("domain", domain)?;
        validate_scope_name("key", key)?;
        let removed = self.connection.lock().execute(
            "DELETE FROM component_kv WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
             AND domain = ?4 AND key = ?5",
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                domain,
                key
            ],
        )?;
        Ok(removed == 1)
    }

    /// Return an exact scope's keys in deterministic byte order.
    ///
    /// `maximum` is a caller-selected response bound and may not exceed the
    /// store's structural per-scope key limit. The query reads at most one
    /// sentinel row beyond that bound so overflow is observable rather than
    /// silently truncated.
    pub fn component_keys(
        &self,
        principal: &Principal,
        domain: &str,
        maximum: usize,
    ) -> Result<Vec<String>, StoreError> {
        validate_scope_name("domain", domain)?;
        if maximum == 0 || maximum > self.limits.maximum_kv_keys_per_scope {
            return Err(StoreError::InvalidKeyListLimit {
                requested: maximum,
                maximum: self.limits.maximum_kv_keys_per_scope,
            });
        }
        let query_limit = maximum.saturating_add(1);
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT key FROM component_kv WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3 AND domain = ?4
             ORDER BY key LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
                domain,
                query_limit
            ],
            |row| row.get::<_, String>(0),
        )?;
        let keys = rows.collect::<Result<Vec<_>, _>>()?;
        if keys.len() > maximum {
            return Err(StoreError::KeyListCapacity {
                actual_at_least: keys.len(),
                maximum,
            });
        }
        Ok(keys)
    }

    pub fn save_workspace(&self, workspace: &WorkspaceRecord) -> Result<(), StoreError> {
        if workspace.definition.byte_len() > self.limits.maximum_workspace_bytes {
            return Err(StoreError::WorkspaceTooLarge {
                actual: workspace.definition.byte_len(),
                maximum: self.limits.maximum_workspace_bytes,
            });
        }
        validate_scope_name("workspace id", &workspace.id)?;
        let receipts = encode_retained_receipts(&workspace.retained_receipts, self.limits)?;
        let connection = self.connection.lock();
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
            [workspace.id.as_ref()],
            |row| row.get(0),
        )?;
        if !exists {
            let count: usize =
                connection.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
            if count >= self.limits.maximum_workspaces {
                return Err(StoreError::WorkspaceCapacity {
                    capacity: self.limits.maximum_workspaces,
                });
            }
        }
        connection.execute(
            "INSERT INTO workspaces (id, definition, retained_receipts)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                definition = excluded.definition,
                retained_receipts = excluded.retained_receipts",
            params![
                workspace.id.as_ref(),
                workspace.definition.as_str(),
                receipts
            ],
        )?;
        Ok(())
    }

    pub fn load_workspaces(&self) -> Result<Vec<WorkspaceRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id, definition, retained_receipts
             FROM workspaces ORDER BY id LIMIT ?1",
        )?;
        let rows =
            statement.query_map([self.limits.maximum_workspaces.saturating_add(1)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
        let workspaces = rows
            .map(|row| {
                let (id, definition, receipts) = row?;
                if receipts.len() > self.limits.maximum_retained_receipt_bytes_per_workspace {
                    return Err(StoreError::Corrupt(format!(
                        "workspace retained receipt bytes exceed {}",
                        self.limits.maximum_retained_receipt_bytes_per_workspace
                    )));
                }
                let retained_receipts: Vec<WriteReceiptId> = serde_json::from_str(&receipts)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                validate_retained_receipts(&retained_receipts, receipts.len(), self.limits)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                Ok(WorkspaceRecord {
                    id: Arc::from(id),
                    definition: BoundedJson::from_raw(
                        definition,
                        self.limits.maximum_workspace_bytes,
                    )
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                    retained_receipts,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        if workspaces.len() > self.limits.maximum_workspaces {
            return Err(StoreError::Corrupt(format!(
                "workspace count exceeds {}",
                self.limits.maximum_workspaces
            )));
        }
        Ok(workspaces)
    }

    pub fn append_activity(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        let record_bytes = validate_activity(record, self.limits)?;
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO activity
                (author, d_tag, aggregate_hash, category, operation, outcome, occurred_at_millis)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.principal.manifest_author(),
                record.principal.d_tag(),
                record.principal.aggregate_hash(),
                record.category.as_ref(),
                record.operation.as_ref(),
                record.outcome.as_ref(),
                record.occurred_at_millis,
            ],
        )?;
        transaction.execute(
            "DELETE FROM activity WHERE id IN (
                SELECT id FROM (
                    SELECT id,
                           ROW_NUMBER() OVER (ORDER BY id DESC) AS newest_rank,
                           SUM(length(category) + length(operation) + length(outcome))
                               OVER (ORDER BY id DESC ROWS BETWEEN
                                     UNBOUNDED PRECEDING AND CURRENT ROW) AS newest_bytes
                    FROM activity
                )
                WHERE newest_rank > ?1 OR newest_bytes > ?2
            )",
            params![
                self.limits.maximum_activity_facts,
                self.limits.maximum_activity_total_bytes,
            ],
        )?;
        debug_assert!(record_bytes <= self.limits.maximum_activity_total_bytes);
        transaction.commit()?;
        Ok(())
    }

    pub fn activity_records(&self) -> Result<Vec<ActivityRecord>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT author, d_tag, aggregate_hash, category, operation, outcome,
                    occurred_at_millis
             FROM activity ORDER BY id ASC LIMIT ?1",
        )?;
        let rows = statement.query_map(
            [self.limits.maximum_activity_facts.saturating_add(1)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u64>(6)?,
                ))
            },
        )?;
        let mut total_bytes = 0usize;
        let records = rows
            .map(|row| {
                let (author, d_tag, aggregate_hash, category, operation, outcome, occurred_at) =
                    row?;
                let record = ActivityRecord {
                    principal: Principal::new(author, d_tag, aggregate_hash)
                        .map_err(|error| StoreError::Corrupt(error.to_string()))?,
                    category: Arc::from(category),
                    operation: Arc::from(operation),
                    outcome: Arc::from(outcome),
                    occurred_at_millis: occurred_at,
                };
                let bytes = validate_activity(&record, self.limits)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
                total_bytes = total_bytes.saturating_add(bytes);
                Ok(record)
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        if records.len() > self.limits.maximum_activity_facts
            || total_bytes > self.limits.maximum_activity_total_bytes
        {
            return Err(StoreError::Corrupt(
                "activity retention bounds were exceeded".to_owned(),
            ));
        }
        Ok(records)
    }

    pub fn table_names(&self) -> Result<Vec<String>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

fn migrate(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_schema (
            version INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS installations (
            author TEXT NOT NULL,
            d_tag TEXT NOT NULL,
            aggregate_hash TEXT NOT NULL,
            title TEXT NOT NULL,
            manifest_metadata TEXT NOT NULL,
            PRIMARY KEY(author, d_tag, aggregate_hash)
        );
        CREATE TABLE IF NOT EXISTS grants (
            author TEXT NOT NULL,
            d_tag TEXT NOT NULL,
            aggregate_hash TEXT NOT NULL,
            capability TEXT NOT NULL,
            decision TEXT NOT NULL,
            PRIMARY KEY(author, d_tag, aggregate_hash, capability)
        );
        CREATE TABLE IF NOT EXISTS component_kv (
            author TEXT NOT NULL,
            d_tag TEXT NOT NULL,
            aggregate_hash TEXT NOT NULL,
            domain TEXT NOT NULL,
            key TEXT NOT NULL,
            value BLOB NOT NULL,
            PRIMARY KEY(author, d_tag, aggregate_hash, domain, key)
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY NOT NULL,
            definition TEXT NOT NULL,
            retained_receipts TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS activity (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            author TEXT NOT NULL,
            d_tag TEXT NOT NULL,
            aggregate_hash TEXT NOT NULL,
            category TEXT NOT NULL,
            operation TEXT NOT NULL,
            outcome TEXT NOT NULL,
            occurred_at_millis INTEGER NOT NULL
        );",
    )?;
    let existing: Option<i64> = connection
        .query_row("SELECT version FROM runtime_schema LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()?;
    match existing {
        None => {
            connection.execute(
                "INSERT INTO runtime_schema(version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }
        Some(SCHEMA_VERSION) => {}
        Some(version) => return Err(StoreError::UnsupportedSchema(version)),
    }
    Ok(())
}

fn validate_limits(limits: StoreLimits) -> Result<(), StoreError> {
    if [
        limits.maximum_installs,
        limits.maximum_install_title_bytes,
        limits.maximum_grants_per_principal,
        limits.maximum_kv_keys_per_scope,
        limits.maximum_kv_bytes_per_scope,
        limits.maximum_value_bytes,
        limits.maximum_workspaces,
        limits.maximum_workspace_bytes,
        limits.maximum_retained_receipts_per_workspace,
        limits.maximum_retained_receipt_bytes_per_workspace,
        limits.maximum_activity_facts,
        limits.maximum_activity_string_bytes,
        limits.maximum_activity_record_bytes,
        limits.maximum_activity_total_bytes,
    ]
    .contains(&0)
        || limits.maximum_activity_record_bytes > limits.maximum_activity_total_bytes
    {
        return Err(StoreError::InvalidLimits);
    }
    Ok(())
}

fn validate_install_title(title: &str, maximum: usize) -> Result<(), StoreError> {
    if title.is_empty()
        || title
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidInstallTitle);
    }
    if title.len() > maximum {
        return Err(StoreError::InstallTitleTooLarge {
            actual: title.len(),
            maximum,
        });
    }
    Ok(())
}

fn encode_retained_receipts(
    receipts: &[WriteReceiptId],
    limits: StoreLimits,
) -> Result<String, StoreError> {
    if receipts.len() > limits.maximum_retained_receipts_per_workspace {
        return Err(StoreError::RetainedReceiptCapacity {
            actual: receipts.len(),
            maximum: limits.maximum_retained_receipts_per_workspace,
        });
    }
    let encoded =
        serde_json::to_string(receipts).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    validate_retained_receipts(receipts, encoded.len(), limits)?;
    Ok(encoded)
}

fn validate_retained_receipts(
    receipts: &[WriteReceiptId],
    encoded_bytes: usize,
    limits: StoreLimits,
) -> Result<(), StoreError> {
    if receipts.len() > limits.maximum_retained_receipts_per_workspace {
        return Err(StoreError::RetainedReceiptCapacity {
            actual: receipts.len(),
            maximum: limits.maximum_retained_receipts_per_workspace,
        });
    }
    if encoded_bytes > limits.maximum_retained_receipt_bytes_per_workspace {
        return Err(StoreError::RetainedReceiptBytes {
            actual: encoded_bytes,
            maximum: limits.maximum_retained_receipt_bytes_per_workspace,
        });
    }
    Ok(())
}

fn validate_activity(record: &ActivityRecord, limits: StoreLimits) -> Result<usize, StoreError> {
    let fields = [
        ("category", record.category.as_ref()),
        ("operation", record.operation.as_ref()),
        ("outcome", record.outcome.as_ref()),
    ];
    for (field, value) in fields {
        if value.is_empty()
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err(StoreError::InvalidActivityString { field });
        }
        if value.len() > limits.maximum_activity_string_bytes {
            return Err(StoreError::ActivityStringTooLarge {
                field,
                actual: value.len(),
                maximum: limits.maximum_activity_string_bytes,
            });
        }
    }
    let bytes = fields.iter().fold(0usize, |total, (_, value)| {
        total.saturating_add(value.len())
    });
    let maximum = limits
        .maximum_activity_record_bytes
        .min(limits.maximum_activity_total_bytes);
    if bytes > maximum {
        return Err(StoreError::ActivityRecordTooLarge {
            actual: bytes,
            maximum,
        });
    }
    Ok(bytes)
}

fn validate_scope_name(field: &'static str, value: &str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidName { field });
    }
    Ok(())
}

fn principal_params(principal: &Principal) -> [&str; 3] {
    [
        principal.manifest_author(),
        principal.d_tag(),
        principal.aggregate_hash(),
    ]
}

fn grant_decision_text(decision: GrantDecision) -> &'static str {
    match decision {
        GrantDecision::Denied => "denied",
        GrantDecision::AskEveryTime => "ask_every_time",
        GrantDecision::AllowSession => "allow_session",
        GrantDecision::AllowExactBuild => "allow_exact_build",
        GrantDecision::Managed => "managed",
    }
}

fn parse_grant_decision(value: &str) -> Result<GrantDecision, StoreError> {
    match value {
        "denied" => Ok(GrantDecision::Denied),
        "ask_every_time" => Ok(GrantDecision::AskEveryTime),
        "allow_session" => Ok(GrantDecision::AllowSession),
        "allow_exact_build" => Ok(GrantDecision::AllowExactBuild),
        "managed" => Ok(GrantDecision::Managed),
        other => Err(StoreError::Corrupt(format!(
            "unknown grant decision {other}"
        ))),
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("runtime store limits must be finite and non-zero")]
    InvalidLimits,
    #[error("unsupported runtime store schema {0}")]
    UnsupportedSchema(i64),
    #[error("runtime store is corrupt: {0}")]
    Corrupt(String),
    #[error("invalid {field}")]
    InvalidName { field: &'static str },
    #[error("installation capacity {capacity} is full")]
    InstallCapacity { capacity: usize },
    #[error("installation title must be non-empty and contain no control characters")]
    InvalidInstallTitle,
    #[error("installation title is {actual} bytes; the maximum is {maximum}")]
    InstallTitleTooLarge { actual: usize, maximum: usize },
    #[error("manifest metadata is {actual} bytes; the maximum is {maximum}")]
    ManifestMetadataTooLarge { actual: usize, maximum: usize },
    #[error("grant capacity {capacity} is full for this exact principal")]
    GrantCapacity { capacity: usize },
    #[error("component value is {actual} bytes; the maximum is {maximum}")]
    ValueTooLarge { actual: usize, maximum: usize },
    #[error("component key capacity {capacity} is full for this exact scope")]
    KeyCapacity { capacity: usize },
    #[error("component key-list limit {requested} is invalid; it must be between 1 and {maximum}")]
    InvalidKeyListLimit { requested: usize, maximum: usize },
    #[error(
        "component scope has at least {actual_at_least} keys; the response maximum is {maximum}"
    )]
    KeyListCapacity {
        actual_at_least: usize,
        maximum: usize,
    },
    #[error("component scope would use {actual} bytes; the maximum is {maximum}")]
    ScopeBytes { actual: usize, maximum: usize },
    #[error("workspace capacity {capacity} is full")]
    WorkspaceCapacity { capacity: usize },
    #[error("workspace is {actual} bytes; the maximum is {maximum}")]
    WorkspaceTooLarge { actual: usize, maximum: usize },
    #[error("workspace retains {actual} receipts; the maximum is {maximum}")]
    RetainedReceiptCapacity { actual: usize, maximum: usize },
    #[error("workspace retained receipt references use {actual} bytes; the maximum is {maximum}")]
    RetainedReceiptBytes { actual: usize, maximum: usize },
    #[error("activity {field} must be non-empty and contain no control characters")]
    InvalidActivityString { field: &'static str },
    #[error("activity {field} is {actual} bytes; the maximum is {maximum}")]
    ActivityStringTooLarge {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    #[error("activity record strings use {actual} bytes; the maximum is {maximum}")]
    ActivityRecordTooLarge { actual: usize, maximum: usize },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn principal(hash: char) -> Principal {
        Principal::new("a".repeat(64), "app", hash.to_string().repeat(64)).unwrap()
    }

    fn store() -> (TempDir, RuntimeStore) {
        let directory = TempDir::new().unwrap();
        let store = RuntimeStore::open(directory.path().join("runtime.db"), StoreLimits::default())
            .unwrap();
        (directory, store)
    }

    fn workspace(receipts: Vec<WriteReceiptId>) -> WorkspaceRecord {
        WorkspaceRecord {
            id: Arc::from("main"),
            definition: BoundedJson::from_value(&serde_json::json!({"slots": ["feed"]}), 1024)
                .unwrap(),
            retained_receipts: receipts,
        }
    }

    fn activity(operation: &str, outcome: &str) -> ActivityRecord {
        ActivityRecord {
            principal: principal('b'),
            category: Arc::from("provider"),
            operation: Arc::from(operation),
            outcome: Arc::from(outcome),
            occurred_at_millis: 1,
        }
    }

    #[test]
    fn component_storage_isolated_by_build_hash() {
        let (_directory, store) = store();
        store
            .put_component_value(&principal('b'), "storage", "token", b"first")
            .unwrap();
        assert_eq!(
            store
                .component_value(&principal('c'), "storage", "token")
                .unwrap(),
            None
        );
    }

    #[test]
    fn component_key_listing_and_removal_are_exact_bounded_and_isolated() {
        let (_directory, store) = store();
        let first = principal('b');
        let second = principal('c');
        for key in ["z-last", "a-first"] {
            store
                .put_component_value(&first, "storage", key, key.as_bytes())
                .unwrap();
        }
        store
            .put_component_value(&second, "storage", "other", b"other")
            .unwrap();

        assert_eq!(
            store.component_keys(&first, "storage", 2).unwrap(),
            ["a-first", "z-last"]
        );
        assert!(matches!(
            store.component_keys(&first, "storage", 1),
            Err(StoreError::KeyListCapacity {
                actual_at_least: 2,
                maximum: 1
            })
        ));
        assert!(matches!(
            store.component_keys(&first, "storage", 0),
            Err(StoreError::InvalidKeyListLimit { requested: 0, .. })
        ));
        assert!(
            store
                .remove_component_value(&first, "storage", "a-first")
                .unwrap()
        );
        assert!(
            !store
                .remove_component_value(&first, "storage", "a-first")
                .unwrap()
        );
        assert_eq!(
            store.component_keys(&first, "storage", 2).unwrap(),
            ["z-last"]
        );
        assert_eq!(
            store.component_keys(&second, "storage", 2).unwrap(),
            ["other"]
        );
    }

    #[test]
    fn sensitive_grant_does_not_transfer_to_update() {
        let (_directory, store) = store();
        let upload = Capability::new("upload").unwrap();
        store
            .set_grant(&principal('b'), &upload, GrantDecision::AllowExactBuild)
            .unwrap();
        assert_eq!(
            store.grant(&principal('c'), &upload).unwrap(),
            GrantDecision::Denied
        );
    }

    #[test]
    fn restart_restores_workspace_and_receipt_reference() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("runtime.db");
        {
            let store = RuntimeStore::open(&path, StoreLimits::default()).unwrap();
            store
                .save_workspace(&workspace(vec![WriteReceiptId(Arc::from("receipt-1"))]))
                .unwrap();
        }
        let reopened = RuntimeStore::open(&path, StoreLimits::default()).unwrap();
        let workspaces = reopened.load_workspaces().unwrap();
        assert_eq!(workspaces[0].retained_receipts[0].0.as_ref(), "receipt-1");
    }

    #[test]
    fn installation_title_and_metadata_are_refused_before_persistence() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("runtime.db");
        let limits = StoreLimits {
            maximum_install_title_bytes: 4,
            maximum_value_bytes: 16,
            ..StoreLimits::default()
        };
        let store = RuntimeStore::open(&path, limits).unwrap();
        let metadata = BoundedJson::from_value(&serde_json::json!({}), 16).unwrap();

        assert!(matches!(
            store.install(&InstalledBuild {
                principal: principal('b'),
                title: Arc::from("large"),
                manifest_metadata: metadata.clone(),
            }),
            Err(StoreError::InstallTitleTooLarge {
                actual: 5,
                maximum: 4
            })
        ));
        assert!(store.installed_builds().unwrap().is_empty());

        store
            .install(&InstalledBuild {
                principal: principal('b'),
                title: Arc::from("four"),
                manifest_metadata: metadata,
            })
            .unwrap();
        drop(store);
        let reopened = RuntimeStore::open(&path, limits).unwrap();
        assert_eq!(
            reopened.installed_builds().unwrap()[0].title.as_ref(),
            "four"
        );
    }

    #[test]
    fn retained_receipt_count_and_bytes_are_typed_refusals_and_survive_restart() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("runtime.db");
        let limits = StoreLimits {
            maximum_retained_receipts_per_workspace: 1,
            maximum_retained_receipt_bytes_per_workspace: 20,
            ..StoreLimits::default()
        };
        let store = RuntimeStore::open(&path, limits).unwrap();

        assert!(matches!(
            store.save_workspace(&workspace(vec![
                WriteReceiptId(Arc::from("one")),
                WriteReceiptId(Arc::from("two")),
            ])),
            Err(StoreError::RetainedReceiptCapacity {
                actual: 2,
                maximum: 1
            })
        ));
        assert!(matches!(
            store.save_workspace(&workspace(vec![WriteReceiptId(Arc::from(
                "receipt-id-that-does-not-fit"
            ))])),
            Err(StoreError::RetainedReceiptBytes {
                actual,
                maximum: 20
            }) if actual > 20
        ));
        assert!(store.load_workspaces().unwrap().is_empty());

        store
            .save_workspace(&workspace(vec![WriteReceiptId(Arc::from("receipt"))]))
            .unwrap();
        drop(store);
        let reopened = RuntimeStore::open(&path, limits).unwrap();
        assert_eq!(
            reopened.load_workspaces().unwrap()[0].retained_receipts,
            vec![WriteReceiptId(Arc::from("receipt"))]
        );
    }

    #[test]
    fn activity_strings_and_records_are_refused_and_retention_is_count_and_byte_bounded() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("runtime.db");
        let limits = StoreLimits {
            maximum_activity_facts: 2,
            maximum_activity_string_bytes: 8,
            maximum_activity_record_bytes: 16,
            maximum_activity_total_bytes: 28,
            ..StoreLimits::default()
        };
        let store = RuntimeStore::open(&path, limits).unwrap();

        assert!(matches!(
            store.append_activity(&activity("operation", "ok")),
            Err(StoreError::ActivityStringTooLarge {
                field: "operation",
                actual: 9,
                maximum: 8
            })
        ));
        let mut aggregate_too_large = activity("12345678", "12345678");
        aggregate_too_large.category = Arc::from("p");
        assert!(matches!(
            store.append_activity(&aggregate_too_large),
            Err(StoreError::ActivityRecordTooLarge {
                actual: 17,
                maximum: 16
            })
        ));

        store.append_activity(&activity("one", "ok")).unwrap();
        store.append_activity(&activity("two", "ok")).unwrap();
        store.append_activity(&activity("three", "ok")).unwrap();
        assert_eq!(
            store
                .activity_records()
                .unwrap()
                .iter()
                .map(|record| record.operation.as_ref())
                .collect::<Vec<_>>(),
            vec!["two", "three"]
        );

        drop(store);
        let reopened = RuntimeStore::open(&path, limits).unwrap();
        let records = reopened.activity_records().unwrap();
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .map(|record| {
                    record.category.len() + record.operation.len() + record.outcome.len()
                })
                .sum::<usize>()
                <= limits.maximum_activity_total_bytes
        );
    }

    #[test]
    fn schema_contains_no_parallel_nostr_truth() {
        let (_directory, store) = store();
        let names = store.table_names().unwrap();
        for forbidden in [
            "events",
            "replacements",
            "deletions",
            "pending_rows",
            "receipt_facts",
            "relay_routes",
        ] {
            assert!(!names.iter().any(|name| name == forbidden));
        }
    }
}
