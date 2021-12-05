// Take a look at the license at the top of the repository in the LICENSE file.

use crate::{
    sys::{component::Component, Disk, Networks, Process, Processor},
    LoadAvg, Pid, RefreshKind, SystemExt, User,
};

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr::NonNull;

use super::utils::{
    self, boot_time, c_buf_to_string, from_cstr_array, get_sys_value, get_sys_value_array,
    get_sys_value_by_name, get_sys_value_str_by_name, get_system_info, init_mib,
};

use libc::c_int;

#[doc = include_str!("../../md_doc/system.md")]
pub struct System {
    process_list: HashMap<Pid, Process>,
    mem_total: u64,
    mem_free: u64,
    mem_used: u64,
    mem_available: u64,
    swap_total: u64,
    swap_used: u64,
    global_processor: Processor,
    processors: Vec<Processor>,
    components: Vec<Component>,
    disks: Vec<Disk>,
    networks: Networks,
    users: Vec<User>,
    boot_time: u64,
    system_info: SystemInfo,
}

impl SystemExt for System {
    const IS_SUPPORTED: bool = true;

    fn new_with_specifics(refreshes: RefreshKind) -> System {
        let system_info = SystemInfo::new();

        let mut s = System {
            process_list: HashMap::with_capacity(200),
            mem_total: 0,
            mem_free: 0,
            mem_available: 0,
            mem_used: 0,
            swap_total: 0,
            swap_used: 0,
            global_processor: Processor::new(String::new(), String::new(), 0),
            processors: Vec::with_capacity(system_info.nb_cpus as _),
            components: Vec::with_capacity(2),
            disks: Vec::with_capacity(1),
            networks: Networks::new(),
            users: Vec::new(),
            boot_time: boot_time(),
            system_info,
        };
        s.refresh_specifics(refreshes);
        s
    }

    fn refresh_memory(&mut self) {
        if self.mem_total == 0 {
            self.mem_total = self.system_info.get_total_memory();
        }
        self.mem_used = self.system_info.get_used_memory();
        self.mem_free = self.system_info.get_free_memory();
        let (swap_used, swap_total) = self.system_info.get_swap_info();
        self.swap_total = swap_total;
        self.swap_used = swap_used;
    }

    fn refresh_cpu(&mut self) {
        if self.processors.is_empty() {
            let mut frequency: libc::size_t = 0;

            // We get the processor vendor ID in here.
            let vendor_id =
                get_sys_value_str_by_name(b"hw.model\0").unwrap_or_else(|| "<unknown>".to_owned());
            for pos in 0..self.system_info.nb_cpus {
                unsafe {
                    // The information can be missing if it's running inside a VM.
                    if !get_sys_value_by_name(
                        format!("dev.cpu.{}.freq\0", pos).as_bytes(),
                        &mut frequency,
                    ) {
                        frequency = 0;
                    }
                }
                self.processors.push(Processor::new(
                    format!("cpu {}", pos),
                    vendor_id.clone(),
                    frequency as _,
                ));
            }
            self.global_processor.vendor_id = vendor_id;
        }
        self.system_info
            .get_cpu_usage(&mut self.global_processor, &mut self.processors);
    }

    fn refresh_components_list(&mut self) {
        if self.processors.is_empty() {
            self.refresh_cpu();
        }
        self.components = unsafe { super::component::get_components(self.processors.len()) };
    }

    fn refresh_processes(&mut self) {
        unsafe { self.refresh_procs() }
    }

