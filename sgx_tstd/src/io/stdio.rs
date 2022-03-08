// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License..

#[cfg(feature = "unit_test")]
mod tests;

use crate::io::prelude::*;

use crate::cell::RefCell;
use crate::fmt;
use crate::io::{self, BufReader, IoSlice, IoSliceMut, LineWriter, Lines};
use crate::lazy::SyncOnceCell;
use crate::pin::Pin;
use crate::sync::{Mutex, MutexGuard};
use crate::sys::stdio;
use crate::sys_common::remutex::{ReentrantMutex, ReentrantMutexGuard};

/// A handle to a raw instance of the standard input stream of this process.
///
/// This handle is not synchronized or buffered in any fashion. Constructed via
/// the `std::io::stdio::stdin_raw` function.
struct StdinRaw(stdio::Stdin);

/// A handle to a raw instance of the standard output stream of this process.
///
/// This handle is not synchronized or buffered in any fashion. Constructed via
/// the `std::io::stdio::stdout_raw` function.
struct StdoutRaw(stdio::Stdout);

/// A handle to a raw instance of the standard output stream of this process.
///
/// This handle is not synchronized or buffered in any fashion. Constructed via
/// the `std::io::stdio::stderr_raw` function.
struct StderrRaw(stdio::Stderr);

/// Constructs a new raw handle to the standard input of this process.
///
/// The returned handle does not interact with any other handles created nor
/// handles returned by `std::io::stdin`. Data buffered by the `std::io::stdin`
/// handles is **not** available to raw handles returned from this function.
///
/// The returned handle has no external synchronization or buffering.
const fn stdin_raw() -> StdinRaw {
    StdinRaw(stdio::Stdin::new())
}

/// Constructs a new raw handle to the standard output stream of this process.
///
/// The returned handle does not interact with any other handles created nor
/// handles returned by `std::io::stdout`. Note that data is buffered by the
/// `std::io::stdout` handles so writes which happen via this raw handle may
/// appear before previous writes.
///
/// The returned handle has no external synchronization or buffering layered on
/// top.
const fn stdout_raw() -> StdoutRaw {
    StdoutRaw(stdio::Stdout::new())
}

/// Constructs a new raw handle to the standard error stream of this process.
///
/// The returned handle does not interact with any other handles created nor
/// handles returned by `std::io::stderr`.
///
/// The returned handle has no external synchronization or buffering layered on
/// top.
const fn stderr_raw() -> StderrRaw {
    StderrRaw(stdio::Stderr::new())
}

impl Read for StdinRaw {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        handle_ebadf(self.0.read(buf), 0)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        handle_ebadf(self.0.read_vectored(bufs), 0)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.0.is_read_vectored()
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        handle_ebadf(self.0.read_to_end(buf), 0)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        handle_ebadf(self.0.read_to_string(buf), 0)
    }
}

impl Write for StdoutRaw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        handle_ebadf(self.0.write(buf), buf.len())
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let total = bufs.iter().map(|b| b.len()).sum();
        handle_ebadf(self.0.write_vectored(bufs), total)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn flush(&mut self) -> io::Result<()> {
        handle_ebadf(self.0.flush(), ())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        handle_ebadf(self.0.write_all(buf), ())
    }

    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        handle_ebadf(self.0.write_all_vectored(bufs), ())
    }

    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        handle_ebadf(self.0.write_fmt(fmt), ())
    }
}

impl Write for StderrRaw {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        handle_ebadf(self.0.write(buf), buf.len())
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let total = bufs.iter().map(|b| b.len()).sum();
        handle_ebadf(self.0.write_vectored(bufs), total)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.0.is_write_vectored()
    }

    fn flush(&mut self) -> io::Result<()> {
        handle_ebadf(self.0.flush(), ())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        handle_ebadf(self.0.write_all(buf), ())
    }

    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        handle_ebadf(self.0.write_all_vectored(bufs), ())
    }

    fn write_fmt(&mut self, fmt: fmt::Arguments<'_>) -> io::Result<()> {
        handle_ebadf(self.0.write_fmt(fmt), ())
    }
}

