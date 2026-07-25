//! Persistent runtime metadata separate from NMP canonical state.
//!
//! This schema intentionally has no Nostr event, replacement, deletion,
//! routing, pending-row, or receipt-fact tables. Receipt identifiers are kept
//! only as workspace recovery references for NMP reattachment.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nmp_native_runtime_core::{
    BoundedJson, Capability, CapabilityRequest, GrantDecision, Principal, WriteReceiptId,
};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreLimits {
    pub maximum_installs: usize,
    pub maximum_install_title_bytes: usize,
    pub maximum_install_search_query_bytes: usize,
    pub maximum_grants_per_principal: usize,
    pub maximum_kv_keys_per_scope: usize,
    pub maximum_kv_bytes_per_scope: usize,
    pub maximum_value_bytes: usize,
    pub maximum_workspaces: usize,
    pub maximum_workspace_bytes: usize,
    pub maximum_workspace_assignments: usize,
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
            maximum_install_search_query_bytes: 256,
            maximum_grants_per_principal: 64,
            maximum_kv_keys_per_scope: 1_024,
            maximum_kv_bytes_per_scope: 8 * 1024 * 1024,
            maximum_value_bytes: 512 * 1024,
            maximum_workspaces: 64,
            maximum_workspace_bytes: 512 * 1024,
            maximum_workspace_assignments: 512,
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
    pub capability_requests: Vec<CapabilityRequest>,
}

