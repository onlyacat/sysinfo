// Take a look at the license at the top of the repository in the LICENSE file.

use crate::{DiskUsage, Pid, ProcessExt, ProcessStatus, Signal};

use std::fmt;
use std::path::{Path, PathBuf};

use super::utils::{get_sys_value_str, Wrap};

#[doc(hidden)]
impl From<libc::c_char> for ProcessStatus {
    fn from(status: libc::c_char) -> ProcessStatus {
        match status {
            libc::SIDL => ProcessStatus::Idle,
            libc::SRUN => ProcessStatus::Run,
            libc::SSLEEP => ProcessStatus::Sleep,
            libc::SSTOP => ProcessStatus::Stop,
            libc::SZOMB => ProcessStatus::Zombie,
            libc::SWAIT => ProcessStatus::Dead,
            libc::SLOCK => ProcessStatus::LockBlocked,
            x => ProcessStatus::Unknown(x as _),
        }
    }
}

impl fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match *self {
            ProcessStatus::Idle => "Idle",
            ProcessStatus::Run => "Runnable",
            ProcessStatus::Sleep => "Sleeping",
            ProcessStatus::Stop => "Stopped",
            ProcessStatus::Zombie => "Zombie",
            ProcessStatus::Dead => "Dead",
            ProcessStatus::LockBlocked => "LockBlocked",
            _ => "Unknown",
        })
    }
}

#[doc = include_str!("../../md_doc/process.md")]
pub struct Process {
    pub(crate) name: String,
    pub(crate) cmd: Vec<String>,
    pub(crate) exe: PathBuf,
    pub(crate) pid: Pid,
    parent: Option<Pid>,
    pub(crate) environ: Vec<String>,
    pub(crate) cwd: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) memory: u64,
    pub(crate) virtual_memory: u64,
    pub(crate) updated: bool,
    cpu_usage: f32,
    start_time: u64,
    run_time: u64,
    pub(crate) status: ProcessStatus,
    /// User id of the process owner.
    pub uid: libc::uid_t,
    /// Group id of the process owner.
    pub gid: libc::gid_t,
    read_bytes: u64,
    old_read_bytes: u64,
    written_bytes: u64,
    old_written_bytes: u64,
}

impl ProcessExt for Process {
    fn new(pid: Pid, parent: Option<Pid>, start_time: u64) -> Process {
        Process {
            name: String::new(),
            cmd: Vec::new(),
            exe: PathBuf::new(),
            pid,
            parent,
            environ: Vec::new(),
            cwd: PathBuf::new(),
            root: PathBuf::new(),
            memory: 0,
            virtual_memory: 0,
            updated: false,
            cpu_usage: 0.,
            start_time,
            run_time: 0,
            status: ProcessStatus::Unknown(0),
            uid: 0,
            gid: 0,
            read_bytes: 0,
            old_read_bytes: 0,
            written_bytes: 0,
            old_written_bytes: 0,
        }
    }

