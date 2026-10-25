//! Detection of processes holding the dGPU open (`/proc/<pid>/fd/*`).

use std::fs;
use std::io;
use std::path::Path;

use crate::fs::{FsRoot, read_trimmed};

/// A process with at least one open descriptor on the dGPU.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Holder {
    /// Process id.
    pub pid: u32,
    /// Process name (`/proc/<pid>/comm`).
    pub name: String,
}

/// Result of a holder scan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HolderScan {
    /// Holders, sorted by pid, one entry per process.
    pub holders: Vec<Holder>,
    /// Processes whose `fd/` could not be read (permission denied). Non-zero means the scan is incomplete:
    /// only a root daemon sees every process.
    pub unreadable: usize,
}

/// `true` if an fd link target designates the dGPU: any NVIDIA character device, or one of its DRM nodes.
pub(crate) fn targets_dgpu(target: &Path, drm_nodes: &[String]) -> bool {
    let Some(target) = target.to_str() else {
        return false;
    };
    target.starts_with("/dev/nvidia") || drm_nodes.iter().any(|node| node == target)
}

/// Scans every process under `<root>/proc` for descriptors matching [`targets_dgpu`].
pub(crate) fn scan(root: &FsRoot, drm_nodes: &[String]) -> HolderScan {
    let mut scan = HolderScan::default();
    let Ok(entries) = fs::read_dir(root.proc("")) else {
        return scan;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let fd_dir = entry.path().join("fd");
        let fds = match fs::read_dir(&fd_dir) {
            Ok(fds) => fds,
            // The process exited between the two read_dir calls.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                scan.unreadable += 1;
                continue;
            }
        };
        let holds = fds
            .flatten()
            .filter_map(|fd| fs::read_link(fd.path()).ok())
            .any(|target| targets_dgpu(&target, drm_nodes));
        if holds {
            let name = read_trimmed(&entry.path().join("comm")).unwrap_or_default();
            scan.holders.push(Holder { pid, name });
        }
    }
    scan.holders.sort();
    scan
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::fs::testutil::{symlink, write};

    fn nodes() -> Vec<String> {
        vec!["/dev/dri/card0".into(), "/dev/dri/renderD129".into()]
    }

    #[test]
    fn nvidia_devices_and_dgpu_drm_nodes_match() {
        for t in [
            "/dev/nvidia0",
            "/dev/nvidiactl",
            "/dev/nvidia-uvm",
            "/dev/dri/card0",
            "/dev/dri/renderD129",
        ] {
            assert!(targets_dgpu(&PathBuf::from(t), &nodes()), "{t}");
        }
        for t in [
            "/dev/dri/card1",
            "/dev/dri/renderD128",
            "/dev/null",
            "socket:[123]",
            "/home/x/nvidia0",
        ] {
            assert!(!targets_dgpu(&PathBuf::from(t), &nodes()), "{t}");
        }
    }

    #[test]
    fn scan_lists_each_holder_once_sorted_and_skips_others() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "proc/4255/comm", "Xwayland\n");
        symlink(r, "proc/4255/fd/7", "/dev/dri/card0");
        write(r, "proc/3876/comm", "gnome-shell\n");
        symlink(r, "proc/3876/fd/3", "/dev/dri/card1");
        symlink(r, "proc/3876/fd/14", "/dev/nvidia0");
        symlink(r, "proc/3876/fd/15", "/dev/nvidia0");
        write(r, "proc/6164/comm", "ghostty\n");
        symlink(r, "proc/6164/fd/9", "/dev/dri/renderD128");
        write(r, "proc/self-not-a-pid/comm", "x");
        write(r, "proc/stat", "cpu 0");

        let scan = scan(&FsRoot::new(r), &nodes());
        assert_eq!(
            scan.holders,
            vec![
                Holder {
                    pid: 3876,
                    name: "gnome-shell".into()
                },
                Holder {
                    pid: 4255,
                    name: "Xwayland".into()
                },
            ]
        );
        assert_eq!(scan.unreadable, 0);
    }

    #[test]
    fn unreadable_fd_dirs_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "proc/867/comm", "nvidia-powerd\n");
        fs::create_dir_all(r.join("proc/867/fd")).unwrap();
        fs::set_permissions(r.join("proc/867/fd"), fs::Permissions::from_mode(0o000)).unwrap();
        let readable_as_root = fs::read_dir(r.join("proc/867/fd")).is_ok();

        let scan = scan(&FsRoot::new(r), &nodes());
        fs::set_permissions(r.join("proc/867/fd"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(scan.holders, Vec::<Holder>::new());
        // Running the tests as root bypasses permissions; only assert when they apply.
        if !readable_as_root {
            assert_eq!(scan.unreadable, 1);
        }
    }

    #[test]
    fn missing_proc_yields_empty_scan() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(scan(&FsRoot::new(dir.path()), &nodes()), HolderScan::default());
    }
}
