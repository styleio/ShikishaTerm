//! Optional limits for untrusted HTTP clients; one request per connection.
use std::io::{self, Read};
use std::sync::{Arc, Mutex, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Waiting for application work after receiving a body is not a timeout.
#[derive(Clone, Copy)]
pub struct Limits {
    pub connections: usize,
    pub request_timeout: Duration,
    pub response_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self { connections: 128, request_timeout: Duration::from_secs(15), response_timeout: Duration::from_secs(30) }
    }
}
pub(crate) struct Guard {
    read_closed: AtomicBool,
    deadline: Mutex<Option<Instant>>,
    response_timeout: Duration,
}
impl Guard {
    pub(crate) fn complete(&self) { *self.deadline.lock().unwrap() = None; }
    pub(crate) fn responding(&self) { *self.deadline.lock().unwrap() = Some(Instant::now() + self.response_timeout); }
    pub(crate) fn close_read(&self) { self.read_closed.store(true, Ordering::Relaxed); }
    fn remaining(&self) -> io::Result<Option<Duration>> {
        self.deadline.lock().unwrap().map(|at| at.checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero()).ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP delivery deadline"))).transpose()
    }
    pub(crate) fn reading(&self) -> io::Result<Option<Duration>> {
        if self.read_closed.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "HTTP body no longer needed"));
        }
        self.remaining()
    }
    pub(crate) fn writing(&self) -> io::Result<Option<Duration>> { self.remaining() }
}
pub(crate) struct Connections {
    limits: Limits,
    active: Mutex<Vec<Weak<Guard>>>,
}
impl Connections {
    pub(crate) fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self { limits, active: Mutex::new(Vec::new()) })
    }
    pub(crate) fn accept(&self) -> Option<Arc<Guard>> {
        let mut active = self.active.lock().unwrap();
        active.retain(|c| c.strong_count() > 0);
        if active.len() >= self.limits.connections { return None; }
        let guard = Arc::new(Guard {
            read_closed: AtomicBool::new(false),
            deadline: Mutex::new(Some(Instant::now() + self.limits.request_timeout)),
            response_timeout: self.limits.response_timeout,
        });
        active.push(Arc::downgrade(&guard));
        Some(guard)
    }
}
pub(crate) struct Body {
    pub inner: Box<dyn Read + Send>,
    pub guard: Arc<Guard>,
    pub remaining: Option<usize>,
}
impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(left) = &mut self.remaining { *left = left.saturating_sub(n); }
        if (n == 0 && !buf.is_empty()) || self.remaining == Some(0) { self.guard.complete(); }
        Ok(n)
    }
}
