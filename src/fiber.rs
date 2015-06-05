// The MIT License (MIT)

// Copyright (c) 2015 Rustcc developers

// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files (the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so,
// subject to the following conditions:

// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
// FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
// COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
// IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
// CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

//! Basic single threaded Fiber
//!
//! ```rust
//! use bran::{spawn, sched};
//!
//! let coro = spawn(|| {
//!     println!("Before yield");
//!
//!     // Yield back to its parent who resume this coroutine
//!     sched();
//!
//!     println!("I am back!");
//! });
//!
//! // Starts the Fiber
//! coro.resume().ok().expect("Failed to resume");
//!
//! println!("Back to main");
//!
//! // Resume it
//! coro.resume().ok().expect("Failed to resume");
//!
//! println!("Fiber finished");
//! ```
//!

/* Here is the coroutine(with scheduler) workflow:
 *
 *                               --------------------------------
 * --------------------------    |                              |
 * |                        |    v                              |
 * |                  ----------------                          |  III.Fiber::yield_now()
 * |             ---> |   Scheduler  |  <-----                  |
 * |    parent   |    ----------------       |   parent         |
 * |             |           ^ parent        |                  |
 * |   --------------  --------------  --------------           |
 * |   |Fiber(1)    |  |Fiber(2)    |  |Fiber(3)    |  ----------
 * |   --------------  --------------  --------------
 * |         ^            |     ^
 * |         |            |     |  II.do_some_works
 * -----------            -------
 *   I.Handle.resume()
 *
 *
 *  First, all coroutines have a link to a parent coroutine, which was set when the coroutine resumed.
 *  In the scheduler/coroutine model, every worker coroutine has a parent pointer pointing to
 *  the scheduler coroutine(which is a raw thread).
 *  Scheduler resumes a proper coroutine and set the parent pointer, like procedure I does.
 *  When a coroutine is awaken, it does some work like procedure II does.
 *  When a coroutine yield(io, finished, paniced or sched), it resumes its parent's context,
 *  like procedure III does.
 *  Now the scheduler is awake again and it simply decides whether to put the coroutine to queue again or not,
 *  according to the coroutine's return status.
 *  And last, the scheduler continues the scheduling loop and selects a proper coroutine to wake up.
 */

use std::default::Default;
use std::rt::util::min_stack;
use thunk::Thunk;
use std::mem::transmute;
use std::rt::unwind::try;
use std::any::Any;
use std::cell::UnsafeCell;
use std::ops::Deref;
use std::ptr::Unique;
use std::fmt::{self, Debug};
use std::boxed;

use context::Context;
use stack::{StackPool, Stack};

/// State of a Fiber
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum State {
    /// Waiting its child to return
    Normal,

    /// Suspended. Can be waked up by `resume`
    Suspended,

    /// Blocked. Can be waked up by `resume`
    Blocked,

    /// Running
    Running,

    /// Finished
    Finished,

    /// Panic happened inside, cannot be resumed again
    Panicked,
}

/// Return type of resuming.
///
/// See `Fiber::resume` for more detail
pub type ResumeResult<T> = Result<T, Box<Any + Send>>;

/// Fiber spawn options
#[derive(Debug)]
pub struct Options {
    /// The size of the stack
    pub stack_size: usize,

    /// The name of the Fiber
    pub name: Option<String>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            stack_size: min_stack(),
            name: None,
        }
    }
}

/// Handle of a Fiber
pub struct Handle(Unique<Fiber>);

impl Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        unsafe {
            self.get_inner().name().fmt(f)
        }
    }
}

unsafe impl Send for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let p = Box::from_raw(*self.0);
            drop(p);
        }
    }
}

impl Handle {
    fn new(c: Fiber) -> Handle {
        unsafe {
            Handle(Unique::new(boxed::into_raw(Box::new(c))))
        }
    }

    unsafe fn get_inner_mut(&self) -> &mut Fiber {
        &mut **self.0
    }

    unsafe fn get_inner(&self) -> &Fiber {
        & *self.0.get()
    }

