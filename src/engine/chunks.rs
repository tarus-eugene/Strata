use super::*;
use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};
use tokio::sync::Notify;

const TARGET_CHUNK: u64 = 4 * 1024 * 1024;
const MAX_CHUNKS: usize = 8192;

pub(super) enum Outcome {
    Complete,
    Split,
}
pub(super) enum Interrupt {
    Stop,
    Split,
}

pub(super) async fn interrupted(job: &Job, split: &AtomicBool, abort: &AtomicBool) -> Interrupt {
    loop {
        if job.control.load(Ordering::SeqCst) != RUN || abort.load(Ordering::SeqCst) {
            return Interrupt::Stop;
        }
        if split.load(Ordering::SeqCst) {
            return Interrupt::Split;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(super) fn plan(size: u64, connections: usize) -> Vec<Part> {
    let connections = connections.clamp(1, 16);
    let count = size.div_ceil(TARGET_CHUNK).min(4096) as usize;
    partition(
        size,
        count
            .max(connections)
            .min((size / MIN_PART).max(1) as usize),
    )
}

pub(super) fn ordered_indices(parts: &[Part], total: u64) -> Result<Vec<usize>> {
    let mut indices = (0..parts.len()).collect::<Vec<_>>();
    indices.sort_unstable_by_key(|index| parts[*index].start);
    let mut next = 0;
    for &index in &indices {
        let part = &parts[index];
        ensure!(
            part.start == next && part.end >= part.start && part.end < total,
            "The saved chunk ranges overlap or have a gap"
        );
        next = part.end + 1;
    }
    ensure!(
        next == total,
        "The saved chunk ranges do not cover the whole file"
    );
    Ok(indices)
}

struct Active {
    started: Instant,
    baseline: u64,
    split: Arc<AtomicBool>,
}

struct State {
    pending: VecDeque<usize>,
    active: HashMap<usize, Active>,
    error: Option<anyhow::Error>,
}

struct Queue {
    state: Mutex<State>,
    notify: Notify,
    abort: AtomicBool,
}

impl Queue {
    fn fail(&self, error: anyhow::Error) {
        let mut state = self.state.lock().unwrap();
        if state.error.is_none() {
            state.error = Some(error);
        }
        self.abort.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    async fn next(&self, job: &Job) -> Option<(usize, Arc<AtomicBool>)> {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self.state.lock().unwrap();
                if self.abort.load(Ordering::SeqCst) || job.control.load(Ordering::SeqCst) != RUN {
                    return None;
                }
                if let Some(index) = state.pending.pop_front() {
                    let split = Arc::new(AtomicBool::new(false));
                    let baseline = job.data.lock().unwrap().parts[index].downloaded;
                    state.active.insert(
                        index,
                        Active {
                            started: Instant::now(),
                            baseline,
                            split: split.clone(),
                        },
                    );
                    return Some((index, split));
                }
                if state.active.is_empty() {
                    return None;
                }
                let data = job.data.lock().unwrap();
                if data.ranged && data.parts.len() < MAX_CHUNKS {
                    let candidate = state
                        .active
                        .iter()
                        .filter(|(index, active)| {
                            let part = &data.parts[**index];
                            let elapsed = active.started.elapsed().as_secs_f64();
                            let received = part.downloaded.saturating_sub(active.baseline);
                            let remaining = part.len().saturating_sub(part.downloaded);
                            !active.split.load(Ordering::SeqCst)
                                && elapsed >= 0.75
                                && received > 0
                                && remaining >= 2 * MIN_PART
                                && remaining as f64 * elapsed / received as f64 >= 1.0
                        })
                        .max_by_key(|(index, _)| {
                            data.parts[**index]
                                .len()
                                .saturating_sub(data.parts[**index].downloaded)
                        });
                    if let Some((_, active)) = candidate {
                        active.split.store(true, Ordering::SeqCst);
                    }
                }
            }
            tokio::select! {
                _ = notified => {},
                _ = tokio::time::sleep(Duration::from_millis(150)) => {},
                _ = stopped(job) => return None,
            }
        }
    }

    fn finish(&self, job: &Job, index: usize, outcome: Outcome) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.active.remove(&index);
        if matches!(outcome, Outcome::Split) {
            // The original response and writer have stopped before changing either range.
            let mut data = job.data.lock().unwrap();
            let received = fs::metadata(data.parts_dir().join(format!("{index}.part")))?.len();
            let part = &data.parts[index];
            ensure!(received <= part.len(), "A chunk exceeds its assigned range");
            let remaining = part.len() - received;
            if remaining >= 2 * MIN_PART && data.parts.len() < MAX_CHUNKS {
                let boundary = part.start + received + remaining / 2;
                let end = part.end;
                let new_index = data.parts.len();
                fs::File::create(data.parts_dir().join(format!("{new_index}.part")))?;
                data.parts[index].end = boundary - 1;
                data.parts[index].downloaded = received;
                data.parts.push(Part {
                    start: boundary,
                    end,
                    downloaded: 0,
                });
                state.pending.push_back(new_index);
            }
            state.pending.push_back(index);
            data.downloaded = data.parts.iter().map(|part| part.downloaded).sum();
        }
        self.notify.notify_waiters();
        Ok(())
    }
}

pub(super) async fn download(job: Arc<Job>, client: Client) -> Result<()> {
    let (count, workers) = {
        let data = job.data.lock().unwrap();
        if data.ranged {
            ordered_indices(&data.parts, data.total.unwrap())?;
        }
        (
            data.parts.len(),
            data.connections.clamp(1, 16).min(data.parts.len()).max(1),
        )
    };
    let queue = Arc::new(Queue {
        state: Mutex::new(State {
            pending: (0..count).collect(),
            active: HashMap::new(),
            error: None,
        }),
        notify: Notify::new(),
        abort: AtomicBool::new(false),
    });
    let mut tasks = JoinSet::new();
    for _ in 0..workers {
        let (job, client, queue) = (job.clone(), client.clone(), queue.clone());
        tasks.spawn(async move {
            while let Some((index, split)) = queue.next(&job).await {
                match download_part(client.clone(), job.clone(), index, &split, &queue.abort).await
                {
                    Ok(outcome) => {
                        if let Err(error) = queue.finish(&job, index, outcome) {
                            queue.fail(error);
                            break;
                        }
                    }
                    Err(error) => {
                        queue.fail(error);
                        break;
                    }
                }
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            queue.fail(error.into());
        }
    }
    let error = queue.state.lock().unwrap().error.take();
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ordered_indices, plan};

    #[test]
    fn queued_chunks_cover_the_file_and_remain_valid_when_appended_out_of_order() {
        let parts = plan(32 * 1024 * 1024, 2);
        assert_eq!(parts.len(), 8);
        assert_eq!(ordered_indices(&parts, 32 * 1024 * 1024).unwrap().len(), 8);
        let mut shuffled = parts;
        shuffled.swap(0, 7);
        assert_eq!(ordered_indices(&shuffled, 32 * 1024 * 1024).unwrap()[0], 7);
        shuffled[0].start += 1;
        assert!(ordered_indices(&shuffled, 32 * 1024 * 1024).is_err());
    }
}