fn handle_ebadf<T>(r: io::Result<T>, default: T) -> io::Result<T> {
    match r {
        Err(ref e) if stdio::is_ebadf(e) => Ok(default),
        r => r,
    }
}

/// A handle to the standard input stream of a process.
///
/// Each handle is a shared reference to a global buffer of input data to this
/// process. A handle can be `lock`'d to gain full access to [`BufRead`] methods
/// (e.g., `.lines()`). Reads to this handle are otherwise locked with respect
/// to other reads.
///
/// This handle implements the `Read` trait, but beware that concurrent reads
/// of `Stdin` must be executed with care.
///
/// Created by the [`io::stdin`] method.
///
/// [`io::stdin`]: stdin
///
/// ### Note: Windows Portability Consideration
///
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to read bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// ```no_run
/// use std::io;
///
/// fn main() -> io::Result<()> {
///     let mut buffer = String::new();
///     let mut stdin = io::stdin(); // We get `Stdin` here.
///     stdin.read_line(&mut buffer)?;
///     Ok(())
/// }
/// ```
pub struct Stdin {
    inner: &'static Mutex<BufReader<StdinRaw>>,
}

/// A locked reference to the [`Stdin`] handle.
///
/// This handle implements both the [`Read`] and [`BufRead`] traits, and
/// is constructed via the [`Stdin::lock`] method.
///
/// ### Note: Windows Portability Consideration
///
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to read bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// ```no_run
/// use std::io::{self, BufRead};
///
/// fn main() -> io::Result<()> {
///     let mut buffer = String::new();
///     let stdin = io::stdin(); // We get `Stdin` here.
///     {
///         let mut handle = stdin.lock(); // We get `StdinLock` here.
///         handle.read_line(&mut buffer)?;
///     } // `StdinLock` is dropped here.
///     Ok(())
/// }
/// ```
#[must_use = "if unused stdin will immediately unlock"]
pub struct StdinLock<'a> {
    inner: MutexGuard<'a, BufReader<StdinRaw>>,
}

/// Constructs a new handle to the standard input of the current process.
///
/// Each handle returned is a reference to a shared global buffer whose access
/// is synchronized via a mutex. If you need more explicit control over
/// locking, see the [`Stdin::lock`] method.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to read bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// Using implicit synchronization:
///
/// ```no_run
/// use std::io;
///
/// fn main() -> io::Result<()> {
///     let mut buffer = String::new();
///     io::stdin().read_line(&mut buffer)?;
///     Ok(())
/// }
/// ```
///
/// Using explicit synchronization:
///
/// ```no_run
/// use std::io::{self, BufRead};
///
/// fn main() -> io::Result<()> {
///     let mut buffer = String::new();
///     let stdin = io::stdin();
///     let mut handle = stdin.lock();
///
///     handle.read_line(&mut buffer)?;
///     Ok(())
/// }
/// ```
#[must_use]
pub fn stdin() -> Stdin {
    static INSTANCE: SyncOnceCell<Mutex<BufReader<StdinRaw>>> = SyncOnceCell::new();
    Stdin {
        inner: INSTANCE.get_or_init(|| {
            Mutex::new(BufReader::with_capacity(stdio::STDIN_BUF_SIZE, stdin_raw()))
        }),
    }
}

/// Constructs a new locked handle to the standard input of the current
/// process.
///
/// Each handle returned is a guard granting locked access to a shared
/// global buffer whose access is synchronized via a mutex. If you need
/// more explicit control over locking, for example, in a multi-threaded
/// program, use the [`io::stdin`] function to obtain an unlocked handle,
/// along with the [`Stdin::lock`] method.
///
/// The lock is released when the returned guard goes out of scope. The
/// returned guard also implements the [`Read`] and [`BufRead`] traits for
/// accessing the underlying data.
///
/// **Note**: The mutex locked by this handle is not reentrant. Even in a
/// single-threaded program, calling other code that accesses [`Stdin`]
/// could cause a deadlock or panic, if this locked handle is held across
/// that call.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to read bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// ```no_run
/// #![feature(stdio_locked)]
/// use std::io::{self, BufRead};
///
/// fn main() -> io::Result<()> {
///     let mut buffer = String::new();
///     let mut handle = io::stdin_locked();
///
///     handle.read_line(&mut buffer)?;
///     Ok(())
/// }
/// ```
pub fn stdin_locked() -> StdinLock<'static> {
    stdin().into_locked()
}

