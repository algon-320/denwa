use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

pub struct TrafficMeter {
    upload: AtomicUsize,
    download: AtomicUsize,
    start: Instant,
}

impl TrafficMeter {
    pub fn new() -> Self {
        Self {
            upload: AtomicUsize::new(0),
            download: AtomicUsize::new(0),
            start: Instant::now(),
        }
    }

    pub fn sent_bytes(&self, nb: usize) {
        self.upload.fetch_add(nb, Ordering::Relaxed);
    }

    pub fn received_bytes(&self, nb: usize) {
        self.download.fetch_add(nb, Ordering::Relaxed);
    }
}

impl std::fmt::Display for TrafficMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let elapsed = self.start.elapsed().as_secs();
        let upload = self.upload.load(Ordering::Relaxed);
        let download = self.download.load(Ordering::Relaxed);
        write!(
            f,
            "up: {:.3} Kbps (total: {} KiB) / down: {:.3} Kbps (total: {} KiB)",
            (upload * 8) as f32 / elapsed as f32 / 1024.0,
            upload / 1024,
            (download * 8) as f32 / elapsed as f32 / 1024.0,
            download / 1024,
        )
    }
}
