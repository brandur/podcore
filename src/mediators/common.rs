use diesel;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use errors::*;
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
    let res = f(&log);
    let elapsed = precise_time_ns() - start;
    let (div, unit) = unit(elapsed);
    info!(log, "Finish"; "elapsed" => format!("{:.*}{}", 3, ((elapsed as f64) / div), unit));
    res
}

pub fn set_snapshot(log: &Logger, conn: &PgConnection, snapshot_id: Option<String>) -> Result<()> {
    match snapshot_id {
        Some(id) => {
            info!(log, "Setting snapshot"; "id" => id.as_str());
            diesel::sql_query("BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ").execute(conn)?;
            diesel::sql_query(format!("SET TRANSACTION SNAPSHOT '{}'", id)).execute(conn)?;
        }
        None => {
            info!(log, "Not setting snapshot ID");
        }
    }
    Ok(())
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
