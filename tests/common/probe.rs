//! 벡터 저장소 호출을 세는 래퍼와 LanceDB 버전 수. (FR-STO-7, FR-STO-12)

use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hippo::store::RecordKind;
use hippo::vector::{StoreInfo, VectorError, VectorFilter, VectorHit, VectorRecord, VectorStore};

/// 호출 수와, 쓰기 직후 내부 저장소의 `pending_changes` 최댓값을 기록한다.
pub struct Probe {
    pub inner: Box<dyn VectorStore>,
    pub replaces: AtomicUsize,
    pub other_writes: AtomicUsize,
    pub max_pending: AtomicU64,
}

impl Probe {
    pub fn new(inner: Box<dyn VectorStore>) -> Self {
        Self {
            inner,
            replaces: AtomicUsize::new(0),
            other_writes: AtomicUsize::new(0),
            max_pending: AtomicU64::new(0),
        }
    }
    fn observe(&self) {
        self.max_pending
            .fetch_max(self.inner.pending_changes(), Ordering::SeqCst);
    }
}

impl VectorStore for Probe {
    fn info(&self) -> Result<StoreInfo, VectorError> {
        self.inner.info()
    }
    fn upsert(&self, records: &[VectorRecord]) -> Result<(), VectorError> {
        self.other_writes.fetch_add(1, Ordering::SeqCst);
        self.inner.upsert(records)?;
        self.observe();
        Ok(())
    }
    fn replace(
        &self,
        refs: &[(RecordKind, String)],
        records: &[VectorRecord],
    ) -> Result<(), VectorError> {
        self.replaces.fetch_add(1, Ordering::SeqCst);
        self.inner.replace(refs, records)?;
        self.observe();
        Ok(())
    }
    fn delete(&self, kind: RecordKind, ref_id: &str) -> Result<(), VectorError> {
        self.other_writes.fetch_add(1, Ordering::SeqCst);
        self.inner.delete(kind, ref_id)?;
        self.observe();
        Ok(())
    }
    fn search(
        &self,
        vector: &[f32],
        filter: &VectorFilter,
        limit: usize,
    ) -> Result<Vec<VectorHit>, VectorError> {
        self.inner.search(vector, filter, limit)
    }
    fn count(&self) -> Result<u64, VectorError> {
        self.inner.count()
    }
    fn ensure_index(&self) -> Result<(), VectorError> {
        self.inner.ensure_index()
    }
    fn optimize(&self) -> Result<(), VectorError> {
        self.inner.optimize()
    }
    fn pending_changes(&self) -> u64 {
        self.inner.pending_changes()
    }
    fn scan(&self) -> Result<Vec<VectorRecord>, VectorError> {
        self.inner.scan()
    }
}

/// LanceDB 저장소 디렉터리에 남아 있는 버전(매니페스트) 수.
pub fn lance_versions(dir: &Path) -> usize {
    std::fs::read_dir(dir.join("vectors.lance").join("_versions"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "manifest"))
        })
        .count()
}
