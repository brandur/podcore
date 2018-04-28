use errors::*;
use http_requester::{HttpRequester, HttpRequesterFactory};
use jobs;
use mediators::common;
use model;
use schema;
use time_helpers;

use chan;
use chan::{Receiver, Sender};
use chrono::Utc;
use diesel::pg::PgConnection;
use diesel::prelude::*;
use r2d2::Pool;
use r2d2_diesel::ConnectionManager;
use serde_json;
use slog::Logger;
use std::thread;
use std::time::Duration;

pub struct Mediator {
    // Number of workers to use.
    //
    // Unlike the podcast crawler, this need not necessarily be tied directly to the number of
    // Postgres connections because not all jobs need hold an open connection while they're being
    // worked.
    pub num_workers: u32,

    pub pool:                   Pool<ConnectionManager<PgConnection>>,
    pub http_requester_factory: Box<HttpRequesterFactory>,

    // Tells the worker to run forever rather than fall through after fetching one batch of jobs.
    pub run_forever: bool,
}

impl Mediator {
    pub fn run(&mut self, log: &Logger) -> Result<RunResult> {
        time_helpers::log_timed(&log.new(o!("step" => file!())), |log| self.run_inner(log))
    }

    pub fn run_inner(&mut self, log: &Logger) -> Result<RunResult> {
        let mut workers = vec![];

        let num_jobs = {
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

            self.queue_and_report_jobs(log, &work_send, &res_recv)?

            // `work_send` is dropped, which unblocks our threads' select, passes them a
            // `None` result, and lets them to drop back to main. This only
            // occurs if `run_forever` was set to `false` and the loop above
            // was broken.
        };

        // Wait for threads to rejoin
        for worker in workers {
            let _ = worker.join();
        }

        info!(log, "Finished working"; "num_jobs" => num_jobs);
        Ok(RunResult { num_jobs })
    }

    //
    // Steps
    //

    fn queue_and_report_jobs(
        &mut self,
        log: &Logger,
        work_send: &Sender<model::Job>,
        res_recv: &Receiver<JobResult>,
    ) -> Result<i64> {
        let log = log.new(o!("thread" => "control"));
        time_helpers::log_timed(&log.new(o!("step" => "queue_and_report_jobs")), |log| {
            let conn = &*(self.pool.get().map_err(Error::from))?;

            let mut total_num_jobs = 0i64;
            loop {
                let jobs = Self::select_jobs(log, &*conn)?;

                let num_jobs = jobs.len();
                total_num_jobs += num_jobs as i64;

                if num_jobs == 0 {
                    info!(log, "All jobs consumed -- sleeping";
                        "seconds" => SLEEP_SECONDS);
                    thread::sleep(Duration::from_secs(SLEEP_SECONDS));

                    if self.run_forever {
                        break;
                    }

                    continue;
                }

                for job in jobs.into_iter() {
                    work_send.send(job);
                }

                let mut job_results = Vec::with_capacity(num_jobs);
                for _i in 0..(num_jobs - 1) {
                    job_results.push(res_recv.recv());
                }

                if !self.run_forever {
                    break;
                }
            }

            Ok(total_num_jobs)
        })
    }

    fn select_jobs(log: &Logger, conn: &PgConnection) -> Result<Vec<model::Job>> {
        let res = time_helpers::log_timed(&log.new(o!("step" => "select_jobs")), |_log| {
            schema::job::table
                .filter(schema::job::live.eq(true))
                .filter(schema::job::try_at.le(Utc::now()))
                .limit(MAX_JOBS)
                .get_results(conn)
        })?;

        Ok(res)
    }
}

//
// Private constants
//

// The maximum number of jobs to select in one batch.
const MAX_JOBS: i64 = 100;

// Number of seconds to sleep after finding no jobs to work.
const SLEEP_SECONDS: u64 = 60;

//
// Private structs
//

pub struct RunResult {
    num_jobs: i64,
}

//
// Private enums
//

//
// Private functions
//

// A single thread's work loop.
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
                let job_id = job.id;

                // TODO: Handle error -- don't crash
                let res = time_helpers::log_timed(&log.new(o!("step" => "work_job", "job_id" => job.id)), |log| {
                    work_job(log, pool, &*requester, job)
                });

                debug!(log, "Worked a job");

                match res {
                    Ok(()) => res_send.send(JobResult { id: job_id, e: None }),
                    Err(e) => res_send.send(JobResult { id: job_id, e: Some(e) }),
                }
            }
        }
    }

    Ok(())
}

struct JobResult {
    id: i64,
    e:  Option<Error>,
}

// Working a single job.
fn work_job(
    log: &Logger,
    _pool: &Pool<ConnectionManager<PgConnection>>,
    requester: &HttpRequester,
    job: model::Job,
) -> Result<()> {
    match job.name.as_str() {
        jobs::verification_mailer::NAME => jobs::verification_mailer::Job {
            args:      serde_json::from_value(job.args)?,
            requester: requester,
        }.run(log),
        _ => panic!("Job not covered!"),
    }
}
