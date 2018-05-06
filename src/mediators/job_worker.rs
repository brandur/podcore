use errors::*;
use http_requester::{HttpRequester, HttpRequesterFactory};
use jobs;
use mediators::common;
use model;
use model::insertable;
use schema;
use time_helpers;

use chan;
use chan::{Receiver, Sender};
use chrono::Utc;
use diesel;
use diesel::pg::upsert::excluded;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use serde_json;
use slog::Logger;
use std;
use std::thread;
use time::Duration;

pub struct Mediator {
    // Number of workers to use.
    //
    // Unlike the podcast crawler, this need not necessarily be tied directly to the number of
    // Postgres connections because not all jobs need hold an open connection while they're being
    // worked.
    pub num_workers: u32,

    pub pool:                   Pool<ConnectionManager<PgConnection>>,
    pub http_requester_factory: Box<HttpRequesterFactory>,

    // Tells the worker to run for only one batch of jobs instead of looping continuously forever.
    pub run_once: bool,
}

impl Mediator {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let mut workers = vec![];

        let res = {
            let (res_send, res_recv) = chan::sync(MAX_JOBS as usize);
            let (work_send, work_recv) = chan::sync(MAX_JOBS as usize);

            for i in 0..self.num_workers {
                let thread_name = common::thread_name(i);
                let log =
                    log.new(o!("thread" => thread_name.clone(), "num_threads" => self.num_workers));
                let pool_clone = self.pool.clone();
                let factory_clone = self.http_requester_factory.clone_box();
                let res_send_clone = res_send.clone();
                let work_recv_clone = work_recv.clone();

                workers.push(thread::Builder::new()
                    .name(thread_name)
                    .spawn(move || {
                        work(
                            &log,
                            &pool_clone,
                            &*factory_clone,
                            &work_recv_clone,
                            &res_send_clone,
                        )
                    })
                    .map_err(Error::from)?);
            }

            self.queue_jobs_and_record_results(log, &work_send, &res_recv)?

            // `work_send` is dropped, which unblocks our threads' select, passes them a
            // `None` result, and lets them to drop back to main. This only
            // occurs if `run_once` was set to `false` and the loop above was
            // broken.
        };

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        info!(log, "Finished working";
            "num_jobs" => res.num_jobs,
            "num_succeeded" => res.num_succeeded,
            "num_errored" => res.num_errored);
        Ok(res)
    }

    //
    // Steps
    //

    fn queue_jobs_and_record_results(
        &mut self,
        log: &Logger,
        work_send: &Sender<model::Job>,
        res_recv: &Receiver<JobResult>,
    ) -> Result<RunResult> {
        let log = log.new(o!("thread" => "control"));
        time_helpers::log_timed(
            &log.new(o!("step" => "queue_jobs_and_record_results")),
            |log| {
                let conn = &*(self.pool.get().map_err(Error::from))?;

                let mut res = RunResult {
                    num_jobs:      0,
                    num_succeeded: 0,
                    num_errored:   0,
                };
                loop {
                    let jobs = Self::select_jobs(log, &*conn)?;

                    let num_jobs = jobs.len();
                    res.num_jobs += num_jobs as i64;

                    if num_jobs == 0 {
                        if self.run_once {
                            break;
                        }

                        info!(log, "All jobs consumed -- sleeping"; "seconds" => SLEEP_SECONDS);
                        thread::sleep(std::time::Duration::from_secs(SLEEP_SECONDS));
                        continue;
                    }

                    for job in jobs.into_iter() {
                        work_send.send(job);
                    }

                    let (succeeded_ids, errored) = wait_results(res_recv, num_jobs);
                    res.num_succeeded += succeeded_ids.len() as i64;
                    res.num_errored += errored.len() as i64;

                    record_results(&log, &*conn, succeeded_ids, errored)?;

                    if self.run_once {
                        break;
                    }
                }

                Ok(res)
            },
        )
    }

    fn select_jobs(log: &Logger, conn: &PgConnection) -> Result<Vec<model::Job>> {
        // Helps us easily track from the logs whether the job queue is behind.
        let total_count: i64 = time_helpers::log_timed(
            &log.new(o!("step" => "count_jobs")),
            |_log| schema::job::table.count().first(conn),
        )?;
        info!(log, "Counted total jobs"; "num_jobs" => total_count);

        let res = time_helpers::log_timed(&log.new(o!("step" => "select_jobs")), |_log| {
            schema::job::table
                .filter(schema::job::live.eq(true))
                .filter(schema::job::try_at.le(Utc::now()))
                .limit(MAX_JOBS)
                .get_results(conn)
        })?;
        info!(log, "Selected jobs"; "num_jobs" => res.len());

        Ok(res)
    }
}

