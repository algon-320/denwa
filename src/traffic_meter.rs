use std::fmt::{self, Display, Formatter};
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

impl Display for TrafficMeter {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let elapsed = self.start.elapsed().as_secs() as f64;
        let up = self.upload.load(Ordering::Relaxed) as f64;
        let dw = self.download.load(Ordering::Relaxed) as f64;
        let up_bps = DataAmount((up * 8.0) / elapsed, "bps");
        let dw_bps = DataAmount((dw * 8.0) / elapsed, "bps");
        let up_amo = DataAmount(up, "iB");
        let dw_amo = DataAmount(dw, "iB");
        write!(
            f,
            "up: {} (total: {}) / down: {} (total: {})",
            up_bps, up_amo, dw_bps, dw_amo,
        )
    }
}

struct DataAmount(f64, &'static str);

impl Display for DataAmount {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (amo, unit) = make_human_friendly(self.0);
        write!(f, "{:.3} {}{}", amo, unit, self.1)
    }
}

fn make_human_friendly(amount: f64) -> (f64, &'static str) {
    let table = [
        (0u64, ""),
        (1 << 10, "K"),
        (1 << 20, "M"),
        (1 << 40, "G"),
        (1 << 60, "T"),
    ];
    for (th, unit) in table.iter().copied().rev() {
        let th = th as f64;
        if amount >= th {
            return (amount / th, unit);
        }
    }
    unreachable!()
}
