//! /proc's files (J66): what the kernel says about the machine and its
//! processes, as text a program can read - numbers out of the kernel,
//! never files.
//!
//!   uptime        "SECONDS BUSY": since the clock started, and the CPU
//!                 time spent running something (all CPUs added up),
//!                 each to the hundredth of a second
//!   meminfo       MemTotal, MemFree (the frames, in kB), HeapUsed and
//!                 HeapTotal (the kernel's own heap), as Linux spells its
//!   cpuinfo       one "processor : N" line per CPU
//!   stat          Processes, Threads, Channels: how many there are
//!   PID/status    Name, Pid, State (running, or ended with its code),
//!                 CPU (nanoseconds its threads were on a CPU), Memory
//!                 (what is charged to it, in kB), Threads and Handles,
//!                 for each process the kernel lists
//!
//! The text is made at each request, from what the kernel says then, so
//! a program sees the state as it is. Nothing can be written, made,
//! removed or renamed. There is no "self" here: a service can't know
//! who is asking, so libc turns /proc/self into /proc/PID before asking.
//!
//! Where the numbers come from is a `Source` whoever makes the store
//! gives it: the `proc` service asks the kernel, a test makes them up.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use abi::{ProcessEntry, SystemInfo};

use crate::protocol::{Kind, Status};
use crate::{names, valid_directory, Store};

/// Where the numbers come from.
pub trait Source {
    /// The machine, now: None if the kernel would not say.
    fn system(&mut self) -> Option<SystemInfo>;
    /// The processes, now, in the kernel's order.
    fn processes(&mut self) -> Vec<ProcessEntry>;
}

/// The files in the root, in name order (the order LIST gives).
const FILES: [&str; 4] = ["cpuinfo", "meminfo", "stat", "uptime"];
/// The files in a process's directory.
const PROCESS_FILES: [&str; 1] = ["status"];