pub struct RunResult {
    /// Total number of jobs worked.
    ///
    /// This may include the same job multiple times if it errored and was
    /// processed across multiple runs.
    pub num_jobs: i64,

    /// Number of those jobs that succeeded.
    pub num_succeeded: i64,

    /// Number of those jobs that errored.
    ///
    /// A single job is allowed to error multiple times so this is not a count
    /// of the number of unique jobs that errored.
    pub num_errored: i64,
}

//
// Private constants
//

// The maximum number of times a job is allowed to fail before its `live` is
// set to `false` and it won't be worked again without manual intervention.
const MAX_ERRORS: i32 = 10;

// The maximum number of jobs to select in one batch.
const MAX_JOBS: i64 = 1000;

/// Number of seconds to sleep after finding no jobs to work.
///
/// In practice, especially at first, this will be roughly the average time
/// that a new user has to wait to get an activation email. I've tweaked the
/// timing a bit so that they get it faster. In pratice, doing a no-op loop
/// every 30 seconds or so won't be a huge tax on system resources.
const SLEEP_SECONDS: u64 = 30;

//
// Private structs
//

struct JobResult {
    job: model::Job,
    e:   Option<Error>,
}

//
// Private enums
//

//
// Private functions
//

/// Generates a new job which moves the given to its next error state.
///
/// Most of the time, this means increment its `num_errors` by one, and
/// scheduling a new time when it should be tried next. For jobs that have
/// failed many times, this means changing the state of their `live` field,
/// rendering them dead.
#[inline]
fn create_errored_job(job: model::Job) -> model::Job {
    let num_errors = job.num_errors + 1;

    // If a job has failed too many times, we flip its `live` bit, and it won't be
    // worked again without manual intervention.
    let live = num_errors < MAX_ERRORS;

    // Will contain a timestamp for the next time a job will be tried as long as
    // it's still live. Otherwise contains the time when we effectively set the
    // job to "permanently failed".
    let try_at = if live {
        Utc::now() + next_retry(num_errors)
    } else {
        Utc::now()
    };

    model::Job {
        id: job.id,
        args: job.args,
        created_at: job.created_at,
        live,
        name: job.name,
        num_errors,
        try_at,
    }
}

/// Gets the time that should elapsed before the next time a job is tried.
///
/// This is based on an exponential backoff formula cargo-culted from other job
/// libraries.
#[inline]
fn next_retry(num_errors: i32) -> Duration {
    Duration::seconds((num_errors as i64).pow(4) + 3)
}

/// Records the results of a run of jobs.
///
/// This is all batched together for efficient insertion, with the downside
/// being that we won't be able to start a new batch until the slowest job in
/// the preceding batch has finished running. Batches are large and my jobs are
/// short-lived, so this is okay for my purposes.
///
/// Any job exceptions for succeeded jobs are deleted, the succeeded jobs
/// themselves are deleted, errored jobs are upserted with new scheduling
/// status, and finally job exceptions are upserted for failed jobs.
#[inline]
fn record_results(
    log: &Logger,
    conn: &PgConnection,
    succeeded_ids: Vec<i64>,
    errored: Vec<JobResult>,
) -> Result<()> {
    time_helpers::log_timed(&log.new(o!("step" => "record_results")), |log| {
        conn.transaction::<_, Error, _>(|| record_results_inner(log, conn, succeeded_ids, errored))
    })
}