    /// Resume the Fiber
    pub fn resume(&self) -> ResumeResult<()> {
        match self.state() {
            State::Finished | State::Running => return Ok(()),
            State::Panicked => panic!("Trying to resume a panicked coroutine"),
            State::Normal => panic!("Fiber {:?} is waiting for its child to return, cannot resume!",
                                    self.name().unwrap_or("<unnamed>")),
            _ => {}
        }

        let env = Environment::current();

        let from_coro_hdl = Fiber::current();
        {
            let (from_coro, to_coro) = unsafe {
                (from_coro_hdl.get_inner_mut(), self.get_inner_mut())
            };

            // Save state
            to_coro.set_state(State::Running);
            from_coro.set_state(State::Normal);

            env.coroutine_stack.push(unsafe { transmute(self) });
            Context::swap(&mut from_coro.saved_context, &to_coro.saved_context);

            from_coro.set_state(State::Running);
        }

        match env.running_state.take() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Join this Fiber.
    ///
    /// If the Fiber panicked, this method will return an `Err` with panic message.
    ///
    /// ```ignore
    /// // Wait until the Fiber exits
    /// spawn(|| {
    ///     println!("Before yield");
    ///     sched();
    ///     println!("Exiting");
    /// }).join().unwrap();
    /// ```
    #[inline]
    pub fn join(&self) -> ResumeResult<()> {
        loop {
            match self.state() {
                State::Finished | State::Panicked => break,
                _ => try!(self.resume()),
            }
        }
        Ok(())
    }

    /// Get the state of the Fiber
    #[inline]
    pub fn state(&self) -> State {
        unsafe {
            self.get_inner().state()
        }
    }

    /// Set the state of the Fiber
    #[inline]
    fn set_state(&self, state: State) {
        unsafe { self.get_inner_mut().set_state(state) }
    }
}

impl Deref for Handle {
    type Target = Fiber;

    #[inline]
    fn deref(&self) -> &Fiber {
        unsafe { self.get_inner() }
    }
}

/// A coroutine is nothing more than a (register context, stack) pair.
#[allow(raw_pointer_derive)]
#[derive(Debug)]
pub struct Fiber {
    /// The segment of stack on which the task is currently running or
    /// if the task is blocked, on which the task will resume
    /// execution.
    current_stack_segment: Option<Stack>,

    /// Always valid if the task is alive and not running.
    saved_context: Context,

    /// State
    state: State,

    /// Name
    name: Option<String>,
}

unsafe impl Send for Fiber {}

/// Destroy coroutine and try to reuse std::stack segment.
impl Drop for Fiber {
    fn drop(&mut self) {
        match self.current_stack_segment.take() {
            Some(stack) => {
                let env = Environment::current();
                env.stack_pool.give_stack(stack);
            },
            None => {}
        }
    }
}

/// Initialization function for make context
extern "C" fn coroutine_initialize(_: usize, f: *mut ()) -> ! {
    let func: Box<Thunk> = unsafe { transmute(f) };

    let ret = unsafe { try(move|| func.invoke(())) };

    let env = Environment::current();

    let cur: &mut Fiber = unsafe {
        let last = & **env.coroutine_stack.last().expect("Impossible happened! No current coroutine!");
        last.get_inner_mut()
    };

    let state = match ret {
        Ok(..) => {
            env.running_state = None;

            State::Finished
        }
        Err(err) => {
            {
                use std::io::stderr;
                use std::io::Write;
                let msg = match err.downcast_ref::<&'static str>() {
                    Some(s) => *s,
                    None => match err.downcast_ref::<String>() {
                        Some(s) => &s[..],
                        None => "Box<Any>",
                    }
                };

                let name = cur.name().unwrap_or("<unnamed>");

                let _ = writeln!(&mut stderr(), "Fiber '{}' panicked at '{}'", name, msg);
            }

            env.running_state = Some(err);

            State::Panicked
        }
    };

    loop {
        Fiber::yield_now(state);
    }
}