    fn refresh_process(&mut self, pid: Pid) -> bool {
        let proc_ = unsafe {
            let kd = self.system_info.kd.as_ptr();
            let mut count = 0;
            let procs = libc::kvm_getprocs(kd, libc::KERN_PROC_PROC, 0, &mut count);
            if count < 1 {
                sysinfo_debug!("kvm_getprocs returned nothing...");
                return false;
            }

            let fscale = self.system_info.fscale;
            let page_size_k = self.system_info.page_size_k as isize;
            let proc_list = utils::Wrap(UnsafeCell::new(&mut self.process_list));
            let procs = utils::ProcList(std::slice::from_raw_parts(procs, count as _));

            #[cfg(feature = "multithread")]
            use rayon::iter::ParallelIterator;

            let iter = crate::utils::into_iter(procs);
            #[cfg(not(feature = "multithread"))]
            {
                iter.find(|kproc| kproc.ki_pid == pid)
            }
            #[cfg(feature = "multithread")]
            { iter.find_any(|kproc| kproc.0.ki_pid == pid) }.and_then(|kproc| {
                super::process::get_process_data(kproc.0, &proc_list, page_size_k, fscale)
            })
        };
        if let Some(proc_) = proc_ {
            self.process_list.insert(proc_.pid, proc_);
            true
        } else {
            self.process_list
                .get(&pid)
                .map(|p| p.updated)
                .unwrap_or(false)
        }
    }

    fn refresh_disks_list(&mut self) {
        self.disks = unsafe { super::disk::get_all_disks() };
    }

    fn refresh_users_list(&mut self) {
        self.users = crate::users::get_users_list();
    }

    // COMMON PART
    //
    // Need to be moved into a "common" file to avoid duplication.

    fn processes(&self) -> &HashMap<Pid, Process> {
        &self.process_list
    }

    fn process(&self, _pid: Pid) -> Option<&Process> {
        None
    }

    fn networks(&self) -> &Networks {
        &self.networks
    }

    fn networks_mut(&mut self) -> &mut Networks {
        &mut self.networks
    }

    fn global_processor_info(&self) -> &Processor {
        &self.global_processor
    }

    fn processors(&self) -> &[Processor] {
        &self.processors
    }

    fn physical_core_count(&self) -> Option<usize> {
        let mut physical_core_count: u32 = 0;

        if unsafe { get_sys_value_by_name(b"hw.ncpu\0", &mut physical_core_count) } {
            Some(physical_core_count as _)
        } else {
            None
        }
    }

    fn total_memory(&self) -> u64 {
        self.mem_total
    }

    fn free_memory(&self) -> u64 {
        self.mem_free
    }

    fn available_memory(&self) -> u64 {
        self.mem_available
    }

    fn used_memory(&self) -> u64 {
        self.mem_used
    }

    fn total_swap(&self) -> u64 {
        self.swap_total
    }

    fn free_swap(&self) -> u64 {
        self.swap_total - self.swap_used
    }

    // TODO: need to be checked
    fn used_swap(&self) -> u64 {
        self.swap_used
    }

    fn components(&self) -> &[Component] {
        &self.components
    }

    fn components_mut(&mut self) -> &mut [Component] {
        &mut self.components
    }

    fn disks(&self) -> &[Disk] {
        &self.disks
    }

    fn disks_mut(&mut self) -> &mut [Disk] {
        &mut self.disks
    }

    fn uptime(&self) -> u64 {
        let csec = unsafe { libc::time(::std::ptr::null_mut()) };

        unsafe { libc::difftime(csec, self.boot_time as _) as u64 }
    }

    fn boot_time(&self) -> u64 {
        self.boot_time
    }

    fn load_average(&self) -> LoadAvg {
        let mut loads = vec![0f64; 3];
        unsafe {
            libc::getloadavg(loads.as_mut_ptr(), 3);
        }
        LoadAvg {
            one: loads[0],
            five: loads[1],
            fifteen: loads[2],
        }
    }

    fn users(&self) -> &[User] {
        &self.users
    }

    fn name(&self) -> Option<String> {
        self.system_info.get_os_name()
    }

    fn long_os_version(&self) -> Option<String> {
        self.system_info.get_os_release_long()
    }

    fn host_name(&self) -> Option<String> {
        self.system_info.get_hostname()
    }

    fn kernel_version(&self) -> Option<String> {
        self.system_info.get_kernel_version()
    }