/// The same as `record_results`, but allows us to avoid some indentation.
///
/// This function must run in a transaction.
#[inline]
fn record_results_inner(
    log: &Logger,
    conn: &PgConnection,
    succeeded_ids: Vec<i64>,
    errored: Vec<JobResult>,
) -> Result<()> {
    info!(log, "Recording results";
        "num_succeeded" => succeeded_ids.len(), "num_errored" => errored.len());

    // Delete any errors that might have been produced for this job
    time_helpers::log_timed(&log.new(o!("step" => "delete_job_exceptions")), |_log| {
        diesel::delete(
            schema::job_exception::table
                .filter(schema::job_exception::job_id.eq_any(&succeeded_ids)),
        ).execute(conn)
            .chain_err(|| "Error deleting job exceptions for successful jobs")
    })?;

    time_helpers::log_timed(&log.new(o!("step" => "delete_jobs")), |_log| {
        diesel::delete(schema::job::table.filter(schema::job::id.eq_any(&succeeded_ids)))
            .execute(conn)
            .chain_err(|| "Error deleting succeeded jobs")
    })?;

    // Return early if we only had successes. This is the happy case that will
    // hopefully be occurring most of the time.
    if errored.is_empty() {
        return Ok(());
    }

    let mut errors: Vec<insertable::JobException> = Vec::with_capacity(errored.len());
    let mut jobs: Vec<model::Job> = Vec::with_capacity(errored.len());
    let now = Utc::now();

    for job_result in errored.into_iter() {
        errors.push(insertable::JobException {
            errors:      error_strings(&job_result.e.unwrap()),
            job_id:      job_result.job.id,
            occurred_at: now,
        });
        jobs.push(job_result.job);
    }

    // Re-insert failed jobs. With upsert we end up overwriting all the existing
    // records with new status information.
    time_helpers::log_timed(&log.new(o!("step" => "upsert_jobs")), |_log| {
        diesel::insert_into(schema::job::table)
            .values(&jobs)
            .on_conflict(schema::job::id)
            .do_update()
            .set((
                schema::job::live.eq(excluded(schema::job::live)),
                schema::job::num_errors.eq(excluded(schema::job::num_errors)),
                schema::job::try_at.eq(excluded(schema::job::try_at)),
            ))
            .execute(conn)
            .chain_err(|| "Error upserting jobs")
    })?;

    // This may be the second time (or more) that any job has failed, so we upsert
    // exceptions.
    time_helpers::log_timed(&log.new(o!("step" => "upsert_job_exceptions")), |_log| {
        diesel::insert_into(schema::job_exception::table)
            .values(&errors)
            .on_conflict(schema::job_exception::job_id)
            .do_update()
            .set((
                schema::job_exception::errors.eq(excluded(schema::job_exception::errors)),
                schema::job_exception::occurred_at.eq(excluded(schema::job_exception::occurred_at)),
            ))
            .execute(conn)
            .chain_err(|| "Error upserting job exceptions")
    })?;

    Ok(())
}

/// Waits on the job result channel for the expected number of results to come
/// back and sorts them appropriately into vectors of succeeded IDs and errors.
#[inline]
fn wait_results(res_recv: &Receiver<JobResult>, num_jobs: usize) -> (Vec<i64>, Vec<JobResult>) {
    let mut succeeded_ids: Vec<i64> = Vec::with_capacity(num_jobs);
    let mut errored: Vec<JobResult> = Vec::new();
    for _i in 0..num_jobs {
        match res_recv.recv().unwrap() {
            JobResult {
                job: model::Job { id, .. },
                e: None,
                ..
            } => succeeded_ids.push(id),
            res => errored.push(res),
        }
    }
    (succeeded_ids, errored)
}

