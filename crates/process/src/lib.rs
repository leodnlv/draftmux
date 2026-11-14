//! Child processes started in a group of their own, so a process and
//! everything it started can be ended together: a process group on Unix, a
//! job object on Windows.

use std::io;
use std::process::{Child, Command};

/// The group of a process started with [`spawn`]. Clones name the same group.
/// Dropping it leaves the processes running.
#[derive(Clone)]
pub struct Group(imp::Group);

/// Spawns `command` as the first member of a new group. On Windows the
/// process gets no console window, and it joins the group before it runs, so
/// nothing it starts escapes the group.
pub fn spawn(command: &mut Command) -> io::Result<(Child, Group)> {
    imp::spawn(command).map(|(child, group)| (child, Group(group)))
}

impl Group {
    /// Kills every process in the group at once.
    pub fn kill(&self) {
        self.0.kill();
    }

    /// Asks the group's first process to exit: SIGTERM on Unix. Windows has
    /// no such request, so this does nothing there; closing the process's
    /// stdin is the portable request.
    pub fn terminate(&self) {
        self.0.terminate();
    }
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    #[derive(Clone)]
    pub struct Group {
        /// The first process's pid, which is also the group id. It stays
        /// reserved while the group has members or the process is unreaped.
        pid: libc::pid_t,
    }

    pub fn spawn(command: &mut Command) -> io::Result<(Child, Group)> {
        let child = command.process_group(0).spawn()?;
        let pid = child.id() as libc::pid_t;
        Ok((child, Group { pid }))
    }

    impl Group {
        pub fn kill(&self) {
            // SAFETY: a plain syscall on our own child's process group.
            unsafe { libc::killpg(self.pid, libc::SIGKILL) };
        }

        pub fn terminate(&self) {
            // SAFETY: a plain syscall on our own child.
            unsafe { libc::kill(self.pid, libc::SIGTERM) };
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::Arc;

    use windows_sys::Win32::Foundation::{FALSE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    #[derive(Clone)]
    pub struct Group {
        job: Arc<OwnedHandle>,
    }

    pub fn spawn(command: &mut Command) -> io::Result<(Child, Group)> {
        // SAFETY: no attributes and no name create a private job.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new handle that nothing else owns.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        // Suspended, so the process joins the job before it can start
        // anything that would be outside it.
        let mut child = command
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()?;
        let joined = assign(&job, &child).and_then(|()| resume(child.id()));
        if let Err(err) = joined {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        Ok((child, Group { job: Arc::new(job) }))
    }

    fn assign(job: &OwnedHandle, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are open for the call.
        let ok = unsafe {
            AssignProcessToJobObject(
                job.as_raw_handle() as HANDLE,
                child.as_raw_handle() as HANDLE,
            )
        };
        if ok == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Resumes the threads of the suspended process `pid`: its initial
    /// thread, the only one a suspended process has.
    fn resume(pid: u32) -> io::Result<()> {
        // SAFETY: a snapshot of all threads, closed by `OwnedHandle`.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        let mut resumed = false;
        // SAFETY: `entry` is a valid THREADENTRY32 with its size set.
        let mut more = unsafe { Thread32First(snapshot.as_raw_handle() as HANDLE, &mut entry) };
        while more != FALSE {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: opens a thread of our own child.
                let thread =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, FALSE, entry.th32ThreadID) };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                // SAFETY: the handle has THREAD_SUSPEND_RESUME access.
                if unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                resumed = true;
            }
            more = unsafe { Thread32Next(snapshot.as_raw_handle() as HANDLE, &mut entry) };
        }
        if !resumed {
            return Err(io::Error::other("the new process has no thread to resume"));
        }
        Ok(())
    }

    impl Group {
        pub fn kill(&self) {
            // SAFETY: the job handle is open while `self` exists.
            unsafe { TerminateJobObject(self.job.as_raw_handle() as HANDLE, 1) };
        }

        pub fn terminate(&self) {}
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// The platform shell running `unix` or `windows`.
    fn shell(unix: &str, windows: &str) -> Command {
        let (program, flag, script) = if cfg!(windows) {
            ("cmd", "/c", windows)
        } else {
            ("/bin/sh", "-c", unix)
        };
        let mut command = Command::new(program);
        command.args([flag, script]);
        command
    }

    #[test]
    fn kill_ends_the_process_and_what_it_started() {
        // A child of the shell holds stdout open for ten minutes.
        let mut command = shell("sleep 600 & wait", "ping -n 600 127.0.0.1");
        command.stdin(Stdio::null()).stdout(Stdio::piped());
        let (mut child, group) = spawn(&mut command).unwrap();
        let mut stdout = child.stdout.take().unwrap();
        // stdout reaches end of file only once every process holding it,
        // the shell's child included, has exited.
        let (done, eof) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            let _ = done.send(());
        });
        std::thread::sleep(Duration::from_millis(300));
        group.kill();
        child.wait().unwrap();
        eof.recv_timeout(Duration::from_secs(5))
            .expect("the shell's child was killed with the group");
    }

    #[test]
    fn spawned_process_runs() {
        let mut command = shell("echo ok", "echo ok");
        command.stdout(Stdio::piped());
        let (mut child, _group) = spawn(&mut command).unwrap();
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(output.trim(), "ok");
    }
}
