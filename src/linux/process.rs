// Take a look at the license at the top of the repository in the LICENSE file.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use libc::{gid_t, kill, sysconf, uid_t, _SC_CLK_TCK};

use crate::sys::system::REMAINING_FILES;
use crate::sys::utils::{get_all_data, get_all_data_from_file, realpath};
use crate::utils::into_iter;
use crate::{DiskUsage, Pid, ProcessExt, ProcessStatus, Signal};

#[doc(hidden)]
impl From<u32> for ProcessStatus {
    fn from(status: u32) -> ProcessStatus {
        match status {
            1 => ProcessStatus::Idle,
            2 => ProcessStatus::Run,
            3 => ProcessStatus::Sleep,
            4 => ProcessStatus::Stop,
            5 => ProcessStatus::Zombie,
            x => ProcessStatus::Unknown(x),
        }
    }
}

#[doc(hidden)]
impl From<char> for ProcessStatus {
    fn from(status: char) -> ProcessStatus {
        match status {
            'R' => ProcessStatus::Run,
            'S' => ProcessStatus::Sleep,
            'D' => ProcessStatus::Idle,
            'Z' => ProcessStatus::Zombie,
            'T' => ProcessStatus::Stop,
            't' => ProcessStatus::Tracing,
            'X' | 'x' => ProcessStatus::Dead,
            'K' => ProcessStatus::Wakekill,
            'W' => ProcessStatus::Waking,
            'P' => ProcessStatus::Parked,
            x => ProcessStatus::Unknown(x as u32),
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
            ProcessStatus::Tracing => "Tracing",
            ProcessStatus::Dead => "Dead",
            ProcessStatus::Wakekill => "Wakekill",
            ProcessStatus::Waking => "Waking",
            ProcessStatus::Parked => "Parked",
            ProcessStatus::Unknown(_) => "Unknown",
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
    utime: u64,
    stime: u64,
    old_utime: u64,
    old_stime: u64,
    start_time: u64,
    updated: bool,
    cpu_usage: f32,
    /// User id of the process owner.
    pub uid: uid_t,
    /// Group id of the process owner.
    pub gid: gid_t,
    pub(crate) status: ProcessStatus,
    /// Tasks run by this process.
    pub tasks: HashMap<Pid, Process>,
    pub(crate) stat_file: Option<File>,
    old_read_bytes: u64,
    old_written_bytes: u64,
    read_bytes: u64,
    written_bytes: u64,
}

impl ProcessExt for Process {
    fn new(pid: Pid, parent: Option<Pid>, start_time: u64) -> Process {
        Process {
            name: String::with_capacity(20),
            pid,
            parent,
            cmd: Vec::with_capacity(2),
            environ: Vec::with_capacity(10),
            exe: PathBuf::new(),
            cwd: PathBuf::new(),
            root: PathBuf::new(),
            memory: 0,
            virtual_memory: 0,
            cpu_usage: 0.,
            utime: 0,
            stime: 0,
            old_utime: 0,
            old_stime: 0,
            updated: true,
            start_time,
            uid: 0,
            gid: 0,
            status: ProcessStatus::Unknown(0),
            tasks: if pid == 0 {
                HashMap::with_capacity(1000)
            } else {
                HashMap::new()
            },
            stat_file: None,
            old_read_bytes: 0,
            old_written_bytes: 0,
            read_bytes: 0,
            written_bytes: 0,
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
            Signal::Poll => libc::SIGPOLL,
            Signal::Power => libc::SIGPWR,
            Signal::Sys => libc::SIGSYS,
        };
        unsafe { kill(self.pid, c_signal) == 0 }
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
            written_bytes: self.written_bytes - self.old_written_bytes,
            total_written_bytes: self.written_bytes,
            read_bytes: self.read_bytes - self.old_read_bytes,
            total_read_bytes: self.read_bytes,
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.stat_file.is_some() {
            if let Ok(ref mut x) = unsafe { crate::sys::system::REMAINING_FILES.lock() } {
                **x += 1;
            }
        }
    }
}

pub fn compute_cpu_usage(p: &mut Process, total_time: f32, max_value: f32) {
    // First time updating the values without reference, wait for a second cycle to update cpu_usage
    if p.old_utime == 0 && p.old_stime == 0 {
        return;
    }

    // We use `max_value` to ensure that the process CPU usage will never get bigger than:
    // `"number of CPUs" * 100.`
    p.cpu_usage = ((p.utime.saturating_sub(p.old_utime) + p.stime.saturating_sub(p.old_stime))
        as f32
        / total_time
        * 100.)
        .min(max_value);
    p.updated = false;
}

pub fn set_time(p: &mut Process, utime: u64, stime: u64) {
    p.old_utime = p.utime;
    p.old_stime = p.stime;
    p.utime = utime;
    p.stime = stime;
    p.updated = true;
}

pub fn has_been_updated(p: &Process) -> bool {
    p.updated
}

pub(crate) fn update_process_disk_activity(p: &mut Process, path: &Path) {
    let mut path = PathBuf::from(path);
    path.push("io");
    let data = match get_all_data(&path, 16_384) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut done = 0;
    for line in data.split('\n') {
        let mut parts = line.split(": ");
        match parts.next() {
            Some("read_bytes") => {
                p.old_read_bytes = p.read_bytes;
                p.read_bytes = parts
                    .next()
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(p.old_read_bytes);
            }
            Some("write_bytes") => {
                p.old_written_bytes = p.written_bytes;
                p.written_bytes = parts
                    .next()
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(p.old_written_bytes);
            }
            _ => continue,
        }
        done += 1;
        if done > 1 {
            // No need to continue the reading.
            break;
        }
    }
}

struct Wrap<'a, T>(UnsafeCell<&'a mut T>);

impl<'a, T> Wrap<'a, T> {
    fn get(&self) -> &'a mut T {
        unsafe { *(self.0.get()) }
    }
}