    fn os_version(&self) -> Option<String> {
        self.system_info.get_os_release()
    }
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

impl System {
    unsafe fn refresh_procs(&mut self) {
        let kd = self.system_info.kd.as_ptr();
        let procs = {
            let mut count = 0;
            let procs = libc::kvm_getprocs(kd, libc::KERN_PROC_PROC, 0, &mut count);
            if count < 1 {
                sysinfo_debug!("kvm_getprocs returned nothing...");
                return;
            }
            #[cfg(feature = "multithread")]
            use rayon::iter::{ParallelIterator, ParallelIterator as IterTrait};
            #[cfg(not(feature = "multithread"))]
            use std::iter::Iterator as IterTrait;

            crate::utils::into_iter(&mut self.process_list).for_each(|(_, proc_)| {
                proc_.updated = false;
            });

            let fscale = self.system_info.fscale;
            let page_size_k = self.system_info.page_size_k as isize;
            let proc_list = utils::Wrap(UnsafeCell::new(&mut self.process_list));
            let procs = utils::ProcList(std::slice::from_raw_parts(procs, count as _));

            IterTrait::filter_map(crate::utils::into_iter(procs), |kproc| {
                super::process::get_process_data(kproc.0, &proc_list, page_size_k, fscale)
                    .map(|p| (kproc, p))
            })
            .collect::<Vec<_>>()
        };

        // We remove all processes that don't exist anymore.
        self.process_list.retain(|_, v| v.updated);

        for (kproc, mut proc_) in procs {
            let kproc = kproc.0;
            proc_.cmd = from_cstr_array(libc::kvm_getargv(kd, kproc, 0) as _);
            if !proc_.cmd.is_empty() {
                // First, we try to retrieve the name from the command line.
                let p = Path::new(&proc_.cmd[0]);
                if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                    proc_.name = name.to_owned();
                } else {
                    // The name can be cut short because the `ki_comm` field size is limited,
                    // which is why we prefer to get the name from the command line as much as
                    // possible.
                    proc_.name = c_buf_to_string(&kproc.ki_comm).unwrap_or_else(String::new);
                }
                if let Some(parent) = p.parent() {
                    proc_.root = parent.to_path_buf();
                }
            }
            proc_.environ = from_cstr_array(libc::kvm_getenvv(kd, kproc, 0) as _);
            self.process_list.insert(proc_.pid, proc_);
        }
    }
}

// FIXME: to be removed once 0.2.108 libc has been published!
const CPUSTATES: usize = 5;

/// This struct is used to get system information more easily.
#[derive(Debug)]
struct SystemInfo {
    hw_physical_memory: [c_int; 2],
    page_size_k: c_int,
    virtual_page_count: [c_int; 4],
    virtual_wire_count: [c_int; 4],
    virtual_active_count: [c_int; 4],
    virtual_cache_count: [c_int; 4],
    virtual_inactive_count: [c_int; 4],
    virtual_free_count: [c_int; 4],
    os_type: [c_int; 2],
    os_release: [c_int; 2],
    kern_version: [c_int; 2],
    hostname: [c_int; 2],
    buf_space: [c_int; 2],
    nb_cpus: c_int,
    kd: NonNull<libc::kvm_t>,
    // For these two fields, we could use `kvm_getcptime` but the function isn't very efficient...
    mib_cp_time: [c_int; 2],
    mib_cp_times: [c_int; 2],
    // For the global CPU usage.
    cp_time: utils::VecSwitcher<libc::c_ulong>,
    // For each processor CPU usage.
    cp_times: utils::VecSwitcher<libc::c_ulong>,
    /// From FreeBSD manual: "The kernel fixed-point scale factor". It's used when computing
    /// processes' CPU usage.
    fscale: f32,
}

// This is needed because `kd: *mut libc::kvm_t` isn't thread-safe.
unsafe impl Send for SystemInfo {}
unsafe impl Sync for SystemInfo {}

