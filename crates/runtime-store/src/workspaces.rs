use std::sync::Arc;

use nmp_native_runtime_core::{BoundedJson, Principal, WriteReceiptId};
use rusqlite::params;

use crate::{
    RuntimeStore, StoreError, WorkspaceRecord,
    validate::{
        encode_retained_receipts, principal_params, validate_retained_receipts, validate_scope_name,
    },
};

impl RuntimeStore {
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
}
