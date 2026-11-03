use hippo::queue::{EmbedQueue, Lane, LaneQueue};
use hippo::store::{EmbedJob, RecordKind};

fn job(id: &str) -> EmbedJob {
    EmbedJob {
        kind: RecordKind::Iteration,
        ref_id: id.into(),
    }
}

fn refs(batch: Option<(String, Vec<EmbedJob>)>) -> Option<(String, Vec<String>)> {
    batch.map(|(p, jobs)| (p, jobs.into_iter().map(|j| j.ref_id).collect()))
}

#[test]
fn normal_jobs_are_taken_before_bulk_jobs() {
    let mut q = EmbedQueue::default();
    q.push("p1", job("b1"), true);
    q.push("p1", job("b2"), true);
    q.push("p2", job("n1"), false);
    assert_eq!(
        refs(q.front_batch(8)),
        Some(("p2".into(), vec!["n1".into()]))
    );
    q.remove("p2", &[job("n1")]);
    assert_eq!(
        refs(q.front_batch(8)),
        Some(("p1".into(), vec!["b1".into(), "b2".into()]))
    );
}

#[test]
fn batch_is_limited_to_one_project_and_size() {
    let mut q = EmbedQueue::default();
    q.push("p1", job("a"), false);
    q.push("p1", job("b"), false);
    q.push("p2", job("c"), false);
    q.push("p1", job("d"), false);
    assert_eq!(
        refs(q.front_batch(8)),
        Some(("p1".into(), vec!["a".into(), "b".into()]))
    );
    assert_eq!(
        refs(q.front_batch(1)),
        Some(("p1".into(), vec!["a".into()]))
    );
}

#[test]
fn duplicate_jobs_are_queued_once_and_project_removal_clears_both_lanes() {
    let mut q = EmbedQueue::default();
    q.push("p1", job("a"), false);
    q.push("p1", job("a"), true);
    q.push("p1", job("b"), true);
    assert_eq!(q.len(), 2);
    q.remove_project("p1");
    assert_eq!(q.len(), 0);
    assert!(q.front_batch(8).is_none());
}

#[test]
fn lane_queue_pops_normal_first_then_bulk_in_order() {
    let mut q = LaneQueue::default();
    q.push(1, Lane::Bulk);
    q.push(2, Lane::Normal);
    q.push(3, Lane::Bulk);
    q.push(4, Lane::Normal);
    let order: Vec<i32> = std::iter::from_fn(|| q.pop()).collect();
    assert_eq!(order, vec![2, 4, 1, 3]);
}

#[test]
fn lane_counts_track_each_lane() {
    let mut q = EmbedQueue::default();
    assert_eq!(q.lane_counts(), (0, 0));
    q.push("p1", job("n1"), false);
    q.push("p1", job("b1"), true);
    q.push("p2", job("b2"), true);
    q.push("p2", job("b2"), true);
    assert_eq!(q.lane_counts(), (1, 2));
    q.remove("p2", &[job("b2")]);
    assert_eq!(q.lane_counts(), (1, 1));
    q.remove_project("p1");
    assert_eq!(q.lane_counts(), (0, 0));
}