    fn kill(&self, signal: Signal) -> bool {
        let c_signal = match signal {
            Signal::Hangup => libc::SIGHUP,
            Signal::Interrupt => libc::SIGINT,
            Signal::Quit => libc::SIGQUIT,
            Signal::Illegal => libc::SIGILL,
            Signal::Trap => libc::SIGTRAP,
            Signal::Abort => libc::SIGABRT,
            Signal::IOT => libc::SIGIOT,
            Signal::Bus => libc::SIGBUS,
            Signal::FloatingPointException => libc::SIGFPE,
            Signal::Kill => libc::SIGKILL,
            Signal::User1 => libc::SIGUSR1,
            Signal::Segv => libc::SIGSEGV,
            Signal::User2 => libc::SIGUSR2,
            Signal::Pipe => libc::SIGPIPE,
            Signal::Alarm => libc::SIGALRM,
            Signal::Term => libc::SIGTERM,
            Signal::Child => libc::SIGCHLD,
            Signal::Continue => libc::SIGCONT,
            Signal::Stop => libc::SIGSTOP,
            Signal::TSTP => libc::SIGTSTP,
            Signal::TTIN => libc::SIGTTIN,
            Signal::TTOU => libc::SIGTTOU,
            Signal::Urgent => libc::SIGURG,
            Signal::XCPU => libc::SIGXCPU,
            Signal::XFSZ => libc::SIGXFSZ,
            Signal::VirtualAlarm => libc::SIGVTALRM,
            Signal::Profiling => libc::SIGPROF,
            Signal::Winch => libc::SIGWINCH,
            Signal::IO => libc::SIGIO,
            Signal::Sys => libc::SIGSYS,
            Signal::Poll | Signal::Power => return false,
        };
        unsafe { libc::kill(self.pid, c_signal) == 0 }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn cmd(&self) -> &[String] {
        &self.cmd
    }

    fn exe(&self) -> &Path {
        self.exe.as_path()
    }

    fn pid(&self) -> Pid {
        self.pid
    }

    fn environ(&self) -> &[String] {
        &self.environ
    }

    fn cwd(&self) -> &Path {
        self.cwd.as_path()
    }

    fn root(&self) -> &Path {
        self.root.as_path()
    }

    fn memory(&self) -> u64 {
        self.memory
    }

    fn virtual_memory(&self) -> u64 {
        self.virtual_memory
    }

    fn parent(&self) -> Option<Pid> {
        self.parent
    }

    fn status(&self) -> ProcessStatus {
        self.status
    }

    fn start_time(&self) -> u64 {
        self.start_time
    }

    fn cpu_usage(&self) -> f32 {
        self.cpu_usage
    }

    fn disk_usage(&self) -> DiskUsage {
        DiskUsage {
            written_bytes: self.written_bytes.saturating_sub(self.old_written_bytes),
            total_written_bytes: self.written_bytes,
            read_bytes: self.read_bytes.saturating_sub(self.old_read_bytes),
            total_read_bytes: self.read_bytes,
        }
    }
}

impl Process {
    // FIXME: this should be a method of ProcessExt.
    /// Return how much the process has been running.
    pub fn run_time(&self) -> u64 {
        self.run_time
    }
}

pub(crate) unsafe fn get_process_data(
    kproc: &libc::kinfo_proc,
    wrap: &Wrap,
    page_size: isize,
    fscale: f32,
) -> Option<Process> {
    if kproc.ki_pid != 1 && (kproc.ki_flag as libc::c_int & libc::P_SYSTEM) != 0 {
        // We filter out the kernel threads.
        return None;
    }

    // We now get the values needed for both new and existing process.
    let cpu_usage = (100 * kproc.ki_pctcpu) as f32 / fscale;
    // Processes can be reparented apparently?
    let parent = if kproc.ki_ppid != 0 {
        Some(kproc.ki_ppid)
    } else {
        None
    };
    let status = ProcessStatus::from(kproc.ki_stat);

    // from FreeBSD source /src/usr.bin/top/machine.c
    let virtual_memory = (kproc.ki_size / 1_000) as u64;
    let memory = (kproc.ki_rssize * page_size) as u64;
    let run_time = (kproc.ki_runtime + 5_000) / 10_000;

    if let Some(proc_) = (*wrap.0.get()).get_mut(&kproc.ki_pid) {
        proc_.cpu_usage = cpu_usage;
        proc_.parent = parent;
        proc_.status = status;
        proc_.virtual_memory = virtual_memory;
        proc_.memory = memory;
        proc_.run_time = run_time;
        proc_.updated = true;

        proc_.old_read_bytes = proc_.read_bytes;
        proc_.read_bytes = kproc.ki_rusage.ru_inblock as _;
        proc_.old_written_bytes = proc_.written_bytes;
        proc_.written_bytes = kproc.ki_rusage.ru_oublock as _;

        return None;
    }

    // This is a new process, we need to get more information!
    let mut buffer = [0; 2048];

    let exe = get_sys_value_str(
        &[
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PATHNAME,
            kproc.ki_pid,
        ],
        &mut buffer,
    )
    .unwrap_or_else(String::new);
    let cwd = get_sys_value_str(
        &[
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_CWD,
            kproc.ki_pid,
        ],
        &mut buffer,
    )
    .map(|s| s.into())
    .unwrap_or_else(PathBuf::new);

    Some(Process {
        pid: kproc.ki_pid,
        parent,
        uid: kproc.ki_ruid,
        gid: kproc.ki_rgid,
        start_time: kproc.ki_start.tv_sec as _,
        run_time,
        cpu_usage,
        virtual_memory,
        memory,
        cwd,
        exe: exe.into(),
        // kvm_getargv isn't thread-safe so we get it in the main thread.
        name: String::new(),
        // kvm_getargv isn't thread-safe so we get it in the main thread.
        cmd: Vec::new(),
        // kvm_getargv isn't thread-safe so we get it in the main thread.
        root: PathBuf::new(),
        // kvm_getenvv isn't thread-safe so we get it in the main thread.
        environ: Vec::new(),
        status,
        read_bytes: kproc.ki_rusage.ru_inblock as _,
        old_read_bytes: 0,
        written_bytes: kproc.ki_rusage.ru_oublock as _,
        old_written_bytes: 0,
        updated: true,
    })
}
