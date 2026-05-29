/// Single-producer single-consumer lock-free ring buffer.
///
/// The producer (`Sender`) and consumer (`Receiver`) are distinct types that
/// may be sent to different threads. The buffer itself is heap-allocated and
/// shared via `Arc`.
///
/// Uses a power-of-two capacity so the index wrap can use a bitmask. The
/// actual usable capacity is `capacity` slots; one extra slot is wasted to
/// distinguish full from empty without a separate counter.
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Inner<T> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize, // written by Sender
    tail: AtomicUsize, // written by Receiver
}

// Safety: T: Send means it's safe to move T across threads.
// head is written only by Sender; tail only by Receiver — no data races.
unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

/// Create a linked (Sender, Receiver) pair with `capacity` usable slots.
/// `capacity` is rounded up to the next power of two.
pub fn channel<T: Send>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0, "ring buffer capacity must be > 0");
    let cap = capacity.next_power_of_two();
    // Allocate cap+1 slots so we can distinguish full (head+1 == tail) from empty.
    let len = cap + 1;
    let buf: Box<[UnsafeCell<MaybeUninit<T>>]> = (0..len)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect::<Vec<_>>()
        .into_boxed_slice();

    // Index wrapping uses `% len` (not a bitmask) because len = cap+1 is not
    // a power of two. This is fine — the ring buffer is used for main-thread
    // messages, not sample-rate inner loops.
    let inner = Arc::new(Inner {
        buf,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    });

    (Sender { inner: inner.clone() }, Receiver { inner })
}

impl<T: Send> Sender<T> {
    /// Try to push a value into the buffer.
    /// Returns `Err(value)` if the buffer is full.
    pub fn try_send(&self, value: T) -> Result<(), T> {
        let len = self.inner.buf.len();
        let head = self.inner.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % len;
        if next_head == self.inner.tail.load(Ordering::Acquire) {
            return Err(value);
        }
        // SAFETY: head is only written by this Sender; the slot at `head` is
        // not accessible to the Receiver until head advances.
        unsafe {
            (*self.inner.buf[head].get()).write(value);
        }
        self.inner.head.store(next_head, Ordering::Release);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_full(&self) -> bool {
        let len = self.inner.buf.len();
        let head = self.inner.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % len;
        next_head == self.inner.tail.load(Ordering::Acquire)
    }
}

impl<T: Send> Receiver<T> {
    /// Try to pop a value from the buffer.
    /// Returns `None` if the buffer is empty.
    pub fn try_recv(&self) -> Option<T> {
        let len = self.inner.buf.len();
        let tail = self.inner.tail.load(Ordering::Relaxed);
        if tail == self.inner.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: tail is only written by this Receiver; head >= tail+1 here,
        // so the Sender has fully written the slot at `tail`.
        let value = unsafe { (*self.inner.buf[tail].get()).assume_init_read() };
        let next_tail = (tail + 1) % len;
        self.inner.tail.store(next_tail, Ordering::Release);
        Some(value)
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.tail.load(Ordering::Relaxed) == self.inner.head.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_recv_roundtrip() {
        let (tx, rx) = channel::<u32>(4);
        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_ok());
        assert_eq!(rx.try_recv(), Some(1));
        assert_eq!(rx.try_recv(), Some(2));
        assert_eq!(rx.try_recv(), None);
    }

    #[test]
    fn full_buffer_returns_err() {
        let (tx, _rx) = channel::<u32>(4);
        for i in 0..4 {
            assert!(tx.try_send(i).is_ok());
        }
        assert!(tx.try_send(99).is_err());
    }
}
