//! Scoped immutable-object work; every worker is joined before returning.
use crate::{Error, Result};

pub(crate) fn parallel_map<T: Sync, R: Send>(
    work: &[T],
    workers: usize,
    load: &(impl Fn(&T) -> Result<R> + Sync),
) -> Result<Vec<R>> {
    if workers <= 1 || work.len() < 32 {
        return work.iter().map(load).collect();
    }
    std::thread::scope(|scope| {
        let tasks: Vec<_> = work
            .chunks(work.len().div_ceil(workers))
            .map(|chunk| scope.spawn(move || chunk.iter().map(load).collect::<Result<Vec<_>>>()))
            .collect();
        let mut values = Vec::with_capacity(work.len());
        let mut error = None;
        // Join every worker, including failures; no object I/O survives the call.
        for task in tasks {
            match task.join() {
                Ok(Ok(items)) => values.extend(items),
                Ok(Err(cause)) => {
                    if error.is_none() {
                        error = Some(cause);
                    }
                }
                Err(_) => {
                    if error.is_none() {
                        error = Some(Error::Invalid("object worker panicked".into()));
                    }
                }
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(values),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc, Barrier, Mutex};

    #[test]
    fn errors_and_panics_join_other_workers_before_returning() {
        for panic in [false, true] {
            let barrier = Arc::new(Barrier::new(2));
            let (started, receive_started) = mpsc::channel();
            let (release, receive_release) = mpsc::channel();
            let receive_release = Mutex::new(receive_release);
            let coordinator = std::thread::spawn(move || {
                let work: Vec<_> = (0usize..64).collect();
                parallel_map(&work, 2, &|&n| {
                    if n == 0 || n == 32 {
                        barrier.wait();
                    }
                    if n == 0 {
                        assert!(!panic, "injected object worker panic");
                        return Err(Error::Invalid("injected object failure".into()));
                    }
                    if n == 32 {
                        started.send(()).unwrap();
                        receive_release.lock().unwrap().recv().unwrap();
                    }
                    Ok(n)
                })
            });
            receive_started
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            assert!(!coordinator.is_finished());
            release.send(()).unwrap();
            assert!(coordinator.join().unwrap().is_err());
        }
    }
}
