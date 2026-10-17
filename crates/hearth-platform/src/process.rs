//! Read-only process inspection through `/proc`.

use std::collections::BTreeSet;
use std::fs;

/// One process as seen in `/proc/<pid>/stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    /// Process id.
    pub pid: u32,
    /// Parent process id.
    pub ppid: u32,
    /// Process group id.
    pub pgrp: u32,
    /// Command name (`comm`, at most 15 bytes).
    pub comm: String,
}

/// Parses a `/proc/<pid>/stat` line. The command name may contain spaces and parentheses,
/// so fields are counted from the last `)`: state, ppid, pgrp.
pub fn parse_stat(stat: &str) -> Option<ProcInfo> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let pid = stat[..open].trim().parse().ok()?;
    let comm = stat.get(open + 1..close)?.to_owned();
    let mut rest = stat[close + 1..].split_whitespace();
    let _state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    let pgrp = rest.next()?.parse().ok()?;
    Some(ProcInfo {
        pid,
        ppid,
        pgrp,
        comm,
    })
}

/// Every process currently visible in `/proc`.
pub fn processes() -> Vec<ProcInfo> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?))
        .collect()
}

/// Pids belonging to a game: every process in group `pgid`, plus every descendant of any of
/// them (children that moved to their own group or session included).
pub fn game_pids(pgid: u32) -> BTreeSet<u32> {
    game_pids_in(&processes(), pgid)
}

fn game_pids_in(procs: &[ProcInfo], pgid: u32) -> BTreeSet<u32> {
    let mut found: BTreeSet<u32> = procs
        .iter()
        .filter(|p| p.pgrp == pgid)
        .map(|p| p.pid)
        .collect();
    loop {
        let before = found.len();
        for p in procs {
            if found.contains(&p.ppid) {
                found.insert(p.pid);
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// Whether a process with this exact `comm` is running (e.g. `steam`).
pub fn any_process_named(comm: &str) -> bool {
    processes().iter().any(|p| p.comm == comm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, pgrp: u32) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            pgrp,
            comm: "x".into(),
        }
    }

    #[test]
    fn stat_with_tricky_comm() {
        let info = parse_stat("4242 (retro arch (x)) S 1 4242 4242 0 -1 4194560").unwrap();
        assert_eq!(info.pid, 4242);
        assert_eq!(info.ppid, 1);
        assert_eq!(info.pgrp, 4242);
        assert_eq!(info.comm, "retro arch (x)");
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn group_members_and_their_descendants() {
        let procs = [
            p(100, 1, 100),          // game leader
            p(101, 100, 100),        // same group
            p(102, 101, 102),        // child that made its own group
            p(103, 102, 102),        // grandchild in that group
            p(200, 1, 200),          // unrelated
            p(201, 200, 100 + 1000), // unrelated
        ];
        let pids: Vec<_> = game_pids_in(&procs, 100).into_iter().collect();
        assert_eq!(pids, [100, 101, 102, 103]);
    }

    #[test]
    fn own_process_is_visible() {
        let me = std::process::id();
        let procs = processes();
        let info = procs.iter().find(|p| p.pid == me).unwrap();
        assert!(game_pids(info.pgrp).contains(&me));
    }
}