impl Stdin {
    /// Locks this handle to the standard input stream, returning a readable
    /// guard.
    ///
    /// The lock is released when the returned lock goes out of scope. The
    /// returned guard also implements the [`Read`] and [`BufRead`] traits for
    /// accessing the underlying data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io::{self, BufRead};
    ///
    /// fn main() -> io::Result<()> {
    ///     let mut buffer = String::new();
    ///     let stdin = io::stdin();
    ///     let mut handle = stdin.lock();
    ///
    ///     handle.read_line(&mut buffer)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn lock(&self) -> StdinLock<'_> {
        self.lock_any()
    }

    /// Locks this handle and reads a line of input, appending it to the specified buffer.
    ///
    /// For detailed semantics of this method, see the documentation on
    /// [`BufRead::read_line`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io;
    ///
    /// let mut input = String::new();
    /// match io::stdin().read_line(&mut input) {
    ///     Ok(n) => {
    ///         println!("{} bytes read", n);
    ///         println!("{}", input);
    ///     }
    ///     Err(error) => println!("error: {}", error),
    /// }
    /// ```
    ///
    /// You can run the example one of two ways:
    ///
    /// - Pipe some text to it, e.g., `printf foo | path/to/executable`
    /// - Give it text interactively by running the executable directly,
    ///   in which case it will wait for the Enter key to be pressed before
    ///   continuing
    pub fn read_line(&self, buf: &mut String) -> io::Result<usize> {
        self.lock().read_line(buf)
    }

    // Locks this handle with any lifetime. This depends on the
    // implementation detail that the underlying `Mutex` is static.
    fn lock_any<'a>(&self) -> StdinLock<'a> {
        StdinLock { inner: self.inner.lock().unwrap_or_else(|e| e.into_inner()) }
    }

    /// Consumes this handle to the standard input stream, locking the
    /// shared global buffer associated with the stream and returning a
    /// readable guard.
    ///
    /// The lock is released when the returned guard goes out of scope. The
    /// returned guard also implements the [`Read`] and [`BufRead`] traits
    /// for accessing the underlying data.
    ///
    /// It is often simpler to directly get a locked handle using the
    /// [`stdin_locked`] function instead, unless nearby code also needs to
    /// use an unlocked handle.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![feature(stdio_locked)]
    /// use std::io::{self, BufRead};
    ///
    /// fn main() -> io::Result<()> {
    ///     let mut buffer = String::new();
    ///     let mut handle = io::stdin().into_locked();
    ///
    ///     handle.read_line(&mut buffer)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn into_locked(self) -> StdinLock<'static> {
        self.lock_any()
    }

    /// Consumes this handle and returns an iterator over input lines.
    ///
    /// For detailed semantics of this method, see the documentation on
    /// [`BufRead::lines`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![feature(stdin_forwarders)]
    /// use std::io;
    ///
    /// let lines = io::stdin().lines();
    /// for line in lines {
    ///     println!("got a line: {}", line.unwrap());
    /// }
    /// ```
    #[must_use = "`self` will be dropped if the result is not used"]
    pub fn lines(self) -> Lines<StdinLock<'static>> {
        self.into_locked().lines()
    }
}

impl fmt::Debug for Stdin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stdin").finish_non_exhaustive()
    }
}

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.lock().read(buf)
    }
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.lock().read_vectored(bufs)
    }
    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.lock().is_read_vectored()
    }
    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.lock().read_to_end(buf)
    }
    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.lock().read_to_string(buf)
    }
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.lock().read_exact(buf)
    }
}

// only used by platform-dependent io::copy specializations, i.e. unused on some platforms
impl StdinLock<'_> {
    pub(crate) fn as_mut_buf(&mut self) -> &mut BufReader<impl Read> {
        &mut self.inner
    }
}

