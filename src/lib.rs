extern crate bits;
extern crate comedy;
extern crate failure;
extern crate failure_derive;
extern crate guid_win;

pub mod bits_protocol;

mod in_process;

use std::convert;
use std::ffi;

use bits_protocol::*;
use failure::Fail;

pub use bits::status::{BitsErrorContext, BitsJobState};
pub use bits::{BitsJobError, BitsJobProgress, BitsJobStatus, BitsProxyUsage};
pub use comedy::Error as ComedyError;
pub use guid_win::Guid;

// These errors would come from a Local Service client, this structure properly lives in the
// crate that deals with named pipes.
#[derive(Clone, Debug, Eq, Fail, PartialEq)]
pub enum PipeError {
    #[fail(display = "Pipe is not connected")]
    NotConnected,
    #[fail(display = "Operation timed out")]
    Timeout,
    #[fail(display = "Should have written {} bytes, wrote {}", _0, _1)]
    WriteCount(usize, u32),
    #[fail(display = "Windows API error")]
    Api(#[fail(cause)] ComedyError),
}

impl convert::From<ComedyError> for PipeError {
    fn from(err: ComedyError) -> PipeError {
        PipeError::Api(err)
    }
}

pub use PipeError as Error;

pub enum BitsClient {
    /// The InProcess variant does all BITS calls in-process.
    InProcess(in_process::InProcessClient),
    // Space is reserved here for the LocalService variant, which will work through an external
    // process running as Local Service.
}

use BitsClient::*;

/// A client object for interfacing with BITS.
///
/// Methods on `BitsClient` usually return a `Result<Result<_, xyzFailure>>`. The outer `Result`
/// is `Err` if there was a communication error in sending the associated command or receiving
/// its response. Currently this is always `Ok` as all clients are in-process. The inner
/// `Result` is `Err` if there was an error executing the command.
impl BitsClient {
    /// Create an in-process `BitsClient`.
    /// `job_name` will be used when creating jobs, and this `BitsClient` can only be used to
    /// manipulate jobs with that name.
    /// `save_path_prefix` will be prepended to the local `save_path` given to `start_job()`
    pub fn new(
        job_name: ffi::OsString,
        save_path_prefix: ffi::OsString,
    ) -> Result<BitsClient, Error> {
        Ok(InProcess(in_process::InProcessClient::new(
            job_name,
            save_path_prefix,
        )?))
    }

    /// Start a job to download a single file at `url` to local path `save_path` (relative to the
    /// `save_path_prefix` given when constructing the `BitsClient`).
    ///
    /// `proxy_usage` determines what proxy will be used.
    ///
    /// When a successful result `Ok(result)` is returned, `result.0.guid` is the id for the
    /// new job, and `result.1` is a monitor client that can be polled for periodic updates,
    /// returning a result approximately once per `monitor_interval_millis` milliseconds.
    pub fn start_job(
        &mut self,
        url: ffi::OsString,
        save_path: ffi::OsString,
        proxy_usage: BitsProxyUsage,
        monitor_interval_millis: u32,
    ) -> Result<Result<(StartJobSuccess, BitsMonitorClient), StartJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client
                .start_job(url, save_path, proxy_usage, monitor_interval_millis)
                .map(|(success, monitor)| (success, BitsMonitorClient::InProcess(monitor)))),
        }
    }

    /// Start monitoring the job with id `guid` approximately once per `monitor_interval_millis`
    /// milliseconds.
    pub fn monitor_job(
        &mut self,
        guid: Guid,
        interval_millis: u32,
    ) -> Result<Result<BitsMonitorClient, MonitorJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client
                .monitor_job(guid, interval_millis)
                .map(|monitor| BitsMonitorClient::InProcess(monitor))),
        }
    }

    /// Suspend job `guid`.
    pub fn suspend_job(&mut self, guid: Guid) -> Result<Result<(), SuspendJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.suspend_job(guid)),
        }
    }

    /// Resume job `guid`.
    pub fn resume_job(&mut self, guid: Guid) -> Result<Result<(), ResumeJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.resume_job(guid)),
        }
    }

    /// Set the priority of job `guid`.
    ///
    /// `foreground == true` will set the priority to `BG_JOB_PRIORITY_FOREGROUND`,
    /// `false` will use the default `BG_JOB_PRIORITY_NORMAL`.
    /// See the Microsoft documentation for `BG_JOB_PRIORITY` for details.
    ///
    /// This is usually not needed if you have a `BitsMonitorClient`, which will boost the
    /// priority to foreground as long as it is running, and return the priority to normal when
    /// it stops.
    pub fn set_job_priority(
        &mut self,
        guid: Guid,
        foreground: bool,
    ) -> Result<Result<(), SetJobPriorityFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.set_job_priority(guid, foreground)),
        }
    }

    /// Change the update interval for an ongoing monitor of job `guid`.
    pub fn set_update_interval(
        &mut self,
        guid: Guid,
        interval_millis: u32,
    ) -> Result<Result<(), SetUpdateIntervalFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.set_update_interval(guid, interval_millis)),
        }
    }

    /// Stop any ongoing monitor for job `guid`.
    pub fn stop_update(
        &mut self,
        guid: Guid,
    ) -> Result<Result<(), SetUpdateIntervalFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.stop_update(guid)),
        }
    }

    /// Complete the job `guid`.
    ///
    /// This also stops any ongoing monitor for the job.
    pub fn complete_job(&mut self, guid: Guid) -> Result<Result<(), CompleteJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.complete_job(guid)),
        }
    }

    /// Cancel the job `guid`.
    ///
    /// This also stops any ongoing monitor for the job.
    pub fn cancel_job(&mut self, guid: Guid) -> Result<Result<(), CancelJobFailure>, Error> {
        match self {
            InProcess(client) => Ok(client.cancel_job(guid)),
        }
    }
}

/// A `BitsMonitorClient` is the client side of a monitor for a particular BITS job.
pub enum BitsMonitorClient {
    InProcess(in_process::InProcessMonitor),
}

impl BitsMonitorClient {
    /// `get_status` will return a result approximately every `monitor_interval_millis`
    /// milliseconds, but in case a result isn't available within `timeout_millis` milliseconds
    /// this will return `Err(Error::Timeout)`. Any `Err` returned is usually a sign that the
    /// connection has been dropped.
    ///
    /// If there is an error or the job transfer completes, a result may be available sooner than
    /// the monitor interval.
    ///
    /// While the `BitsMonitorClient` is running the job's priority will be boosted to foreground,
    /// if it is stopped or dropped the priority will be returned to background, if possible.
    pub fn get_status(&mut self, timeout_millis: u32) -> Result<BitsJobStatus, Error> {
        match self {
            BitsMonitorClient::InProcess(client) => client.get_status(timeout_millis),
        }
    }
}
