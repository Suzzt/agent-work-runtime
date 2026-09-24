use crate::Store;
use awr_core::{Error, Result};
use std::{
    fs::{File, OpenOptions},
    time::{Duration, Instant},
};

pub struct SourceLock {
    database: String,
    _file: Option<File>,
}
impl Store {
    fn database_path(&self) -> Result<String> {
        self.conn
            .query_row(
                "SELECT file FROM pragma_database_list WHERE name='main'",
                [],
                |r| r.get(0),
            )
            .map_err(crate::db_error)
    }
    /// Serialize source/configuration transitions across processes, with a bounded wait.
    ///
    /// Lock order (WS-023 / SQLite single-writer compatibility): acquire this
    /// source lock **before** opening a SQLite `BEGIN IMMEDIATE` writer
    /// transaction that also needs source consistency (see `runtime_transaction`).
    /// Reversing that order across processes deadlocks: one holds IMMEDIATE and
    /// waits for the file lock while the other holds the file lock and waits for
    /// the writer lock.
    pub fn lock_sources(&self) -> Result<SourceLock> {
        let database = self.database_path()?;
        if database.is_empty() {
            return Ok(SourceLock {
                database,
                _file: None,
            });
        }
        let path = format!("{database}-sources.lock");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
            return Err(Error::RuleViolation(
                "source lock must be a regular file".into(),
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => {
                    return Ok(SourceLock {
                        database,
                        _file: Some(file),
                    });
                }
                Err(std::fs::TryLockError::WouldBlock)
                    if start.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(Error::SourceConflict(
                        "source transition is busy; retry after the current operation".into(),
                    ));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    }
    pub fn check_source_lock(&self, guard: &SourceLock) -> Result<()> {
        if self.database_path()? == guard.database {
            Ok(())
        } else {
            Err(Error::RuleViolation(
                "source lock belongs to another database".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::TransactionBehavior;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    fn temp_db() -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("awr-src-ord-{}", awr_core::Id::new()));
        std::fs::create_dir(&root).unwrap();
        let db = root.join("state.db");
        let mut store = Store::open(&db).unwrap();
        store.register_project(&root, "example", "Example").unwrap();
        drop(store);
        (root, db)
    }

    fn lock_sources_retry(store: &Store) -> SourceLock {
        let start = Instant::now();
        loop {
            match store.lock_sources() {
                Ok(guard) => return guard,
                Err(awr_core::Error::SourceConflict(_))
                    if start.elapsed() < Duration::from_secs(8) =>
                {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("unexpected source lock error: {error}"),
            }
        }
    }

    #[test]
    fn source_lock_then_immediate_serializes_without_deadlock() {
        let (root, db) = temp_db();
        let barrier = Arc::new(Barrier::new(2));
        let db_a = db.clone();
        let db_b = db;
        let b1 = barrier.clone();
        let b2 = barrier;
        let left = thread::spawn(move || {
            let mut store = Store::open(&db_a).unwrap();
            b1.wait();
            let guard = lock_sources_retry(&store);
            let tx = store
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            thread::sleep(Duration::from_millis(30));
            tx.commit().unwrap();
            drop(guard);
        });
        let right = thread::spawn(move || {
            let mut store = Store::open(&db_b).unwrap();
            b2.wait();
            let guard = lock_sources_retry(&store);
            let tx = store
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            thread::sleep(Duration::from_millis(30));
            tx.commit().unwrap();
            drop(guard);
        });
        let start = Instant::now();
        left.join().unwrap();
        right.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(8));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reverse_immediate_then_source_lock_contends() {
        let (root, db) = temp_db();
        let barrier = Arc::new(Barrier::new(2));
        let db_a = db.clone();
        let db_b = db;
        let b1 = barrier.clone();
        let b2 = barrier;
        let left = thread::spawn(move || {
            let mut writer = Store::open(&db_a).unwrap();
            let tx = writer
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            b1.wait();
            let locker = Store::open(&db_a).unwrap();
            let result = locker.lock_sources();
            drop(tx);
            result
        });
        let right = thread::spawn(move || {
            let mut store = Store::open(&db_b).unwrap();
            let guard = store.lock_sources().unwrap();
            b2.wait();
            let started = Instant::now();
            let outcome = store
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate);
            let elapsed = started.elapsed();
            if let Ok(tx) = outcome {
                tx.rollback().ok();
            }
            drop(guard);
            elapsed
        });
        let left_result = left.join().unwrap();
        let right_elapsed = right.join().unwrap();
        assert!(
            left_result.is_err() || right_elapsed > Duration::from_millis(15),
            "reverse order must contend: left_err={} right={right_elapsed:?}",
            left_result.is_err()
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