/// A single thread's work loop.
fn work(
    log: &Logger,
    pool: &Pool<ConnectionManager<PgConnection>>,
    http_requester_factory: &HttpRequesterFactory,
    work_recv: &Receiver<model::Job>,
    res_send: &Sender<JobResult>,
) -> Result<()> {
    let requester = http_requester_factory.create();

    loop {
        chan_select! {
            work_recv.recv() -> job => {
                let job: model::Job = match job {
                    Some(t) => t,
                    None => {
                        debug!(log, "Received empty data over channel -- dropping");
                        break;
                    }
                };

                let res = time_helpers::log_timed(&log.new(o!("step" => "work_job", "job_id" => job.id)), |log| {
                    work_job(log, pool, &*requester, &job)
                });

                debug!(log, "Worked a job");

                match res {
                    Ok(()) => res_send.send(JobResult { job, e: None }),
                    Err(e) => res_send.send(JobResult { job: create_errored_job(job), e: Some(e) }),
                }
            }
        }
    }

    Ok(())
}

/// Work a single job.
fn work_job(
    log: &Logger,
    _pool: &Pool<ConnectionManager<PgConnection>>,
    requester: &HttpRequester,
    job: &model::Job,
) -> Result<()> {
    match job.name.as_str() {
        jobs::no_op::NAME => jobs::no_op::Job {
            args: serde_json::from_value(job.args.clone())?,
        }.run(log),
        jobs::verification_mailer::NAME => jobs::verification_mailer::Job {
            args:      serde_json::from_value(job.args.clone())?,
            requester: requester,
        }.run(log),
        _ => Err(error::job_unknown(job.name.clone())),
    }
}

#[cfg(test)]
mod tests {
    use http_requester::{HttpRequesterFactoryPassThrough, HttpRequesterPassThrough};
    use mediators::job_worker::*;
    use test_helpers;

    use r2d2::{Pool, PooledConnection};
    use std::sync::Arc;

    #[test]
    #[ignore]
    fn test_job_worker_work() {
        let mut bootstrap = TestBootstrapWithClean::new();

        // Insert lots of jobs to be worked
        let num_jobs = (NUM_WORKERS as i64) * 10;

        // We're only going to do one pass so make sure it's possible to work all our
        // test jobs in one.
        assert!(num_jobs <= MAX_JOBS);

        let mut jobs: Vec<insertable::Job> = Vec::with_capacity(num_jobs as usize);
        for _i in 0..num_jobs {
            jobs.push(insertable::Job {
                args:   json!({"message": "hello"}),
                name:   jobs::no_op::NAME.to_owned(),
                try_at: Utc::now(),
            });
        }
        diesel::insert_into(schema::job::table)
            .values(&jobs)
            .execute(&*bootstrap.conn)
            .unwrap();

        debug!(&bootstrap.log, "Finished setup (starting the real test)";
            "num_jobs" => num_jobs);

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(num_jobs, res.num_jobs);
            assert_eq!(num_jobs, res.num_succeeded);
            assert_eq!(0, res.num_errored);
        }

