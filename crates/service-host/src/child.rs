//! The descriptors of a process Hub starts: standard input, output and error,
//! and no other.
//!
//! **Why.** A descriptor without close-on-exec passes to every program a
//! process starts. On macOS std and mio create and accept sockets with one
//! call and mark them close-on-exec with a second, so a child started from
//! another thread between the two calls inherits the socket: a device's link
//! connection, a control client's connection, the service socket, a download.
//! A link Hub closed then stays open in the child, and a service (code Hub did
//! not write) holds Hub's connections. Linux creates them close-on-exec in
//! one call, but a descriptor opened by C code or a library without the flag
//! passes on there too.
//!
//! **How, in a child** (`inherit_nothing`): between fork and exec, the child
//! marks close-on-exec every descriptor from 3 up that is not marked, and the
//! exec closes them. It marks rather than closes: std keeps a close-on-exec
//! pipe open in the child to report a failed exec to the parent, and that
//! pipe must stay open until the exec.
//!
//! **Enumerating, async-signal-safe.** Between fork and exec only
//! async-signal-safe calls may run, so the child cannot read `/dev/fd` or
//! `/proc/self/fd` (`opendir` allocates). It tries each number from 3 up to
//! the soft `RLIMIT_NOFILE` limit with `fcntl(F_GETFD)`, which answers
//! `EBADF` at once for a number that is not open; the kernel numbers no
//! descriptor at or above the soft limit, and Hub does not lower its limit.
//! The limit is read in the parent before the spawn and capped at
//! `MAX_SCANNED`. The same loop runs on macOS and Linux. Cost: one system
//! call per number: 256 on macOS's default limit (well under a
//! millisecond), about 10,000 on a raised one (a few milliseconds), once per
//! service start or `bun install`.
//!
//! **How, in Hub's own core** (`close_inherited`): `pocket-hubd` closes, as
//! the first thing it does, every descriptor from 3 up that it inherited from
//! whatever started it (the command line, the app), so a core never holds a
//! client's descriptors for its life. Every descriptor without close-on-exec
//! at that moment came through the exec.

use std::os::unix::process::CommandExt;

/// The highest descriptor number scanned, whatever the limit says.
const MAX_SCANNED: libc::c_int = 1 << 16;

/// One past the highest descriptor number the process can hold: the soft
/// `RLIMIT_NOFILE` limit, capped at `MAX_SCANNED`.
fn descriptor_bound() -> libc::c_int {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes into `limit`, which lives for the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return MAX_SCANNED;
    }
    limit.rlim_cur.min(MAX_SCANNED as libc::rlim_t) as libc::c_int
}

/// Marks close-on-exec every descriptor from 3 below `bound` that is not.
/// Async-signal-safe: `fcntl` only.
fn mark_close_on_exec(bound: libc::c_int) {
    for fd in 3..bound {
        // SAFETY: fcntl on a number that may not be open answers EBADF.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
            // SAFETY: as above; setting the flag changes nothing else.
            unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
        }
    }
}

/// A command whose child starts with standard input, output and error only.
pub trait InheritNothing {
    fn inherit_nothing(&mut self) -> &mut Self;
}

impl InheritNothing for std::process::Command {
    fn inherit_nothing(&mut self) -> &mut Self {
        let bound = descriptor_bound();
        // SAFETY: the closure calls fcntl only, which is async-signal-safe,
        // and touches no memory but its copied bound.
        unsafe {
            self.pre_exec(move || {
                mark_close_on_exec(bound);
                Ok(())
            })
        }
    }
}

impl InheritNothing for tokio::process::Command {
    fn inherit_nothing(&mut self) -> &mut Self {
        let bound = descriptor_bound();
        // SAFETY: as for std's command.
        unsafe {
            self.pre_exec(move || {
                mark_close_on_exec(bound);
                Ok(())
            })
        }
    }
}

/// Closes every descriptor from 3 up that is not close-on-exec: what this
/// process inherited through its exec. For `pocket-hubd`'s `main`, before it
/// opens anything or starts a thread.
pub fn close_inherited() {
    for fd in 3..descriptor_bound() {
        // SAFETY: fcntl and close on numbers this process does not use yet:
        // nothing has opened a descriptor of its own without close-on-exec.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                libc::close(fd);
            }
        }
    }
}
