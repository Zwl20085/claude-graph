//! Process-tree lifetime on Windows: every `claude` child is put in its own Job
//! Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Dropping the [`Job`] (end
//! of the turn, or the app exiting or crashing, which closes the handle) kills
//! every process still in it, including grandchildren that inherited the job.
//! The explicit cancel path still uses `taskkill /T /F` (runner::kill_tree).
//!
//! A process the child starts before [`Job::assign`] runs is not in the job;
//! the window is the few milliseconds between spawn and assignment.
//! On other platforms [`Job`] is a no-op; the runner kills the process group.

/// Owns the job handle; dropping it kills what is left in the job.
pub struct Job {
    #[cfg(windows)]
    handle: isize,
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    use super::Job;

    impl Job {
        /// Put `child` in a new kill-on-close job. `None` (logged) if any step fails.
        pub fn assign(child: &tokio::process::Child) -> Option<Job> {
            Self::assign_handle(child.raw_handle()?)
        }

        /// Put the process behind the raw handle `process` (alive, owned by the
        /// caller for the duration of the call) in a new kill-on-close job.
        pub fn assign_handle(process: std::os::windows::io::RawHandle) -> Option<Job> {
            let process = process as HANDLE;
            // SAFETY: plain Win32 calls; `job` is checked for null, `info` is a
            // zero-initialised POD struct of the size passed, and `process` is the
            // live child's handle owned by the caller for the duration of this call.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    log::warn!(
                        "CreateJobObjectW failed: {}",
                        std::io::Error::last_os_error()
                    );
                    return None;
                }
                let owned = Job {
                    handle: job as isize,
                };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                let set = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    size,
                );
                if set == 0 {
                    log::warn!(
                        "SetInformationJobObject failed: {}",
                        std::io::Error::last_os_error()
                    );
                    return None;
                }
                if AssignProcessToJobObject(job, process) == 0 {
                    log::warn!(
                        "AssignProcessToJobObject failed: {}",
                        std::io::Error::last_os_error()
                    );
                    return None;
                }
                Some(owned)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: `handle` came from CreateJobObjectW and is closed exactly once.
            unsafe {
                CloseHandle(self.handle as HANDLE);
            }
        }
    }
}

#[cfg(not(windows))]
impl Job {
    /// No job objects outside Windows; the process group is used instead.
    pub fn assign(_child: &tokio::process::Child) -> Option<Job> {
        None
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A child in the job dies when the job is dropped.
    #[tokio::test]
    async fn dropping_the_job_kills_the_child() {
        let mut child = tokio::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let job = Job::assign(&child).expect("job assigned");
        let t0 = std::time::Instant::now();
        drop(job);
        // ping would run ~29 s; closing the job ends it at once (exit code 0).
        tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .expect("child killed by job close")
            .unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    }
}