impl Read for StdinLock<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        self.inner.read_vectored(bufs)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        self.inner.is_read_vectored()
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.inner.read_to_end(buf)
    }

    fn read_to_string(&mut self, buf: &mut String) -> io::Result<usize> {
        self.inner.read_to_string(buf)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.inner.read_exact(buf)
    }
}

impl BufRead for StdinLock<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, n: usize) {
        self.inner.consume(n)
    }

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.inner.read_until(byte, buf)
    }

    fn read_line(&mut self, buf: &mut String) -> io::Result<usize> {
        self.inner.read_line(buf)
    }
}

impl fmt::Debug for StdinLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdinLock").finish_non_exhaustive()
    }
}

/// A handle to the global standard output stream of the current process.
///
/// Each handle shares a global buffer of data to be written to the standard
/// output stream. Access is also synchronized via a lock and explicit control
/// over locking is available via the [`lock`] method.
///
/// Created by the [`io::stdout`] method.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
///
/// [`lock`]: Stdout::lock
/// [`io::stdout`]: stdout
pub struct Stdout {
    // FIXME: this should be LineWriter or BufWriter depending on the state of
    //        stdout (tty or not). Note that if this is not line buffered it
    //        should also flush-on-panic or some form of flush-on-abort.
    inner: Pin<&'static ReentrantMutex<RefCell<LineWriter<StdoutRaw>>>>,
}

/// A locked reference to the [`Stdout`] handle.
///
/// This handle implements the [`Write`] trait, and is constructed via
/// the [`Stdout::lock`] method. See its documentation for more.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
#[must_use = "if unused stdout will immediately unlock"]
pub struct StdoutLock<'a> {
    inner: ReentrantMutexGuard<'a, RefCell<LineWriter<StdoutRaw>>>,
}

static STDOUT: SyncOnceCell<ReentrantMutex<RefCell<LineWriter<StdoutRaw>>>> = SyncOnceCell::new();

/// Constructs a new handle to the standard output of the current process.
///
/// Each handle returned is a reference to a shared global buffer whose access
/// is synchronized via a mutex. If you need more explicit control over
/// locking, see the [`Stdout::lock`] method.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// Using implicit synchronization:
///
/// ```no_run
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     io::stdout().write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
///
/// Using explicit synchronization:
///
/// ```no_run
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     let stdout = io::stdout();
///     let mut handle = stdout.lock();
///
///     handle.write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
#[must_use]
pub fn stdout() -> Stdout {
    Stdout {
        inner: Pin::static_ref(&STDOUT).get_or_init_pin(
            || ReentrantMutex::new(RefCell::new(LineWriter::new(stdout_raw()))),
            |_| (),
        ),
    }
}

/// Constructs a new locked handle to the standard output of the current
/// process.
///
/// Each handle returned is a guard granting locked access to a shared
/// global buffer whose access is synchronized via a mutex. If you need
/// more explicit control over locking, for example, in a multi-threaded
/// program, use the [`io::stdout`] function to obtain an unlocked handle,
/// along with the [`Stdout::lock`] method.
///
/// The lock is released when the returned guard goes out of scope. The
/// returned guard also implements the [`Write`] trait for writing data.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// ```no_run
/// #![feature(stdio_locked)]
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     let mut handle = io::stdout_locked();
///
///     handle.write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
pub fn stdout_locked() -> StdoutLock<'static> {
    stdout().into_locked()
}

pub fn cleanup() {
    if let Some(instance) = STDOUT.get() {
        // Flush the data and disable buffering during shutdown
        // by replacing the line writer by one with zero
        // buffering capacity.
        // We use try_lock() instead of lock(), because someone
        // might have leaked a StdoutLock, which would
        // otherwise cause a deadlock here.
        if let Some(lock) = Pin::static_ref(instance).try_lock() {
            *lock.borrow_mut() = LineWriter::with_capacity(0, stdout_raw());
        }
    }
}