impl SystemInfo {
    fn new() -> Self {
        let kd = unsafe {
            let mut errbuf =
                MaybeUninit::<[libc::c_char; libc::_POSIX2_LINE_MAX as usize]>::uninit();
            NonNull::new(libc::kvm_openfiles(
                std::ptr::null(),
                b"/dev/null\0".as_ptr() as *const _,
                std::ptr::null(),
                0,
                errbuf.as_mut_ptr() as *mut _,
            ))
            .expect("kvm_openfiles failed")
        };

        let mut smp: c_int = 0;
        let mut nb_cpus: c_int = 1;
        unsafe {
            if !get_sys_value_by_name(b"kern.smp.active\0", &mut smp) {
                smp = 0;
            }
            if smp != 0 {
                if !get_sys_value_by_name(b"kern.smp.cpus\0", &mut nb_cpus) || nb_cpus < 1 {
                    nb_cpus = 1;
                }
            }
        }

        let mut si = SystemInfo {
            hw_physical_memory: Default::default(),
            page_size_k: 0,
            virtual_page_count: Default::default(),
            virtual_wire_count: Default::default(),
            virtual_active_count: Default::default(),
            virtual_cache_count: Default::default(),
            virtual_inactive_count: Default::default(),
            virtual_free_count: Default::default(),
            buf_space: Default::default(),
            os_type: Default::default(),
            os_release: Default::default(),
            kern_version: Default::default(),
            hostname: Default::default(),
            nb_cpus,
            kd,
            mib_cp_time: Default::default(),
            mib_cp_times: Default::default(),
            cp_time: utils::VecSwitcher::new(vec![0; CPUSTATES]),
            cp_times: utils::VecSwitcher::new(vec![0; nb_cpus as usize * CPUSTATES]),
            fscale: 0.,
        };
        unsafe {
            let mut fscale: c_int = 0;
            if !get_sys_value_by_name(b"kern.fscale\0", &mut fscale) {
                // Default value used in htop.
                fscale = 2048;
            }
            si.fscale = fscale as f32;

            if !get_sys_value_by_name(b"vm.stats.vm.v_page_size\0", &mut si.page_size_k) {
                panic!("cannot get page size...");
            }
            si.page_size_k /= 1_000;

            init_mib(b"hw.physmem\0", &mut si.hw_physical_memory);
            init_mib(b"vm.stats.vm.v_page_count\0", &mut si.virtual_page_count);
            init_mib(b"vm.stats.vm.v_wire_count\0", &mut si.virtual_wire_count);
            init_mib(
                b"vm.stats.vm.v_active_count\0",
                &mut si.virtual_active_count,
            );
            init_mib(b"vm.stats.vm.v_cache_count\0", &mut si.virtual_cache_count);
            init_mib(
                b"vm.stats.vm.v_inactive_count\0",
                &mut si.virtual_inactive_count,
            );
            init_mib(b"vm.stats.vm.v_free_count\0", &mut si.virtual_free_count);
            init_mib(b"vfs.bufspace\0", &mut si.buf_space);

            init_mib(b"kern.ostype\0", &mut si.os_type);
            init_mib(b"kern.osrelease\0", &mut si.os_release);
            init_mib(b"kern.version\0", &mut si.kern_version);
            init_mib(b"kern.hostname\0", &mut si.hostname);

            init_mib(b"kern.cp_time\0", &mut si.mib_cp_time);
            init_mib(b"kern.cp_times\0", &mut si.mib_cp_times);
        }

        si
    }

    fn get_os_name(&self) -> Option<String> {
        get_system_info(&[self.os_type[0], self.os_type[1]], Some("FreeBSD"))
    }

    fn get_kernel_version(&self) -> Option<String> {
        get_system_info(&[self.kern_version[0], self.kern_version[1]], None)
    }

    fn get_os_release_long(&self) -> Option<String> {
        get_system_info(&[self.os_release[0], self.os_release[1]], None)
    }

    fn get_os_release(&self) -> Option<String> {
        // It returns something like "13.0-RELEASE". We want to keep everything until the "-".
        get_system_info(&[self.os_release[0], self.os_release[1]], None)
            .and_then(|s| s.split('-').next().map(|s| s.to_owned()))
    }

    fn get_hostname(&self) -> Option<String> {
        get_system_info(&[self.hostname[0], self.hostname[1]], None)
    }

