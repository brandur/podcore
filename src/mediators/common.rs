use hyper::StatusCode;
use slog::Logger;
use std::str;
use time::precise_time_ns;

#[inline]
pub fn log_timed<T, F>(log: &Logger, f: F) -> T
where
    F: FnOnce(&Logger) -> T,
{
    let start = precise_time_ns();
    info!(log, "Start");
    let res = f(log);
    let elapsed = precise_time_ns() - start;
    let (div, unit) = unit(elapsed);
    info!(log, "Finish"; "elapsed" => format!("{:.*}{}", 3, ((elapsed as f64) / div), unit));
    res
}

pub fn log_body_sample(log: &Logger, status: StatusCode, body: &[u8]) {
    let sample = body.iter().take(200).cloned().collect::<Vec<u8>>();
    let string = String::from_utf8_lossy(sample.as_slice()).replace("\n", "");
    info!(log, "Response (sample)"; "status" => status.to_string(), "body" => format!("{}...", string));
}

pub fn thread_name(n: u32) -> String {
    format!("thread_{:03}", n).to_string()
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
    use test_helpers;

    #[test]
    fn test_log_body_sample() {
        // Not much of a test, but we're just making sure that no errors are thrown
        log_body_sample(
            &test_helpers::log(),
            StatusCode::Ok,
            &b"Short string".to_vec(),
        );
        log_body_sample(
            &test_helpers::log(),
            StatusCode::Ok,
            &br#"
            Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor
            incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud
            exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure
            dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur.
            Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt
            mollit anim id est laborum.
"#.to_vec(),
        );
    }

    #[test]
    fn test_thread_name() {
        assert_eq!("thread_000".to_string(), thread_name(0));
        assert_eq!("thread_999".to_string(), thread_name(999));
        assert_eq!("thread_1000".to_string(), thread_name(1000));
    }

    #[test]
    fn test_unit() {
        assert_eq!((1_f64, "ns"), unit(2_u64));
        assert_eq!((1_000_f64, "µs"), unit(2_000_u64));
        assert_eq!((1_000_000_f64, "ms"), unit(2_000_000_u64));
        assert_eq!((1_000_000_000_f64, "s"), unit(2_000_000_000_u64));
    }
}
