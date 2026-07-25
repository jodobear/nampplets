use std::sync::Arc;

use nmp_native_runtime_core::{BoundedJson, Principal};
use rusqlite::params;

use crate::{
    InstalledBuild, RuntimeStore, StoreError, UninstallCleanupPolicy, UninstallReport,
    validate::{
        contains_search, decode_capability_requests, encode_capability_requests, principal_params,
        validate_install_search_query, validate_install_title,
    },
};

impl RuntimeStore {
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
}