#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<'a, T> Send for Wrap<'a, T> {}
unsafe impl<'a, T> Sync for Wrap<'a, T> {}

pub(crate) fn _get_process_data(
    path: &Path,
    proc_list: &mut Process,
    page_size_kb: u64,
    pid: Pid,
    uptime: u64,
    now: u64,
) -> Result<(Option<Process>, Pid), ()> {
    let nb = match path.file_name().and_then(|x| x.to_str()).map(Pid::from_str) {
        Some(Ok(nb)) if nb != pid => nb,
        _ => return Err(()),
    };

    let get_status = |p: &mut Process, part: &str| {
        p.status = part
            .chars()
            .next()
            .map(ProcessStatus::from)
            .unwrap_or_else(|| ProcessStatus::Unknown(0));
    };
    let parent_memory = proc_list.memory;
    let parent_virtual_memory = proc_list.virtual_memory;
    if let Some(ref mut entry) = proc_list.tasks.get_mut(&nb) {
        let data = if let Some(ref mut f) = entry.stat_file {
            get_all_data_from_file(f, 1024).map_err(|_| ())?
        } else {
            let mut tmp = PathBuf::from(path);
            tmp.push("stat");
            let mut file = File::open(tmp).map_err(|_| ())?;
            let data = get_all_data_from_file(&mut file, 1024).map_err(|_| ())?;
            entry.stat_file = check_nb_open_files(file);
            data
        };
        let parts = parse_stat_file(&data)?;
        get_status(entry, parts[2]);
        update_time_and_memory(
            path,
            entry,
            &parts,
            page_size_kb,
            parent_memory,
            parent_virtual_memory,
            nb,
            uptime,
            now,
        );
        update_process_disk_activity(entry, path);
        return Ok((None, nb));
    }

    let mut tmp = PathBuf::from(path);

    tmp.push("stat");
    let mut file = std::fs::File::open(&tmp).map_err(|_| ())?;
    let data = get_all_data_from_file(&mut file, 1024).map_err(|_| ())?;
    let stat_file = check_nb_open_files(file);
    let parts = parse_stat_file(&data)?;
    let name = parts[1];

    let parent_pid = if proc_list.pid != 0 {
        Some(proc_list.pid)
    } else {
        match Pid::from_str(parts[3]) {
            Ok(p) if p != 0 => Some(p),
            _ => None,
        }
    };

    let clock_cycle = unsafe { sysconf(_SC_CLK_TCK) } as u64;
    let since_boot = u64::from_str(parts[21]).unwrap_or(0) / clock_cycle;
    let start_time = now.saturating_sub(uptime.saturating_sub(since_boot));
    let mut p = Process::new(nb, parent_pid, start_time);

    p.stat_file = stat_file;
    get_status(&mut p, parts[2]);

    tmp.pop();
    tmp.push("status");
    if let Ok(data) = get_all_data(&tmp, 16_385) {
        if let Some((uid, gid)) = _get_uid_and_gid(data) {
            p.uid = uid;
            p.gid = gid;
        }
    }

    if proc_list.pid != 0 {
        // If we're getting information for a child, no need to get those info since we
        // already have them...
        p.cmd = proc_list.cmd.clone();
        p.name = proc_list.name.clone();
        p.environ = proc_list.environ.clone();
        p.exe = proc_list.exe.clone();
        p.cwd = proc_list.cwd.clone();
        p.root = proc_list.root.clone();
    } else {
        p.name = name.into();
        tmp.pop();
        tmp.push("cmdline");
        p.cmd = copy_from_file(&tmp);
        tmp.pop();
        tmp.push("exe");
        match tmp.read_link() {
            Ok(exe_path) => {
                p.exe = exe_path;
            }
            Err(_) => {
                p.exe = if let Some(cmd) = p.cmd.get(0) {
                    PathBuf::from(cmd)
                } else {
                    PathBuf::new()
                };
            }
        }
        tmp.pop();
        tmp.push("environ");
        p.environ = copy_from_file(&tmp);
        tmp.pop();
        tmp.push("cwd");
        p.cwd = realpath(&tmp);
        tmp.pop();
        tmp.push("root");
        p.root = realpath(&tmp);
    }

    update_time_and_memory(
        path,
        &mut p,
        &parts,
        page_size_kb,
        proc_list.memory,
        proc_list.virtual_memory,
        nb,
        uptime,
        now,
    );
    update_process_disk_activity(&mut p, path);
    Ok((Some(p), nb))
}