/// The exact runtime-owned state removed when one installed build is
/// uninstalled.
///
/// This policy deliberately excludes activity evidence, workspace definitions
/// and retained NMP receipt identifiers. It also cannot delete sealed artifact
/// bytes: those belong to the artifact resolver/cache, which must expose its
/// own exact-build deletion API before the application kernel can coordinate
/// that cleanup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UninstallCleanupPolicy {
    RuntimeOwnedExactBuildState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UninstallReport {
    pub installation_removed: bool,
    pub grants_removed: usize,
    pub component_values_removed: usize,
    pub workspace_assignments_removed: usize,
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
        let capability_requests = encode_capability_requests(
            &build.capability_requests,
            self.limits.maximum_grants_per_principal,
        )?;
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
                (author, d_tag, aggregate_hash, title, manifest_metadata, capability_requests)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(author, d_tag, aggregate_hash) DO UPDATE SET
                title = excluded.title,
                manifest_metadata = excluded.manifest_metadata,
                capability_requests = excluded.capability_requests",
            params![
                build.principal.manifest_author(),
                build.principal.d_tag(),
                build.principal.aggregate_hash(),
                build.title.as_ref(),
                build.manifest_metadata.as_str(),
                capability_requests,
            ],
        )?;
        Ok(())
    }

    pub fn installed_builds(&self) -> Result<Vec<InstalledBuild>, StoreError> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT author, d_tag, aggregate_hash, title, manifest_metadata, capability_requests
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
                    row.get::<_, String>(5)?,
                ))
            })?;
        let builds = rows
            .map(|row| {
                let (author, d_tag, aggregate_hash, title, metadata, capability_requests) = row?;
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
                    capability_requests: decode_capability_requests(
                        &capability_requests,
                        self.limits.maximum_grants_per_principal,
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

    /// Search verified installed-build metadata with explicit input and output
    /// bounds.
    ///
    /// Search covers the exact coordinate and the verified display title. The
    /// opaque manifest JSON remains available in each result, but is not
    /// interpreted as another metadata schema.
    pub fn search_installed_builds(
        &self,
        query: &str,
        maximum: usize,
    ) -> Result<Vec<InstalledBuild>, StoreError> {
        validate_install_search_query(query, self.limits.maximum_install_search_query_bytes)?;
        if maximum == 0 || maximum > self.limits.maximum_installs {
            return Err(StoreError::InvalidInstallSearchLimit {
                requested: maximum,
                maximum: self.limits.maximum_installs,
            });
        }
        let mut matches = self
            .installed_builds()?
            .into_iter()
            .filter(|build| {
                query.is_empty()
                    || [
                        build.title.as_ref(),
                        build.principal.manifest_author(),
                        build.principal.d_tag(),
                        build.principal.aggregate_hash(),
                    ]
                    .iter()
                    .any(|value| contains_search(value, query))
            })
            .take(maximum.saturating_add(1))
            .collect::<Vec<_>>();
        if matches.len() > maximum {
            return Err(StoreError::InstallSearchCapacity {
                actual_at_least: matches.len(),
                maximum,
            });
        }
        matches.shrink_to_fit();
        Ok(matches)
    }

    /// Atomically remove one exact build's runtime-owned persistent state.
    ///
    /// No NMP store is reachable from this type, so canonical events, pending
    /// writes and receipts cannot be deleted by this operation.
    pub fn uninstall_exact_build(
        &self,
        principal: &Principal,
        policy: UninstallCleanupPolicy,
    ) -> Result<UninstallReport, StoreError> {
        match policy {
            UninstallCleanupPolicy::RuntimeOwnedExactBuildState => {}
        }
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let grants_removed = transaction.execute(
            "DELETE FROM grants WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
            principal_params(principal),
        )?;
        let component_values_removed = transaction.execute(
            "DELETE FROM component_kv WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
            principal_params(principal),
        )?;
        let workspace_assignments_removed = transaction.execute(
            "DELETE FROM workspace_assignments WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
            principal_params(principal),
        )?;
        let installation_removed = transaction.execute(
            "DELETE FROM installations WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
            principal_params(principal),
        )? == 1;
        transaction.commit()?;
        Ok(UninstallReport {
            installation_removed,
            grants_removed,
            component_values_removed,
            workspace_assignments_removed,
        })
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

    /// Atomically replaces the named persistent grant rows for one exact
    /// principal. Session-only grants deliberately delete a durable row so a
    /// prior exact-build allowance cannot reappear after restart.
    pub fn set_grants_atomic(
        &self,
        principal: &Principal,
        decisions: &[(Capability, GrantDecision)],
    ) -> Result<(), StoreError> {
        if decisions.is_empty() {
            return Err(StoreError::EmptyGrantBatch);
        }
        let unique = decisions
            .iter()
            .map(|(capability, _)| capability)
            .collect::<std::collections::BTreeSet<_>>();
        if unique.len() != decisions.len() {
            return Err(StoreError::DuplicateGrantBatchCapability);
        }
        if decisions.len() > self.limits.maximum_grants_per_principal {
            return Err(StoreError::GrantCapacity {
                capacity: self.limits.maximum_grants_per_principal,
            });
        }

        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let installed: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM installations
                WHERE author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
            )",
            principal_params(principal),
            |row| row.get(0),
        )?;
        if !installed {
            return Err(StoreError::InstallationNotFound);
        }
        let mut statement = transaction.prepare(
            "SELECT capability FROM grants WHERE
             author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3",
        )?;
        let rows =
            statement.query_map(principal_params(principal), |row| row.get::<_, String>(0))?;
        let mut persistent = rows.collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        drop(statement);
        for (capability, decision) in decisions {
            if *decision == GrantDecision::AllowSession {
                persistent.remove(capability.as_str());
            } else {
                persistent.insert(capability.as_str().to_owned());
            }
        }
        if persistent.len() > self.limits.maximum_grants_per_principal {
            return Err(StoreError::GrantCapacity {
                capacity: self.limits.maximum_grants_per_principal,
            });
        }

        for (capability, decision) in decisions {
            if *decision == GrantDecision::AllowSession {
                transaction.execute(
                    "DELETE FROM grants WHERE
                     author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3 AND capability = ?4",
                    params![
                        principal.manifest_author(),
                        principal.d_tag(),
                        principal.aggregate_hash(),
                        capability.as_str(),
                    ],
                )?;
            } else {
                transaction.execute(
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
                        grant_decision_text(*decision),
                    ],
                )?;
            }
        }
        transaction.commit()?;
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

    pub fn assign_build_to_workspace(
        &self,
        workspace_id: &str,
        principal: &Principal,
    ) -> Result<(), StoreError> {
        validate_scope_name("workspace id", workspace_id)?;
        let connection = self.connection.lock();
        let workspace_exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
            [workspace_id],
            |row| row.get(0),
        )?;
        if !workspace_exists {
            return Err(StoreError::WorkspaceNotFound);
        }
        let installation_exists: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM installations
                WHERE author = ?1 AND d_tag = ?2 AND aggregate_hash = ?3
            )",
            principal_params(principal),
            |row| row.get(0),
        )?;
        if !installation_exists {
            return Err(StoreError::InstallationNotFound);
        }
        let exists: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM workspace_assignments
                WHERE workspace_id = ?1
                  AND author = ?2 AND d_tag = ?3 AND aggregate_hash = ?4
            )",
            params![
                workspace_id,
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
            ],
            |row| row.get(0),
        )?;
        if !exists {
            let count: usize = connection.query_row(
                "SELECT COUNT(*) FROM workspace_assignments WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get(0),
            )?;
            if count >= self.limits.maximum_workspace_assignments {
                return Err(StoreError::WorkspaceAssignmentCapacity {
                    capacity: self.limits.maximum_workspace_assignments,
                });
            }
        }
        connection.execute(
            "INSERT OR IGNORE INTO workspace_assignments
                (workspace_id, author, d_tag, aggregate_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                workspace_id,
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
            ],
        )?;
        Ok(())
    }

    pub fn remove_build_from_workspace(
        &self,
        workspace_id: &str,
        principal: &Principal,
    ) -> Result<bool, StoreError> {
        validate_scope_name("workspace id", workspace_id)?;
        Ok(self.connection.lock().execute(
            "DELETE FROM workspace_assignments
             WHERE workspace_id = ?1
               AND author = ?2 AND d_tag = ?3 AND aggregate_hash = ?4",
            params![
                workspace_id,
                principal.manifest_author(),
                principal.d_tag(),
                principal.aggregate_hash(),
            ],
        )? == 1)
    }

    pub fn workspace_assignments(&self, workspace_id: &str) -> Result<Vec<Principal>, StoreError> {
        validate_scope_name("workspace id", workspace_id)?;
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT author, d_tag, aggregate_hash
             FROM workspace_assignments
             WHERE workspace_id = ?1
             ORDER BY author, d_tag, aggregate_hash
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![
                workspace_id,
                self.limits.maximum_workspace_assignments.saturating_add(1)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        let assignments = rows
            .map(|row| {
                let (author, d_tag, aggregate_hash) = row?;
                Principal::new(author, d_tag, aggregate_hash)
                    .map_err(|error| StoreError::Corrupt(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if assignments.len() > self.limits.maximum_workspace_assignments {
            return Err(StoreError::Corrupt(format!(
                "workspace assignment count exceeds {}",
                self.limits.maximum_workspace_assignments
            )));
        }
        Ok(assignments)
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
            capability_requests TEXT NOT NULL DEFAULT '[]',
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
            create_workspace_assignments(connection)?;
            add_capability_requests_column(connection)?;
            connection.execute(
                "INSERT INTO runtime_schema(version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }
        Some(1) => {
            create_workspace_assignments(connection)?;
            add_capability_requests_column(connection)?;
            connection.execute("UPDATE runtime_schema SET version = ?1", [SCHEMA_VERSION])?;
        }
        Some(2) => {
            add_capability_requests_column(connection)?;
            connection.execute("UPDATE runtime_schema SET version = ?1", [SCHEMA_VERSION])?;
        }
        Some(SCHEMA_VERSION) => {}
        Some(version) => return Err(StoreError::UnsupportedSchema(version)),
    }
    Ok(())
}

fn add_capability_requests_column(connection: &Connection) -> Result<(), StoreError> {
    let present: bool = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('installations')
            WHERE name = 'capability_requests'
        )",
        [],
        |row| row.get(0),
    )?;
    if !present {
        connection.execute(
            "ALTER TABLE installations
             ADD COLUMN capability_requests TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    Ok(())
}

fn create_workspace_assignments(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_assignments (
            workspace_id TEXT NOT NULL,
            author TEXT NOT NULL,
            d_tag TEXT NOT NULL,
            aggregate_hash TEXT NOT NULL,
            PRIMARY KEY(workspace_id, author, d_tag, aggregate_hash),
            FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE,
            FOREIGN KEY(author, d_tag, aggregate_hash)
                REFERENCES installations(author, d_tag, aggregate_hash) ON DELETE CASCADE
        );",
    )?;
    Ok(())
}

fn validate_limits(limits: StoreLimits) -> Result<(), StoreError> {
    if [
        limits.maximum_installs,
        limits.maximum_install_title_bytes,
        limits.maximum_install_search_query_bytes,
        limits.maximum_grants_per_principal,
        limits.maximum_kv_keys_per_scope,
        limits.maximum_kv_bytes_per_scope,
        limits.maximum_value_bytes,
        limits.maximum_workspaces,
        limits.maximum_workspace_bytes,
        limits.maximum_workspace_assignments,
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

fn validate_install_search_query(query: &str, maximum: usize) -> Result<(), StoreError> {
    if query
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(StoreError::InvalidInstallSearchQuery);
    }
    if query.len() > maximum {
        return Err(StoreError::InstallSearchQueryTooLarge {
            actual: query.len(),
            maximum,
        });
    }
    Ok(())
}

fn encode_capability_requests(
    requests: &[CapabilityRequest],
    maximum: usize,
) -> Result<String, StoreError> {
    validate_capability_requests(requests, maximum)?;
    serde_json::to_string(requests).map_err(|error| StoreError::Corrupt(error.to_string()))
}

fn decode_capability_requests(
    encoded: &str,
    maximum: usize,
) -> Result<Vec<CapabilityRequest>, StoreError> {
    let requests: Vec<CapabilityRequest> =
        serde_json::from_str(encoded).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    validate_capability_requests(&requests, maximum)?;
    Ok(requests)
}

fn validate_capability_requests(
    requests: &[CapabilityRequest],
    maximum: usize,
) -> Result<(), StoreError> {
    if requests.len() > maximum {
        return Err(StoreError::CapabilityRequestCapacity {
            actual: requests.len(),
            maximum,
        });
    }
    let unique = requests
        .iter()
        .map(|request| &request.capability)
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != requests.len() {
        return Err(StoreError::DuplicateCapabilityRequest);
    }
    Ok(())
}

fn contains_search(value: &str, query: &str) -> bool {
    query.is_empty()
        || value
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()))
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
    #[error("installation search query contains a control character")]
    InvalidInstallSearchQuery,
    #[error("installation search query is {actual} bytes; the maximum is {maximum}")]
    InstallSearchQueryTooLarge { actual: usize, maximum: usize },
    #[error("installation search limit {requested} is invalid; it must be between 1 and {maximum}")]
    InvalidInstallSearchLimit { requested: usize, maximum: usize },
    #[error(
        "installation search has at least {actual_at_least} results; the response maximum is {maximum}"
    )]
    InstallSearchCapacity {
        actual_at_least: usize,
        maximum: usize,
    },
    #[error("manifest metadata is {actual} bytes; the maximum is {maximum}")]
    ManifestMetadataTooLarge { actual: usize, maximum: usize },
    #[error("installation was not found")]
    InstallationNotFound,
    #[error("capability request count {actual} exceeds the maximum {maximum}")]
    CapabilityRequestCapacity { actual: usize, maximum: usize },
    #[error("installed capability requests repeat a domain")]
    DuplicateCapabilityRequest,
    #[error("grant capacity {capacity} is full for this exact principal")]
    GrantCapacity { capacity: usize },
    #[error("grant decision batch must not be empty")]
    EmptyGrantBatch,
    #[error("grant decision batch repeats a capability")]
    DuplicateGrantBatchCapability,
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
    #[error("workspace was not found")]
    WorkspaceNotFound,
    #[error("workspace assignment capacity {capacity} is full")]
    WorkspaceAssignmentCapacity { capacity: usize },
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
mod tests;
