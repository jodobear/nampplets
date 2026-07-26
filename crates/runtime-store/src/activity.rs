use std::sync::Arc;

use nmp_native_runtime_core::Principal;
use rusqlite::params;

use crate::{ActivityRecord, RuntimeStore, StoreError, validate::validate_activity};

impl RuntimeStore {
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
}
