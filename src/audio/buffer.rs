use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct AudioRing {
    inner: Arc<Inner>,
}

pub struct AudioProducer {
    inner: Arc<Inner>,
}

pub struct AudioConsumer {
    inner: Arc<Inner>,
}

struct Inner {
    buffer: Box<[UnsafeCell<f32>]>,
    capacity: usize,
    read: AtomicUsize,
    write: AtomicUsize,
}

unsafe impl Sync for Inner {}
unsafe impl Send for Inner {}

impl AudioRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 1);
        let buffer = (0..capacity)
            .map(|_| UnsafeCell::new(0.0))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self {
            inner: Arc::new(Inner {
                buffer,
                capacity,
                read: AtomicUsize::new(0),
                write: AtomicUsize::new(0),
            }),
        }
    }

    pub fn split(self) -> (AudioProducer, AudioConsumer) {
        (
            AudioProducer {
                inner: Arc::clone(&self.inner),
            },
            AudioConsumer { inner: self.inner },
        )
    }
}

impl AudioProducer {
    pub fn push_slice_lossy(&self, samples: &[f32]) {
        for &sample in samples {
            self.push_lossy(sample);
        }
    }

    #[inline]
    pub fn push_lossy(&self, sample: f32) {
        let write = self.inner.write.load(Ordering::Relaxed);
        let next = self.inner.next(write);
        let read = self.inner.read.load(Ordering::Acquire);

        if next == read {
            let _ = self.inner.read.compare_exchange(
                read,
                self.inner.next(read),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }

        unsafe {
            *self.inner.buffer[write].get() = sample;
        }
        self.inner.write.store(next, Ordering::Release);
    }
}

impl AudioConsumer {
    pub fn pop_into(&self, out: &mut [f32]) -> usize {
        let mut count = 0;

        while count < out.len() {
            let read = self.inner.read.load(Ordering::Relaxed);
            let write = self.inner.write.load(Ordering::Acquire);
            if read == write {
                break;
            }

            out[count] = unsafe { *self.inner.buffer[read].get() };
            self.inner.read.store(self.inner.next(read), Ordering::Release);
            count += 1;
        }

        count
    }
}

impl Inner {
    #[inline]
    fn next(&self, index: usize) -> usize {
        let next = index + 1;
        if next == self.capacity { 0 } else { next }
    }
}

#[cfg(test)]
mod tests {
    use super::AudioRing;

    #[test]
    fn wraps_and_keeps_newest_samples() {
        let (producer, consumer) = AudioRing::new(4).split();

        producer.push_slice_lossy(&[1.0, 2.0, 3.0, 4.0, 5.0]);

        let mut out = [0.0; 4];
        let count = consumer.pop_into(&mut out);

        assert_eq!(count, 3);
        assert_eq!(&out[..count], &[3.0, 4.0, 5.0]);
    }
}

