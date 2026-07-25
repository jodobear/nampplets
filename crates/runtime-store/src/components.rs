use nmp_native_runtime_core::Principal;
use rusqlite::{OptionalExtension, params};

use crate::{RuntimeStore, StoreError, validate::validate_scope_name};

impl RuntimeStore {
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
}
