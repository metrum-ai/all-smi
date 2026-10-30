// Copyright (c) 2026 Metrum AI, Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Serializes shared PDH sampling and coalesces concurrent consumers.
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Coalesced<T>(Mutex<Option<(Instant, T)>>);

impl<T> Default for Coalesced<T> {
    fn default() -> Self {
        Self(Mutex::new(None))
    }
}

impl<T: Clone> Coalesced<T> {
    pub fn get_or_collect(&self, max_age: Duration, collect: impl FnOnce() -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((time, value)) = guard.as_ref()
            && time.elapsed() < max_age
        {
            return value.clone();
        }
        let value = collect();
        *guard = Some((Instant::now(), value.clone()));
        value
    }

    pub fn latest(&self) -> Option<T> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|(_, value)| value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn concurrent_gpu_and_npu_consumers_share_one_collection() {
        let cache = Arc::new(Coalesced::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(3));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let (cache, calls, barrier) = (cache.clone(), calls.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    cache.get_or_collect(Duration::from_secs(10), || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (89.0, 60.0)
                    })
                })
            })
            .collect();
        barrier.wait();
        for worker in workers {
            assert_eq!(worker.join().unwrap(), (89.0, 60.0));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.latest(), Some((89.0, 60.0)));
        let fresh = cache.get_or_collect(Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            (92.0, 0.0)
        });
        assert_eq!(fresh, (92.0, 0.0));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
