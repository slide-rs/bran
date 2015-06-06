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
use std::ptr::{self, Unique};
use std::fmt::{self, Debug};
use std::boxed;

use pulse::{self, Signal};

use context::Context;
use stack::{Stack};

/// State of a Fiber
#[derive(Debug, Clone)]
pub enum State {
    /// Pending its child to return
    Pending(Signal),

    /// Time
    PendingTimeout(Signal, u32),

    /// Finished
    Finished,

    /// Panic happened inside, cannot be resumed again
    Panicked,
}

impl State {
    pub fn is_pending(&self) -> bool {
        match self {
            &State::Pending(_) => true,
            &State::PendingTimeout(_, _) => true,
            _ => false
        }
    }

    pub fn is_panic(&self) -> bool {
        match self {
            &State::Panicked => true,
            _ => false
        }
    }

    pub fn is_finished(&self) -> bool {
        match self {
            &State::Finished => true,
            _ => false
        }
    }
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

    unsafe fn get_inner(&self) -> &Fiber {
        & *self.0.get()
    }

    pub fn run(&self) -> State {
        // Only run if the signal is set
        match self.state {
            State::Pending(ref sig) | State::PendingTimeout(ref sig, _) => {
                if !sig.is_pending() {
                    let mut ctx = Parent{
                        context: Context::empty(),
                        running: *self.0
                    };
                    PARENT_CONTEXT.with(|pctx| {
                        unsafe { *pctx.get() = &mut ctx as *mut Parent; }
                    });
                    pulse::with_scheduler(|| { unsafe {
                        Context::swap(&mut ctx.context, &(**self.0).saved_context);
                    }}, Box::new(Resume));
                }
            }
            State::Finished | State::Panicked => ()
        }
        self.state.clone()
    }

    /// Get the state of the Fiber
    #[inline]
    pub fn state(&self) -> State {
        unsafe {
            self.get_inner().state()
        }
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

/// Initialization function for make context
extern "C" fn coroutine_initialize(_: usize, f: *mut ()) -> ! {
    let func: Box<Thunk> = unsafe { transmute(f) };

    let ret = unsafe { try(move|| func.invoke(())) };

    let state = match ret {
        Ok(..) => State::Finished,
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

                let _ = writeln!(&mut stderr(), "Fiber panicked at '{}'", msg);
            }
            State::Panicked
        }
    };

    Fiber::yield_now(state);
    unreachable!()
}

impl Fiber {
    fn new(name: Option<String>, stack: Stack, ctx: Context, state: State) -> Handle {
        Handle::new(Fiber {
            current_stack_segment: Some(stack),
            saved_context: ctx,
            state: state,
            name: name,
        })
    }

    fn yield_now(state: State) {
        let parent: &mut Parent = PARENT_CONTEXT.with(|pctx| {
            unsafe { transmute(*pctx.get()) }
        });
        unsafe {
            (*parent.running).state = state;
            Context::swap(&mut (*parent.running).saved_context, &parent.context);
        }
    }

    /// Spawn a Fiber with options
    pub fn spawn_opts<F>(f: F, opts: Options) -> Handle
        where F: FnOnce() + Send + 'static
    {
        let mut stack = Stack::new(2*1024*1024);
        let ctx = Context::new(coroutine_initialize, 0, f, &mut stack);
        Fiber::new(opts.name, stack, ctx, State::Pending(Signal::pulsed()))
    }

    /// Spawn a Fiber with default options
    pub fn spawn<F>(f: F) -> Handle
        where F: FnOnce() + Send + 'static
    {
        Fiber::spawn_opts(f, Default::default())
    }

    #[inline(always)]
    fn state(&self) -> State {
        self.state.clone()
    }

    /// Get the name of the Fiber
    #[inline(always)]
    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|s| &**s)
    }

    /// Determines whether the current Fiber is unwinding because of panic.
    #[inline(always)]
    pub fn panicking(&self) -> bool {
        match self.state() {
            State::Panicked => true,
            _ => false
        }
    }

    /// Determines whether the Fiber is finished
    #[inline(always)]
    pub fn finished(&self) -> bool {
        match self.state() {
            State::Finished => true,
            _ => false
        }
    }
}

struct Parent {
    context: Context,
    running: *mut Fiber
}

thread_local!(static PARENT_CONTEXT: UnsafeCell<*mut Parent> = UnsafeCell::new(ptr::null_mut()));


/// This is the `default` system scheduler that is used if no
/// user provided scheduler is installed. It is very basic
/// and will block the OS thread using `thread::park`
#[derive(Debug)]
pub struct Resume;

impl pulse::Scheduler for Resume {
    fn wait(&self, signal: Signal) -> Result<(), pulse::WaitError> {
        loop {
            match signal.state() {
                pulse::SignalState::Pending => {
                    Fiber::yield_now(State::Pending(signal.clone()));
                }
                pulse::SignalState::Pulsed => return Ok(()),
                pulse::SignalState::Dropped => return Err(pulse::WaitError::Dropped)
            }
        }
    }

    fn wait_timeout_ms(&self, _: Signal, _: u32) -> Result<(), pulse::TimeoutError> {
        panic!("unsupported yet")
    }
}