    /// Returns (used, total).
    fn get_swap_info(&self) -> (u64, u64) {
        // Magic number used in htop. Cannot find how they got when reading `kvm_getswapinfo` source
        // code so here we go...
        const LEN: usize = 16;
        let mut swap = MaybeUninit::<[libc::kvm_swap; LEN]>::uninit();
        unsafe {
            let nswap =
                libc::kvm_getswapinfo(self.kd.as_ptr(), swap.as_mut_ptr() as *mut _, LEN as _, 0)
                    as usize;
            if nswap < 1 {
                return (0, 0);
            }
            let swap =
                std::slice::from_raw_parts(swap.as_ptr() as *mut libc::kvm_swap, nswap.min(LEN));
            let (used, total) = swap.iter().fold((0, 0), |(used, total), swap| {
                (used + swap.ksw_used as u64, total + swap.ksw_total as u64)
            });
            (
                used * self.page_size_k as u64,
                total * self.page_size_k as u64,
            )
        }
    }

    fn get_total_memory(&self) -> u64 {
        let mut total_memory: u64 = 0;
        unsafe {
            get_sys_value(&self.hw_physical_memory, &mut total_memory);
        }
        total_memory / 1_000
    }

    fn get_used_memory(&self) -> u64 {
        let mut mem_active: u64 = 0;
        let mut mem_wire: u64 = 0;

        unsafe {
            get_sys_value(&self.virtual_active_count, &mut mem_active);
            get_sys_value(&self.virtual_wire_count, &mut mem_wire);
        }

        (mem_active * self.page_size_k as u64) + (mem_wire * self.page_size_k as u64)
    }

    fn get_free_memory(&self) -> u64 {
        let mut buffers_mem: u64 = 0;
        let mut inactive_mem: u64 = 0;
        let mut cached_mem: u64 = 0;
        let mut free_mem: u64 = 0;

        unsafe {
            get_sys_value(&self.buf_space, &mut buffers_mem);
            get_sys_value(&self.virtual_inactive_count, &mut inactive_mem);
            get_sys_value(&self.virtual_cache_count, &mut cached_mem);
            get_sys_value(&self.virtual_free_count, &mut free_mem);
        }
        // For whatever reason, buffers_mem is already the right value...
        buffers_mem
            + (inactive_mem * self.page_size_k as u64)
            + (cached_mem * self.page_size_k as u64)
            + (free_mem * self.page_size_k as u64)
    }

    fn get_cpu_usage(&mut self, global: &mut Processor, processors: &mut [Processor]) {
        unsafe {
            get_sys_value_array(&self.mib_cp_time, self.cp_time.get_mut());
            get_sys_value_array(&self.mib_cp_times, self.cp_times.get_mut());
        }

        fn fill_processor(
            proc_: &mut Processor,
            new_cp_time: &[libc::c_ulong],
            old_cp_time: &[libc::c_ulong],
        ) {
            let mut total_new: u64 = 0;
            let mut total_old: u64 = 0;
            let mut cp_diff: libc::c_ulong = 0;

            for i in 0..(CPUSTATES as usize) {
                // We obviously don't want to get the idle part of the processor usage, otherwise
                // we would always be at 100%...
                if i != libc::CP_IDLE as usize {
                    cp_diff += new_cp_time[i] - old_cp_time[i];
                }
                total_new += new_cp_time[i] as u64;
                total_old += old_cp_time[i] as u64;
            }

            let total_diff = total_new - total_old;
            if total_diff < 1 {
                proc_.cpu_usage = 0.;
            } else {
                proc_.cpu_usage = cp_diff as f32 / total_diff as f32 * 100.;
            }
        }

        fill_processor(global, self.cp_time.get_new(), self.cp_time.get_old());
        let old_cp_times = self.cp_times.get_old();
        let new_cp_times = self.cp_times.get_new();
        for (pos, proc_) in processors.iter_mut().enumerate() {
            let index = pos * CPUSTATES as usize;

            fill_processor(proc_, &new_cp_times[index..], &old_cp_times[index..]);
        }
    }
}

impl Drop for SystemInfo {
    fn drop(&mut self) {
        unsafe {
            libc::kvm_close(self.kd.as_ptr());
        }
    }
}