/// What a path names here.
enum Named {
    Root,
    /// A file in the root.
    File(&'static str),
    /// A process's directory.
    Process,
    /// A file in it.
    ProcessFile(ProcessEntry, &'static str),
}

/// /proc, as a Store.
pub struct ProcStore<S: Source> {
    source: S,
}

impl<S: Source> ProcStore<S> {
    pub fn new(source: S) -> Self {
        ProcStore { source }
    }

    /// What `path` names: the root, a file, a process or one of its
    /// files - or why nothing.
    fn find(&mut self, path: &str) -> Result<Named, Status> {
        if !valid_directory(path) {
            return Err(Status::BadName);
        }
        let mut steps = names(path);
        let Some(first) = steps.next() else { return Ok(Named::Root) };
        let second = steps.next();
        if let Some(&file) = FILES.iter().find(|&&file| file == first) {
            return match second {
                None => Ok(Named::File(file)),
                Some(_) => Err(Status::NotDirectory),
            };
        }
        // A process: its number, written as `ls` shows it (no "+", no
        // leading zeros), and listed by the kernel now.
        let pid = first.parse::<u64>().ok().filter(|pid| pid.to_string() == first).ok_or(Status::NotFound)?;
        let process = self.source.processes().into_iter().find(|p| p.pid == pid).ok_or(Status::NotFound)?;
        match (second, steps.next()) {
            (None, _) => Ok(Named::Process),
            (Some(name), None) => match PROCESS_FILES.iter().find(|&&file| file == name) {
                Some(&file) => Ok(Named::ProcessFile(process, file)),
                None => Err(Status::NotFound),
            },
            (Some(_), Some(_)) => Err(Status::NotDirectory),
        }
    }

    /// The text of a file in the root, now.
    fn text(&mut self, file: &str) -> Result<String, Status> {
        // The kernel not answering is the one thing that can go wrong
        // here: an error a program can see, not an empty file.
        let info = self.source.system().ok_or(Status::DiskError)?;
        Ok(match file {
            "uptime" => {
                let per_second = info.ticks_per_second.max(1);
                let hundredths = |ticks: u64| ticks * 100 / per_second;
                let (up, busy) = (hundredths(info.ticks), hundredths(info.busy_ticks));
                format!("{}.{:02} {}.{:02}\n", up / 100, up % 100, busy / 100, busy % 100)
            }
            "meminfo" => format!(
                "MemTotal:  {:>10} kB\nMemFree:   {:>10} kB\nHeapUsed:  {:>10} kB\nHeapTotal: {:>10} kB\n",
                info.frames_total * 4,
                info.frames_free * 4,
                info.heap_used / 1024,
                info.heap_total / 1024
            ),
            "cpuinfo" => (0..info.cpus).map(|cpu| format!("processor\t: {cpu}\n")).collect(),
            "stat" => format!("Processes:\t{}\nThreads:\t{}\nChannels:\t{}\n", info.processes, info.threads, info.channels),
            _ => return Err(Status::NotFound),
        })
    }

    /// The text of a process's file.
    fn process_text(process: &ProcessEntry, file: &str) -> Result<String, Status> {
        match file {
            "status" => {
                let state = match process.exit_code {
                    None => String::from("running"),
                    Some(code) => format!("ended ({code})"),
                };
                Ok(format!(
                    "Name:\t{}\nPid:\t{}\nState:\t{}\nCPU:\t{} ns\nMemory:\t{} kB\nThreads:\t{}\nHandles:\t{}\n",
                    process.name(),
                    process.pid,
                    state,
                    process.cpu_ns,
                    process.memory / 1024,
                    process.threads,
                    process.handles
                ))
            }
            _ => Err(Status::NotFound),
        }
    }

    /// The whole text of what `path` names, if it is a file.
    fn contents(&mut self, path: &str) -> Result<String, Status> {
        match self.find(path)? {
            Named::Root | Named::Process => Err(Status::IsDirectory),
            Named::File(file) => self.text(file),
            Named::ProcessFile(process, file) => Self::process_text(&process, file),
        }
    }
}

impl<S: Source> Store for ProcStore<S> {
    fn list(&mut self, directory: &str, from: usize, each: &mut dyn FnMut(&str, Kind, u64) -> bool) -> Result<(), Status> {
        let entries: Vec<(String, Kind)> = match self.find(directory)? {
            Named::File(_) | Named::ProcessFile(..) => return Err(Status::NotDirectory),
            Named::Root => {
                // The files, then the processes by number - in the
                // order `ls` sorts names, which is what LIST promises.
                let mut pids: Vec<u64> = self.source.processes().iter().map(|p| p.pid).collect();
                pids.sort_unstable();
                let mut entries: Vec<(String, Kind)> = pids.iter().map(|pid| (pid.to_string(), Kind::Directory)).collect();
                entries.extend(FILES.iter().map(|&file| (String::from(file), Kind::File)));
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                entries
            }
            Named::Process => PROCESS_FILES.iter().map(|&file| (String::from(file), Kind::File)).collect(),
        };
        for (name, kind) in entries.into_iter().skip(from) {
            // A file's size is its text's, now: what a program reading
            // it will get.
            let size = if kind == Kind::File {
                let path = format!("{}/{}", directory.trim_end_matches('/'), name);
                self.contents(&path).map(|text| text.len() as u64).unwrap_or(0)
            } else {
                0
            };
            if !each(&name, kind, size) {
                break;
            }
        }
        Ok(())
    }

    fn stat(&mut self, path: &str) -> Result<(Kind, u64), Status> {
        match self.find(path)? {
            Named::Root | Named::Process => Ok((Kind::Directory, 0)),
            Named::File(_) | Named::ProcessFile(..) => Ok((Kind::File, self.contents(path)?.len() as u64)),
        }
    }

    /// Everything here changes all the time: the stamp is the clock.
    fn read_only(&self) -> bool {
        true
    }

    fn stamp(&mut self, path: &str) -> Result<u64, Status> {
        match self.find(path)? {
            Named::Root | Named::Process => Ok(0),
            _ => Ok(self.source.system().map_or(0, |info| info.ticks)),
        }
    }

    fn touch(&mut self, name: &str) -> Result<(), Status> {
        self.find(name)?;
        Err(Status::ReadOnly)
    }

    fn make_directory(&mut self, path: &str) -> Result<(), Status> {
        Err(match self.find(path) {
            Ok(_) => Status::Exists,
            Err(Status::NotFound) => Status::ReadOnly,
            Err(status) => status,
        })
    }

    fn read(&mut self, name: &str, offset: u64, out: &mut [u8]) -> Result<usize, Status> {
        let text = self.contents(name)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(text.len());
        let n = out.len().min(text.len() - start);
        out[..n].copy_from_slice(&text.as_bytes()[start..start + n]);
        Ok(n)
    }

    fn write(&mut self, name: &str, _offset: u64, _data: &[u8]) -> Result<u64, Status> {
        Err(match self.find(name) {
            Ok(Named::Root | Named::Process) => Status::IsDirectory,
            Ok(_) | Err(Status::NotFound) => Status::ReadOnly,
            Err(status) => status,
        })
    }

    fn truncate(&mut self, name: &str, _length: u64) -> Result<(), Status> {
        Err(match self.find(name) {
            Ok(Named::Root | Named::Process) => Status::IsDirectory,
            Ok(_) | Err(Status::NotFound) => Status::ReadOnly,
            Err(status) => status,
        })
    }

    fn remove(&mut self, name: &str) -> Result<(), Status> {
        self.find(name)?;
        Err(Status::ReadOnly)
    }

    fn rename(&mut self, from: &str, _to: &str) -> Result<(), Status> {
        self.find(from)?;
        Err(Status::ReadOnly)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine made up for the tests: two processes, one ended.
    struct Fake;

    fn entry(pid: u64, name: &str, exit_code: Option<i64>, cpu_ns: u64) -> ProcessEntry {
        let mut entry = ProcessEntry { pid, name: [0; 16], exit_code, cpu_ns, memory: 48 * 1024, threads: 2, handles: 9 };
        entry.name[..name.len()].copy_from_slice(name.as_bytes());
        entry
    }

    impl Source for Fake {
        fn system(&mut self) -> Option<SystemInfo> {
            Some(SystemInfo {
                ticks: 12_345,
                ticks_per_second: 100,
                frames_total: 1000,
                frames_free: 600,
                heap_used: 10 * 1024,
                heap_total: 64 * 1024,
                processes: 2,
                threads: 3,
                channels: 7,
                cpus: 2,
                busy_ticks: 250,
            })
        }
        fn processes(&mut self) -> Vec<ProcessEntry> {
            alloc::vec![entry(7, "shell", None, 1_500_000), entry(3, "init", None, 20), entry(12, "ls", Some(0), 999)]
        }
    }

    /// The kernel that would not say.
    struct Silent;

    impl Source for Silent {
        fn system(&mut self) -> Option<SystemInfo> {
            None
        }
        fn processes(&mut self) -> Vec<ProcessEntry> {
            Vec::new()
        }
    }

    fn read_all(store: &mut ProcStore<impl Source>, path: &str) -> String {
        let mut out = [0u8; 256];
        let n = store.read(path, 0, &mut out).unwrap();
        String::from_utf8(out[..n].to_vec()).unwrap()
    }

    #[test]
    fn lists_files_and_processes_in_name_order() {
        let mut proc = ProcStore::new(Fake);
        let mut seen = Vec::new();
        proc.list("", 0, &mut |name, kind, _| {
            seen.push((String::from(name), kind));
            true
        })
        .unwrap();
        let names: Vec<&str> = seen.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["12", "3", "7", "cpuinfo", "meminfo", "stat", "uptime"]);
        assert_eq!(seen[0].1, Kind::Directory);
        assert_eq!(seen[3].1, Kind::File);
        let mut inside = Vec::new();
        proc.list("7", 0, &mut |name, kind, size| {
            inside.push((String::from(name), kind, size));
            true
        })
        .unwrap();
        assert_eq!(inside, [(String::from("status"), Kind::File, 86)]);
        assert_eq!(proc.list("uptime", 0, &mut |_, _, _| true), Err(Status::NotDirectory));
        assert_eq!(proc.list("99", 0, &mut |_, _, _| true), Err(Status::NotFound));
        assert_eq!(proc.stat(""), Ok((Kind::Directory, 0)));
        assert_eq!(proc.stat("12"), Ok((Kind::Directory, 0)));
    }

    #[test]
    fn the_files_say_what_the_kernel_said() {
        let mut proc = ProcStore::new(Fake);
        assert_eq!(read_all(&mut proc, "uptime"), "123.45 2.50\n");
        assert_eq!(read_all(&mut proc, "meminfo"), "MemTotal:        4000 kB\nMemFree:         2400 kB\nHeapUsed:          10 kB\nHeapTotal:         64 kB\n");
        assert_eq!(read_all(&mut proc, "cpuinfo"), "processor\t: 0\nprocessor\t: 1\n");
        assert_eq!(read_all(&mut proc, "stat"), "Processes:\t2\nThreads:\t3\nChannels:\t7\n");
        assert_eq!(read_all(&mut proc, "7/status"), "Name:\tshell\nPid:\t7\nState:\trunning\nCPU:\t1500000 ns\nMemory:\t48 kB\nThreads:\t2\nHandles:\t9\n");
        assert_eq!(read_all(&mut proc, "12/status"), "Name:\tls\nPid:\t12\nState:\tended (0)\nCPU:\t999 ns\nMemory:\t48 kB\nThreads:\t2\nHandles:\t9\n");
        // stat's size is the text's, and a read past the end gives none.
        assert_eq!(proc.stat("7/status"), Ok((Kind::File, 86)));
        assert_eq!(proc.read("7/status", 86, &mut [0; 8]), Ok(0));
        let mut piece = [0u8; 4];
        assert_eq!(proc.read("7/status", 6, &mut piece), Ok(4));
        assert_eq!(&piece, b"shel");
        // A kernel that would not say: an error, not an empty file.
        assert_eq!(ProcStore::new(Silent).read("uptime", 0, &mut [0; 8]), Err(Status::DiskError));
    }

    #[test]
    fn nothing_can_be_changed_and_bad_names_are_refused() {
        let mut proc = ProcStore::new(Fake);
        assert_eq!(proc.stat("nothing"), Err(Status::NotFound));
        assert_eq!(proc.stat("07"), Err(Status::NotFound)); // not how the number is written
        assert_eq!(proc.stat("7/nothing"), Err(Status::NotFound));
        assert_eq!(proc.stat("7/status/x"), Err(Status::NotDirectory));
        assert_eq!(proc.stat("uptime/x"), Err(Status::NotDirectory));
        assert_eq!(proc.read("", 0, &mut [0; 4]), Err(Status::IsDirectory));
        assert_eq!(proc.read("7", 0, &mut [0; 4]), Err(Status::IsDirectory));
        assert_eq!(proc.write("uptime", 0, b"1"), Err(Status::ReadOnly));
        assert_eq!(proc.write("new", 0, b"1"), Err(Status::ReadOnly));
        assert_eq!(proc.write("7", 0, b"1"), Err(Status::IsDirectory));
        assert_eq!(proc.truncate("uptime", 0), Err(Status::ReadOnly));
        assert_eq!(proc.make_directory("d"), Err(Status::ReadOnly));
        assert_eq!(proc.make_directory("7"), Err(Status::Exists));
        assert_eq!(proc.remove("uptime"), Err(Status::ReadOnly));
        assert_eq!(proc.remove("nothing"), Err(Status::NotFound));
        assert_eq!(proc.rename("7", "8"), Err(Status::ReadOnly));
        assert_eq!(proc.touch("uptime"), Err(Status::ReadOnly));
        assert_eq!(proc.write("..", 0, b"x"), Err(Status::BadName));
    }
}
