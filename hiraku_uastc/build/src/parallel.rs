//! Build-only CPU scheduling. Results may accumulate without a memory cap;
//! only the caller writes the archive, in input order.
use super::{Result, progress::PackProgress};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

/// Reserve a CPU for compression when possible; aim for four threads per
/// image and distribute the remaining budget without oversubscribing it.
pub(crate) fn worker_threads(budget: usize, jobs: usize) -> Vec<u32> {
    if jobs == 0 {
        return vec![];
    }
    let encode = budget.saturating_sub(1).max(1);
    let workers = encode.div_ceil(4).min(jobs);
    (0..workers)
        .map(|i| ((encode / workers + usize::from(i < encode % workers)).min(8)) as u32)
        .collect()
}

pub(crate) enum Event<T> {
    Progress(PackProgress),
    Ready(usize, T),
}

pub(crate) fn ordered<T: Send>(
    count: usize,
    threads: &[u32],
    work: impl Fn(usize, u32, &dyn Fn(PackProgress)) -> Result<T> + Sync,
    mut consume: impl FnMut(Event<T>) -> Result<()>,
) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    if threads.is_empty() {
        return Err("parallel encoding needs at least one worker".into());
    }
    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let (sender, receiver) = mpsc::channel::<Result<Event<T>>>();
    thread::scope(|scope| {
        let mut workers = Vec::new();
        let result = (|| -> Result<()> {
            for (worker, &threads) in threads.iter().enumerate() {
                let sender = sender.clone();
                let (next, cancelled, work) = (&next, &cancelled, &work);
                workers.push(
                    thread::Builder::new()
                        .name(format!("uastc-{worker}"))
                        .spawn_scoped(scope, move || {
                            while !cancelled.load(Ordering::Relaxed) {
                                let index = next.fetch_add(1, Ordering::Relaxed);
                                if index >= count {
                                    break;
                                }
                                let report = |event| {
                                    let _ = sender.send(Ok(Event::Progress(event)));
                                };
                                let result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        work(index, threads, &report)
                                    }))
                                    .unwrap_or_else(|_| {
                                        Err(format!(
                                            "texture encoding worker panicked at task {index}"
                                        )
                                        .into())
                                    });
                                let failed = result.is_err();
                                if failed {
                                    cancelled.store(true, Ordering::Relaxed);
                                }
                                if sender
                                    .send(result.map(|value| Event::Ready(index, value)))
                                    .is_err()
                                    || failed
                                {
                                    break;
                                }
                            }
                        })?,
                );
            }
            drop(sender);
            let mut waiting = BTreeMap::new();
            let mut expected = 0;
            while expected < count {
                match receiver
                    .recv()
                    .map_err(|_| "texture workers stopped before all results arrived")??
                {
                    Event::Progress(event) => consume(Event::Progress(event))?,
                    Event::Ready(index, value) => {
                        waiting.insert(index, value);
                    }
                }
                while let Some(value) = waiting.remove(&expected) {
                    consume(Event::Ready(expected, value))?;
                    expected += 1;
                }
            }
            Ok(())
        })();
        cancelled.store(true, Ordering::Relaxed);
        // A running native encode cannot be interrupted safely. Join it, but
        // never start further jobs after an encoder/writer/reporting failure.
        for worker in workers {
            if worker.join().is_err() && result.is_ok() {
                return Err("texture worker panicked".into());
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_is_shared_instead_of_multiplied_per_image() {
        for budget in 1..129 {
            for jobs in [0, 1, 2, 100] {
                let threads = worker_threads(budget, jobs);
                assert!(threads.len() <= jobs);
                assert!(threads.iter().all(|t| (1..=8).contains(t)));
                assert!(
                    threads.iter().map(|t| *t as usize).sum::<usize>()
                        <= budget.saturating_sub(1).max(1)
                );
            }
        }
    }
    #[test]
    fn out_of_order_work_is_consumed_in_order() {
        let barrier = std::sync::Barrier::new(2);
        let mut output = Vec::new();
        ordered(
            3,
            &[1, 1],
            |index, _, _| {
                // Job 1 must have been submitted before its worker can start 2.
                if index == 0 || index == 2 {
                    barrier.wait();
                }
                Ok(index)
            },
            |event| {
                if let Event::Ready(index, value) = event {
                    output.push((index, value));
                }
                Ok(())
            },
        )
        .expect("parallel work");
        assert_eq!(output, [(0, 0), (1, 1), (2, 2)]);
    }
    #[test]
    fn errors_stop_new_work_and_propagate() {
        let mut consumed = 0;
        let result = ordered(
            3,
            &[1],
            |index, _, _| {
                if index == 1 {
                    Err("synthetic encode failure".into())
                } else {
                    Ok(index)
                }
            },
            |_| {
                consumed += 1;
                Ok(())
            },
        );
        assert!(
            result
                .expect_err("failure")
                .to_string()
                .contains("synthetic encode failure")
        );
        assert_eq!(consumed, 1);
        assert!(ordered(3, &[1, 1], |i, _, _| Ok(i), |_| Err("writer failed".into())).is_err());
    }
}
