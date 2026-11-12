//! §8.3 The device mutex: at most one state-changing request open per device.
//!
//! Hub's own clients take it before Hub sends `runtime.push`,
//! `runtime.native.install` or `runtime.launch`. A second caller waits in a
//! bounded FIFO queue and gets `busy` naming the holder when its wait times
//! out (the recommendation of §11); a caller that finds the queue full gets
//! `busy` at once. The holder is visible in the device's status.
//!
//! The mutex is released when the guard drops: at the request's `REPLY`, when
//! the connection that carried it closed (the request fails, its caller's
//! future returns), or when the caller's future is dropped because its local
//! client went away (the request is then cancelled, see `ops`). After a reply
//! with `phase: "launching"` the guard is turned into a hold that lasts until
//! the device links again or the launch hold passes.
//!
//! Every change of the holder (taken, released, turned into a launch hold,
//! the hold ending) calls the lock's observer, so the endpoint can tell its
//! subscribers: the holder is part of what a device shows.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

#[derive(Debug, Default)]
struct State {
    holder: Option<String>,
    /// Epoch milliseconds of when the holder took the mutex.
    since: Option<i64>,
    /// The launch hold: the holder stays until this instant or the next `ready`.
    until: Option<Instant>,
    queue: VecDeque<u64>,
    next_ticket: u64,
}

impl State {
    fn expire(&mut self) {
        if self.until.is_some_and(|until| Instant::now() >= until) {
            self.holder = None;
            self.since = None;
            self.until = None;
        }
    }

    fn take(&mut self, holder: &str) {
        self.holder = Some(holder.to_owned());
        self.since = Some(epoch_ms());
    }
}

fn epoch_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Called after every change of the holder, outside the lock's own mutex.
pub type Observer = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub struct DeviceLock {
    state: Mutex<State>,
    notify: Notify,
    observer: OnceLock<Observer>,
}

impl std::fmt::Debug for DeviceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLock").field("state", &self.state).finish_non_exhaustive()
    }
}

/// Why the mutex was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub holder: String,
}

impl DeviceLock {
    /// The current holder, as Hub's device status shows it.
    pub fn holder(&self) -> Option<String> {
        self.held().map(|(holder, _)| holder)
    }

    /// The current holder and the epoch milliseconds since it holds the mutex.
    pub fn held(&self) -> Option<(String, i64)> {
        let mut state = self.state.lock().unwrap();
        state.expire();
        Some((state.holder.clone()?, state.since.unwrap_or(0)))
    }

    /// Sets the function called after each change of the holder; the first
    /// one set stays.
    pub fn observe(&self, observer: Observer) {
        let _ = self.observer.set(observer);
    }

    fn changed(&self) {
        if let Some(observer) = self.observer.get() {
            observer();
        }
    }

    /// Takes the mutex for `holder`, waiting in FIFO order for at most `wait`.
    pub async fn acquire(self: &Arc<Self>, holder: &str, wait: Duration, max_queue: usize) -> Result<LockGuard, Held> {
        let deadline = Instant::now() + wait;
        let ticket;
        {
            let mut state = self.state.lock().unwrap();
            state.expire();
            if state.holder.is_none() && state.queue.is_empty() {
                state.take(holder);
                drop(state);
                self.changed();
                return Ok(LockGuard { lock: self.clone(), armed: true });
            }
            if state.queue.len() >= max_queue {
                return Err(Held { holder: state.holder.clone().unwrap_or_default() });
            }
            ticket = state.next_ticket;
            state.next_ticket += 1;
            state.queue.push_back(ticket);
        }
        // Removes the ticket when the caller gives up or its future is dropped.
        struct Ticket<'a>(&'a DeviceLock, u64, bool);
        impl Drop for Ticket<'_> {
            fn drop(&mut self) {
                if self.2 {
                    let mut state = self.0.state.lock().unwrap();
                    state.queue.retain(|&t| t != self.1);
                    drop(state);
                    self.0.notify.notify_waiters();
                }
            }
        }
        let mut queued = Ticket(self, ticket, true);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let wake_at = {
                let mut state = self.state.lock().unwrap();
                state.expire();
                if state.holder.is_none() && state.queue.front() == Some(&ticket) {
                    state.queue.pop_front();
                    state.take(holder);
                    queued.2 = false;
                    drop(state);
                    self.changed();
                    return Ok(LockGuard { lock: self.clone(), armed: true });
                }
                if Instant::now() >= deadline {
                    return Err(Held { holder: state.holder.clone().unwrap_or_default() });
                }
                state.until.map_or(deadline, |until| until.min(deadline))
            };
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep_until(wake_at) => {}
            }
        }
    }

    fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.holder = None;
        state.since = None;
        state.until = None;
        drop(state);
        self.notify.notify_waiters();
        self.changed();
    }

    /// A launch hold's end by time: waiters wake and the observer hears of it.
    fn hold_ended(&self) {
        let mut state = self.state.lock().unwrap();
        let before = state.holder.is_some();
        state.expire();
        let ended = before && state.holder.is_none();
        drop(state);
        if ended {
            self.notify.notify_waiters();
            self.changed();
        }
    }

    /// The device linked again: a launch hold ends (§8.3).
    pub fn relinked(&self) {
        let held = self.state.lock().unwrap().until.is_some();
        if held {
            self.release();
        }
    }
}

/// The mutex while one state-changing request is open.
#[derive(Debug)]
pub struct LockGuard {
    lock: Arc<DeviceLock>,
    armed: bool,
}

impl LockGuard {
    /// After a `launching` reply: the holder stays until the device links
    /// again or `hold` passes.
    pub fn hold(mut self, hold: Duration) {
        self.armed = false;
        let mut state = self.lock.state.lock().unwrap();
        if let Some(holder) = state.holder.take() {
            state.holder = Some(format!("{holder} (launching)"));
        }
        let until = Instant::now() + hold;
        state.until = Some(until);
        drop(state);
        self.lock.notify.notify_waiters();
        self.lock.changed();
        // The hold ends by time without anyone asking for the holder: a
        // timer makes that end visible to the observer.
        let lock = Arc::downgrade(&self.lock);
        tokio::spawn(async move {
            tokio::time::sleep_until(until).await;
            if let Some(lock) = lock.upgrade() {
                lock.hold_ended();
            }
        });
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.armed {
            self.lock.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fifo_wait_busy_and_hold() {
        let lock = Arc::new(DeviceLock::default());
        let first = lock.acquire("cli", Duration::from_millis(10), 2).await.unwrap();
        assert_eq!(lock.holder().as_deref(), Some("cli"));
        let busy = lock.acquire("ui", Duration::from_millis(30), 2).await.unwrap_err();
        assert_eq!(busy.holder, "cli");

        let waiter = {
            let lock = lock.clone();
            tokio::spawn(async move { lock.acquire("relay", Duration::from_secs(5), 2).await.map(|_| ()) })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(first);
        waiter.await.unwrap().unwrap();

        let second = lock.acquire("cli", Duration::from_millis(10), 2).await.unwrap();
        second.hold(Duration::from_millis(50));
        assert_eq!(lock.holder().as_deref(), Some("cli (launching)"));
        lock.relinked();
        assert_eq!(lock.holder(), None);
        let third = lock.acquire("cli", Duration::from_millis(10), 2).await.unwrap();
        third.hold(Duration::from_millis(30));
        assert!(lock.acquire("ui", Duration::from_secs(2), 2).await.is_ok());
    }
}