impl Stdout {
    /// Locks this handle to the standard output stream, returning a writable
    /// guard.
    ///
    /// The lock is released when the returned lock goes out of scope. The
    /// returned guard also implements the `Write` trait for writing data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::io::{self, Write};
    ///
    /// fn main() -> io::Result<()> {
    ///     let stdout = io::stdout();
    ///     let mut handle = stdout.lock();
    ///
    ///     handle.write_all(b"hello world")?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn lock(&self) -> StdoutLock<'_> {
        self.lock_any()
    }

    // Locks this handle with any lifetime. This depends on the
    // implementation detail that the underlying `ReentrantMutex` is
    // static.
    fn lock_any<'a>(&self) -> StdoutLock<'a> {
        StdoutLock { inner: self.inner.lock() }
    }

    /// Consumes this handle to the standard output stream, locking the
    /// shared global buffer associated with the stream and returning a
    /// writable guard.
    ///
    /// The lock is released when the returned lock goes out of scope. The
    /// returned guard also implements the [`Write`] trait for writing data.
    ///
    /// It is often simpler to directly get a locked handle using the
    /// [`io::stdout_locked`] function instead, unless nearby code also
    /// needs to use an unlocked handle.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// #![feature(stdio_locked)]
    /// use std::io::{self, Write};
    ///
    /// fn main() -> io::Result<()> {
    ///     let mut handle = io::stdout().into_locked();
    ///
    ///     handle.write_all(b"hello world")?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn into_locked(self) -> StdoutLock<'static> {
        self.lock_any()
    }
}

impl fmt::Debug for Stdout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stdout").finish_non_exhaustive()
    }
}

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        io::Write::is_write_vectored(&&*self)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        (&*self).write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        (&*self).write_all_vectored(bufs)
    }
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        (&*self).write_fmt(args)
    }
}

impl Write for &Stdout {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.lock().is_write_vectored()
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.lock().write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.lock().write_all_vectored(bufs)
    }
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.lock().write_fmt(args)
    }
}

impl Write for StdoutLock<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.borrow_mut().write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.borrow_mut().write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.borrow_mut().is_write_vectored()
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.borrow_mut().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.borrow_mut().write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.inner.borrow_mut().write_all_vectored(bufs)
    }
}

impl fmt::Debug for StdoutLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdoutLock").finish_non_exhaustive()
    }
}

/// A handle to the standard error stream of a process.
///
/// For more information, see the [`io::stderr`] method.
///
/// [`io::stderr`]: stderr
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
pub struct Stderr {
    inner: Pin<&'static ReentrantMutex<RefCell<StderrRaw>>>,
}

/// A locked reference to the [`Stderr`] handle.
///
/// This handle implements the [`Write`] trait and is constructed via
/// the [`Stderr::lock`] method. See its documentation for more.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
#[must_use = "if unused stderr will immediately unlock"]
pub struct StderrLock<'a> {
    inner: ReentrantMutexGuard<'a, RefCell<StderrRaw>>,
}

/// Constructs a new handle to the standard error of the current process.
///
/// This handle is not buffered.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
///
/// # Examples
///
/// Using implicit synchronization:
///
/// ```no_run
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     io::stderr().write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
///
/// Using explicit synchronization:
///
/// ```no_run
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     let stderr = io::stderr();
///     let mut handle = stderr.lock();
///
///     handle.write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
#[must_use]
pub fn stderr() -> Stderr {
    // Note that unlike `stdout()` we don't use `at_exit` here to register a
    // destructor. Stderr is not buffered , so there's no need to run a
    // destructor for flushing the buffer
    static INSTANCE: SyncOnceCell<ReentrantMutex<RefCell<StderrRaw>>> = SyncOnceCell::new();

    Stderr {
        inner: Pin::static_ref(&INSTANCE).get_or_init_pin(
            || ReentrantMutex::new(RefCell::new(stderr_raw())),
            |_| (),
        ),
    }
}

