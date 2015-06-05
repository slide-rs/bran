// Copyright 2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// #![license = "MIT/ASL2"]
#![doc(html_logo_url = "http://www.rust-lang.org/logos/rust-logo-128x128-blk-v2.png",
       html_favicon_url = "http://www.rust-lang.org/favicon.ico")]

#![allow(unused_features)]
#![feature(std_misc, libc, asm, core, alloc, test, unboxed_closures, page_size)]
#![feature(rustc_private)]
#![feature(unique)]

#[macro_use] extern crate log;
extern crate libc;
extern crate test;
extern crate mmap;

pub use builder::Builder;
pub use fiber::{Fiber, Handle, ResumeResult};

mod context;
pub mod fiber;

pub mod builder;
mod stack;
mod thunk; // use self-maintained thunk, because std::thunk is temporary. May be replaced by FnBox in the future.
mod sys;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod benchmarks;

/// Spawn a new Fiber
///
/// Equavalent to `Fiber::spawn`.
pub fn spawn<F>(f: F) -> Handle
    where F: FnOnce() + Send + 'static
{
    Builder::new().spawn(f)
}

/// Get the current Fiber
///
/// Equavalent to `Fiber::current`.
pub fn current() -> &'static Handle {
    Fiber::current()
}

/// Resume a Fiber
///
/// Equavalent to `Fiber::resume`.
pub fn resume(coro: &Handle) -> ResumeResult<()> {
    coro.resume()
}

/// Yield the current Fiber
///
/// Equavalent to `Fiber::sched`.
pub fn sched() {
    Fiber::sched()
}