        // All jobs should have been deleted after they were worked.
        assert_eq!(0, count_jobs(&*bootstrap.conn));
    }

    #[test]
    #[ignore]
    fn test_job_worker_error() {
        let mut bootstrap = TestBootstrapWithClean::new();

        //
        // Part 1: Insert a job with an unknown name that the worker can't handle.
        // Verify that the job isn't worked and an exception is inserted.
        //

        let job: model::Job = diesel::insert_into(schema::job::table)
            .values(&insertable::Job {
                args:   json!({"message": "hello"}),
                name:   "bad_job".to_owned(),
                try_at: Utc::now(),
            })
            .get_result(&*bootstrap.conn)
            .unwrap();

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(1, res.num_jobs);
            assert_eq!(0, res.num_succeeded);
            assert_eq!(1, res.num_errored);
        }

        // We should have our job leftover, along with a job exception.
        assert_eq!(1, count_jobs(&*bootstrap.conn));
        assert_eq!(1, count_job_exceptions(&*bootstrap.conn));

        //
        // Part 2: Correct the job's name. See it worked successfully.
        //

        diesel::update(schema::job::table.filter(schema::job::id.eq(job.id)))
            .set((
                schema::job::name.eq(jobs::no_op::NAME),
                schema::job::try_at.eq(Utc::now()),
            ))
            .execute(&*bootstrap.conn)
            .unwrap();

        {
            let (mut mediator, log) = bootstrap.mediator();
            let res = mediator.run(&log).unwrap();
            assert_eq!(1, res.num_jobs);
            assert_eq!(1, res.num_succeeded);
            assert_eq!(0, res.num_errored);
        }

        // All jobs and exceptions are now gone.
        assert_eq!(0, count_jobs(&*bootstrap.conn));
        assert_eq!(0, count_job_exceptions(&*bootstrap.conn));
    }

    #[test]
    fn test_job_worker_create_errored_job() {
        // Initial transition into errored state
        let job = create_errored_job(new_job());
        assert!(job.live);
        assert_eq!(1, job.num_errors);

        // Test transition from live to dead because we're already at the threshold for
        // maximum retries
        let mut job = new_job();
        job.num_errors = MAX_ERRORS;

        let job = create_errored_job(job);
        assert_eq!(false, job.live);
        assert_eq!(MAX_ERRORS + 1, job.num_errors);
    }

    #[test]
    fn test_job_worker_next_retry() {
        assert_eq!(Duration::seconds(4), next_retry(1));
        assert_eq!(Duration::seconds(19), next_retry(2));
        assert_eq!(Duration::seconds(84), next_retry(3));
        assert_eq!(Duration::seconds(259), next_retry(4));
        assert_eq!(Duration::seconds(628), next_retry(5));
    }

    #[test]
    fn test_job_worker_work_job() {
        let bootstrap = TestBootstrap::new();

        work_job(
            &bootstrap.log,
            &bootstrap.pool,
            &HttpRequesterPassThrough {
                data: Arc::new(Vec::new()),
            },
            &new_job(),
        ).unwrap();
    }

    //
    // Private constants/types/functions
    //

    const NUM_WORKERS: u32 = 10;

    struct TestBootstrap {
        _common: test_helpers::CommonTestBootstrap,
        log:     Logger,
        pool:    Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrap {
        fn new() -> Self {
            TestBootstrap {
                _common: test_helpers::CommonTestBootstrap::new(),
                log:     test_helpers::log_sync(),
                pool:    test_helpers::pool(),
            }
        }
    }

    /// Similar to `TestBootstrap` above, but cleans the database after it's
    /// run.
    ///
    /// Not suitable for running in the main test suite because it doesn't play
    /// well with parallelism. Use only for tests that are run
    /// single-threaded and marked with `ignore`.
    struct TestBootstrapWithClean {
        _common: test_helpers::CommonTestBootstrap,
        conn:    PooledConnection<ConnectionManager<PgConnection>>,
        log:     Logger,
        pool:    Pool<ConnectionManager<PgConnection>>,
    }

    impl TestBootstrapWithClean {
        fn new() -> Self {
            let pool = test_helpers::pool();
            let conn = pool.get().map_err(Error::from).unwrap();
            TestBootstrapWithClean {
                _common: test_helpers::CommonTestBootstrap::new(),
                conn:    conn,
                log:     test_helpers::log_sync(),
                pool:    pool,
            }
        }

        fn mediator(&mut self) -> (Mediator, Logger) {
            (
                Mediator {
                    num_workers:            NUM_WORKERS,
                    pool:                   self.pool.clone(),
                    http_requester_factory: Box::new(HttpRequesterFactoryPassThrough {
                        data: Arc::new(Vec::new()),
                    }),
                    run_once:               true,
                },
                self.log.clone(),
            )
        }
    }

    impl Drop for TestBootstrapWithClean {
        fn drop(&mut self) {
            test_helpers::clean_database(&self.log, &*self.conn);
        }
    }

    fn count_jobs(conn: &PgConnection) -> i64 {
        schema::job::table.count().first(conn).unwrap()
    }

    fn count_job_exceptions(conn: &PgConnection) -> i64 {
        schema::job_exception::table.count().first(conn).unwrap()
    }

    fn new_job() -> model::Job {
        model::Job {
            id:         0,
            args:       json!({"message": "hello"}),
            created_at: Utc::now(),
            live:       true,
            name:       jobs::no_op::NAME.to_owned(),
            num_errors: 0,
            try_at:     Utc::now(),
        }
    }
}