#[allow(clippy::too_many_arguments)]
fn update_time_and_memory(
    path: &Path,
    entry: &mut Process,
    parts: &[&str],
    page_size_kb: u64,
    parent_memory: u64,
    parent_virtual_memory: u64,
    pid: Pid,
    uptime: u64,
    now: u64,
) {
    {
        // rss
        entry.memory = u64::from_str(parts[23]).unwrap_or(0) * page_size_kb;
        if entry.memory >= parent_memory {
            entry.memory -= parent_memory;
        }
        // vsz
        entry.virtual_memory = u64::from_str(parts[22]).unwrap_or(0);
        if entry.virtual_memory >= parent_virtual_memory {
            entry.virtual_memory -= parent_virtual_memory;
        }
        set_time(
            entry,
            u64::from_str(parts[13]).unwrap_or(0),
            u64::from_str(parts[14]).unwrap_or(0),
        );
    }
    refresh_procs(entry, &path.join("task"), page_size_kb, pid, uptime, now);
}

pub(crate) fn refresh_procs(
    proc_list: &mut Process,
    path: &Path,
    page_size_kb: u64,
    pid: Pid,
    uptime: u64,
    now: u64,
) -> bool {
    if let Ok(d) = fs::read_dir(path) {
        let folders = d
            .filter_map(|entry| {
                if let Ok(entry) = entry {
                    let entry = entry.path();

                    if entry.is_dir() {
                        Some(entry)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if pid == 0 {
            let proc_list = Wrap(UnsafeCell::new(proc_list));

            #[cfg(feature = "multithread")]
            use rayon::iter::ParallelIterator;

            into_iter(folders)
                .filter_map(|e| {
                    if let Ok((p, _)) = _get_process_data(
                        e.as_path(),
                        proc_list.get(),
                        page_size_kb,
                        pid,
                        uptime,
                        now,
                    ) {
                        p
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        } else {
            let mut updated_pids = Vec::with_capacity(folders.len());
            let new_tasks = folders
                .iter()
                .filter_map(|e| {
                    if let Ok((p, pid)) =
                        _get_process_data(e.as_path(), proc_list, page_size_kb, pid, uptime, now)
                    {
                        updated_pids.push(pid);
                        p
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            // Sub-tasks are not cleaned up outside so we do it here directly.
            proc_list
                .tasks
                .retain(|&pid, _| updated_pids.iter().any(|&x| x == pid));
            new_tasks
        }
        .into_iter()
        .for_each(|e| {
            proc_list.tasks.insert(e.pid(), e);
        });
        true
    } else {
        false
    }
}

fn copy_from_file(entry: &Path) -> Vec<String> {
    match File::open(entry) {
        Ok(mut f) => {
            let mut data = vec![0; 16_384];

            if let Ok(size) = f.read(&mut data) {
                data.truncate(size);
                let mut out = Vec::with_capacity(20);
                let mut start = 0;
                for (pos, x) in data.iter().enumerate() {
                    if *x == 0 {
                        if pos - start >= 1 {
                            if let Ok(s) =
                                std::str::from_utf8(&data[start..pos]).map(|x| x.trim().to_owned())
                            {
                                out.push(s);
                            }
                        }
                        start = pos + 1; // to keeping prevent '\0'
                    }
                }
                out
            } else {
                Vec::new()
            }
        }
        Err(_) => Vec::new(),
    }
}

fn _get_uid_and_gid(status_data: String) -> Option<(uid_t, gid_t)> {
    // We're only interested in the lines starting with Uid: and Gid:
    // here. From these lines, we're looking at the second entry to get
    // the effective u/gid.

    let f = |h: &str, n: &str| -> Option<uid_t> {
        if h.starts_with(n) {
            h.split_whitespace().nth(2).unwrap_or("0").parse().ok()
        } else {
            None
        }
    };
    let mut uid = None;
    let mut gid = None;
    for line in status_data.lines() {
        if let Some(u) = f(line, "Uid:") {
            assert!(uid.is_none());
            uid = Some(u);
        } else if let Some(g) = f(line, "Gid:") {
            assert!(gid.is_none());
            gid = Some(g);
        } else {
            continue;
        }
        if uid.is_some() && gid.is_some() {
            break;
        }
    }
    match (uid, gid) {
        (Some(u), Some(g)) => Some((u, g)),
        _ => None,
    }
}

fn check_nb_open_files(f: File) -> Option<File> {
    if let Ok(ref mut x) = unsafe { REMAINING_FILES.lock() } {
        if **x > 0 {
            **x -= 1;
            return Some(f);
        }
    }
    // Something bad happened...
    None
}

macro_rules! unwrap_or_return {
    ($data:expr) => {{
        match $data {
            Some(x) => x,
            None => return Err(()),
        }
    }};
}

fn parse_stat_file(data: &str) -> Result<Vec<&str>, ()> {
    // The stat file is "interesting" to parse, because spaces cannot
    // be used as delimiters. The second field stores the command name
    // surrounded by parentheses. Unfortunately, whitespace and
    // parentheses are legal parts of the command, so parsing has to
    // proceed like this: The first field is delimited by the first
    // whitespace, the second field is everything until the last ')'
    // in the entire string. All other fields are delimited by
    // whitespace.

    let mut parts = Vec::with_capacity(52);
    let mut data_it = data.splitn(2, ' ');
    parts.push(unwrap_or_return!(data_it.next()));
    let mut data_it = unwrap_or_return!(data_it.next()).rsplitn(2, ')');
    let data = unwrap_or_return!(data_it.next());
    parts.push(unwrap_or_return!(data_it.next()));
    parts.extend(data.split_whitespace());
    // Remove command name '('
    if let Some(name) = parts[1].strip_prefix('(') {
        parts[1] = name;
    }
    Ok(parts)
}