impl Fiber {
    unsafe fn empty(name: Option<String>, state: State) -> Handle {
        Handle::new(Fiber {
            current_stack_segment: None,
            saved_context: Context::empty(),
            state: state,
            name: name,
        })
    }

    fn new(name: Option<String>, stack: Stack, ctx: Context, state: State) -> Handle {
        Handle::new(Fiber {
            current_stack_segment: Some(stack),
            saved_context: ctx,
            state: state,
            name: name,
        })
    }

    /// Spawn a Fiber with options
    pub fn spawn_opts<F>(f: F, opts: Options) -> Handle
        where F: FnOnce() + Send + 'static
    {

        let env = Environment::current();
        let mut stack = env.stack_pool.take_stack(opts.stack_size);

        let ctx = Context::new(coroutine_initialize, 0, f, &mut stack);

        Fiber::new(opts.name, stack, ctx, State::Suspended)
    }

    /// Spawn a Fiber with default options
    pub fn spawn<F>(f: F) -> Handle
        where F: FnOnce() + Send + 'static
    {
        Fiber::spawn_opts(f, Default::default())
    }

    /// Yield the current running Fiber to its parent
    #[inline]
    pub fn yield_now(state: State) {
        // Cannot yield with Running state
        assert!(state != State::Running);

        let env = Environment::current();
        if env.coroutine_stack.len() == 1 {
            // Environment root
            return;
        }

        unsafe {
            match (env.coroutine_stack.pop(), env.coroutine_stack.last()) {
                (Some(from_coro), Some(to_coro)) => {
                    (&mut *from_coro).set_state(state);
                    Context::swap(&mut (& *from_coro).get_inner_mut().saved_context, &(& **to_coro).saved_context);
                },
                _ => unreachable!()
            }
        }
    }

    /// Yield the current running Fiber with `Suspended` state
    #[inline]
    pub fn sched() {
        Fiber::yield_now(State::Suspended)
    }

    /// Yield the current running Fiber with `Blocked` state
    #[inline]
    pub fn block() {
        Fiber::yield_now(State::Blocked)
    }

    /// Get a Handle to the current running Fiber.
    ///
    /// It is unsafe because it is an undefined behavior if you resume a Fiber
    /// in more than one native thread.
    #[inline]
    pub fn current() -> &'static Handle {
        Environment::current().coroutine_stack.last().map(|hdl| unsafe { (& **hdl) })
            .expect("Impossible happened! No current coroutine!")
    }

    #[inline(always)]
    fn state(&self) -> State {
        self.state
    }

    #[inline(always)]
    fn set_state(&mut self, state: State) {
        self.state = state
    }

    /// Get the name of the Fiber
    #[inline(always)]
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|s| &**s)
    }

    /// Determines whether the current Fiber is unwinding because of panic.
    #[inline(always)]
    pub fn panicking(&self) -> bool {
        self.state() == State::Panicked
    }

    /// Determines whether the Fiber is finished
    #[inline(always)]
    pub fn finished(&self) -> bool {
        self.state() == State::Finished
    }
}

thread_local!(static COROUTINE_ENVIRONMENT: UnsafeCell<Box<Environment>> = UnsafeCell::new(Environment::new()));

/// Fiber managing environment
#[allow(raw_pointer_derive)]
struct Environment {
    stack_pool: StackPool,

    coroutine_stack: Vec<*mut Handle>,
    _main_coroutine: Handle,

    running_state: Option<Box<Any + Send>>,
}

impl Environment {
    /// Initialize a new environment
    fn new() -> Box<Environment> {
        let coro = unsafe {
            let coro = Fiber::empty(Some("<Environment Root Fiber>".to_string()), State::Running);
            coro
        };

        let mut env = Box::new(Environment {
            stack_pool: StackPool::new(),

            coroutine_stack: Vec::new(),
            _main_coroutine: coro,

            running_state: None,
        });

        let coro: *mut Handle = &mut env._main_coroutine;
        env.coroutine_stack.push(coro);

        env
    }

    fn current() -> &'static mut Environment {
        COROUTINE_ENVIRONMENT.with(|env| unsafe {
            &mut *env.get()
        })
    }
}