/// Constructs a new locked handle to the standard error of the current
/// process.
///
/// This handle is not buffered.
///
/// ### Note: Windows Portability Consideration
/// When operating in a console, the Windows implementation of this stream does not support
/// non-UTF-8 byte sequences. Attempting to write bytes that are not valid UTF-8 will return
/// an error.
///
/// # Example
///
/// ```no_run
/// #![feature(stdio_locked)]
/// use std::io::{self, Write};
///
/// fn main() -> io::Result<()> {
///     let mut handle = io::stderr_locked();
///
///     handle.write_all(b"hello world")?;
///
///     Ok(())
/// }
/// ```
pub fn stderr_locked() -> StderrLock<'static> {
    stderr().into_locked()
}

impl Stderr {
    /// Locks this handle to the standard error stream, returning a writable
    /// guard.
    ///
    /// The lock is released when the returned lock goes out of scope. The
    /// returned guard also implements the [`Write`] trait for writing data.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::{self, Write};
    ///
    /// fn foo() -> io::Result<()> {
    ///     let stderr = io::stderr();
    ///     let mut handle = stderr.lock();
    ///
    ///     handle.write_all(b"hello world")?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn lock(&self) -> StderrLock<'_> {
        self.lock_any()
    }

    // Locks this handle with any lifetime. This depends on the
    // implementation detail that the underlying `ReentrantMutex` is
    // static.
    fn lock_any<'a>(&self) -> StderrLock<'a> {
        StderrLock { inner: self.inner.lock() }
    }

    /// Locks and consumes this handle to the standard error stream,
    /// returning a writable guard.
    ///
    /// The lock is released when the returned guard goes out of scope. The
    /// returned guard also implements the [`Write`] trait for writing
    /// data.
    ///
    /// # Examples
    ///
    /// ```
    /// #![feature(stdio_locked)]
    /// use std::io::{self, Write};
    ///
    /// fn foo() -> io::Result<()> {
    ///     let stderr = io::stderr();
    ///     let mut handle = stderr.into_locked();
    ///
    ///     handle.write_all(b"hello world")?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub fn into_locked(self) -> StderrLock<'static> {
        self.lock_any()
    }
}

impl fmt::Debug for Stderr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stderr").finish_non_exhaustive()
    }
}

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        io::Write::is_write_vectored(&&*self)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        (&*self).write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        (&*self).write_all_vectored(bufs)
    }
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        (&*self).write_fmt(args)
    }
}

impl Write for &Stderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.lock().write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.lock().is_write_vectored()
    }
    fn flush(&mut self) -> io::Result<()> {
        self.lock().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.lock().write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.lock().write_all_vectored(bufs)
    }
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> io::Result<()> {
        self.lock().write_fmt(args)
    }
}

impl Write for StderrLock<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.borrow_mut().write(buf)
    }
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.inner.borrow_mut().write_vectored(bufs)
    }
    #[inline]
    fn is_write_vectored(&self) -> bool {
        self.inner.borrow_mut().is_write_vectored()
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.borrow_mut().flush()
    }
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.borrow_mut().write_all(buf)
    }
    fn write_all_vectored(&mut self, bufs: &mut [IoSlice<'_>]) -> io::Result<()> {
        self.inner.borrow_mut().write_all_vectored(bufs)
    }
}

impl fmt::Debug for StderrLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StderrLock").finish_non_exhaustive()
    }
}

/// Write `args` to the capture buffer if enabled and possible, or `global_s`
/// otherwise. `label` identifies the stream in a panic message.
///
/// This function is used to print error messages, so it takes extra
/// care to avoid causing a panic when `local_s` is unusable.
/// For instance, if the TLS key for the local stream is
/// already destroyed, or if the local stream is locked by another
/// thread, it will just fall back to the global stream.
///
/// However, if the actual I/O causes an error, this function does panic.
fn print_to<T>(args: fmt::Arguments<'_>, global_s: fn() -> T, label: &str)
where
    T: Write,
{
    if let Err(e) = global_s().write_fmt(args) {
        panic!("failed printing to {}: {}", label, e);
    }
}

pub fn _print(args: fmt::Arguments<'_>) {
    print_to(args, stdout, "stdout");
}

pub fn _eprint(args: fmt::Arguments<'_>) {
    print_to(args, stderr, "stderr");
}
