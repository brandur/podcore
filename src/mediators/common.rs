use slog::Logger;
use std::str;
use time::precise_time_ns;

#[inline]
pub fn log_timed<T, F>(log: &Logger, f: F) -> T
    where F: FnOnce(&Logger) -> T
{
    let start = precise_time_ns();
    info!(log, "Start");
    let res = f(&log);
    let elapsed = precise_time_ns() - start;
    let (div, unit) = unit(elapsed);
    info!(log, "Finish"; "elapsed" => format!("{:.*}{}", 3, ((elapsed as f64) / div), unit));
    res
}

// Private functions
//

#[inline]
fn unit(ns: u64) -> (f64, &'static str) {
    if ns >= 1_000_000_000 {
        (1_000_000_000_f64, "s")
    } else if ns >= 1_000_000 {
        (1_000_000_f64, "ms")
    } else if ns >= 1_000 {
        (1_000_f64, "µs")
    } else {
        (1_f64, "ns")
    }
}

// Tests
//

#[cfg(test)]
mod tests {
    use mediators::common::*;

    #[test]
    fn test_unit() {
        assert_eq!((1_f64, "ns"), unit(2_u64));
        assert_eq!((1_000_f64, "µs"), unit(2_000_u64));
        assert_eq!((1_000_000_f64, "ms"), unit(2_000_000_u64));
        assert_eq!((1_000_000_000_f64, "s"), unit(2_000_000_000_u64));
    }
}
