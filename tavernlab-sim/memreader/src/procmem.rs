//! `process_vm_readv` — the one syscall this whole tool exists to call
//! carefully and nowhere else. It is a **read**: the kernel copies bytes
//! from the target's address space into ours, the same access
//! `/proc/PID/mem` grants, gated by the same permission check ptrace uses
//! (same uid, or `CAP_SYS_PTRACE`, or a permissive `yama.ptrace_scope`).
//! Nothing here writes to the target, calls into it, or attaches a
//! debugger to it.
//!
//! Declared directly against the glibc symbol rather than through the
//! `libc` crate, to keep this workspace's zero-dependency rule
//! (CLAUDE.md) — `process_vm_readv` has shipped in glibc since 2.15 (2012).

use std::os::raw::{c_int, c_long, c_ulong, c_void};

#[repr(C)]
struct IoVec {
    iov_base: *mut c_void,
    iov_len: usize,
}

unsafe extern "C" {
    fn process_vm_readv(
        pid: c_int,
        local_iov: *const IoVec,
        liovcnt: c_ulong,
        remote_iov: *const IoVec,
        riovcnt: c_ulong,
        flags: c_ulong,
    ) -> c_long;
}

pub struct Remote {
    pid: u32,
}

impl Remote {
    pub fn new(pid: u32) -> Remote {
        Remote { pid }
    }

    /// Read `len` bytes starting at `addr` in the target process. `None`
    /// on any failure (permission, unmapped address, process gone) —
    /// callers are expected to explain the likely cause to the person
    /// running this, since a bare `errno` means little to them.
    pub fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let local = IoVec {
            iov_base: buf.as_mut_ptr() as *mut c_void,
            iov_len: len,
        };
        let remote = IoVec {
            iov_base: addr as *mut c_void,
            iov_len: len,
        };
        let n = unsafe {
            process_vm_readv(self.pid as c_int, &local, 1, &remote, 1, 0)
        };
        if n as usize == len {
            Some(buf)
        } else {
            None
        }
    }
}
