//! 데몬의 두 단계 큐. 실시간 작업을 가져오기 작업보다 먼저 꺼낸다. (FR-DMN-6, FR-IMP-6)

use std::collections::{HashSet, VecDeque};

use crate::store::EmbedJob;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Normal,
    Bulk,
}

/// 단계마다 FIFO이고, 일반 단계가 비어 있을 때만 `bulk` 단계에서 꺼낸다.
#[derive(Debug)]
pub struct LaneQueue<T> {
    normal: VecDeque<T>,
    bulk: VecDeque<T>,
}

impl<T> Default for LaneQueue<T> {
    fn default() -> Self {
        Self {
            normal: VecDeque::new(),
            bulk: VecDeque::new(),
        }
    }
}

impl<T> LaneQueue<T> {
    pub fn push(&mut self, item: T, lane: Lane) {
        match lane {
            Lane::Normal => self.normal.push_back(item),
            Lane::Bulk => self.bulk.push_back(item),
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.normal.pop_front().or_else(|| self.bulk.pop_front())
    }

    pub fn is_empty(&self) -> bool {
        self.normal.is_empty() && self.bulk.is_empty()
    }
}

/// (프로젝트 id, 작업) 임베딩 큐. 같은 작업은 한 번만 들어간다.
#[derive(Debug, Default)]
pub struct EmbedQueue {
    normal: VecDeque<(String, EmbedJob)>,
    bulk: VecDeque<(String, EmbedJob)>,
    queued: HashSet<(String, EmbedJob)>,
}

impl EmbedQueue {
    pub fn push(&mut self, project: &str, job: EmbedJob, bulk: bool) {
        let item = (project.to_string(), job);
        if self.queued.insert(item.clone()) {
            if bulk {
                self.bulk.push_back(item);
            } else {
                self.normal.push_back(item);
            }
        }
    }

    /// 앞 단계의 앞에서부터 같은 프로젝트의 작업을 `limit`개까지.
    pub fn front_batch(&self, limit: usize) -> Option<(String, Vec<EmbedJob>)> {
        let lane = if self.normal.is_empty() {
            &self.bulk
        } else {
            &self.normal
        };
        let project = lane.front()?.0.clone();
        let jobs = lane
            .iter()
            .take_while(|(p, _)| *p == project)
            .take(limit.max(1))
            .map(|(_, j)| j.clone())
            .collect();
        Some((project, jobs))
    }

    pub fn remove(&mut self, project: &str, jobs: &[EmbedJob]) {
        for lane in [&mut self.normal, &mut self.bulk] {
            lane.retain(|(p, j)| !(p == project && jobs.contains(j)));
        }
        for job in jobs {
            self.queued.remove(&(project.to_string(), job.clone()));
        }
    }

    pub fn remove_project(&mut self, project: &str) {
        for lane in [&mut self.normal, &mut self.bulk] {
            lane.retain(|(p, _)| p != project);
        }
        self.queued.retain(|(p, _)| p != project);
    }

    pub fn len(&self) -> usize {
        self.queued.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    /// (일반 단계, bulk 단계) 대기 작업 수.
    pub fn lane_counts(&self) -> (usize, usize) {
        (self.normal.len(), self.bulk.len())
    }
}
