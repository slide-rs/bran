// Copyright 2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::ptr;
use std::env::{page_size};
use std::fmt;
use std::sync::{Mutex, Arc};

use libc;

use mmap::{MemoryMap, MapOption};

/// A task's stack. The name "Stack" is a vestige of segmented stacks.
pub struct Stack {
    buf: Option<MemoryMap>,
    min_size: usize,
    pool: Option<StackPool>
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "Stack {} buf: ", "{"));
        match self.buf {
            Some(ref map) => try!(write!(f, "Some({:#x}), ", map.data() as libc::uintptr_t)),
            None => try!(write!(f, "None, ")),
        }
        write!(f, "min_size: {:?} {}", self.min_size, "}")
    }
}

// Try to use MAP_STACK on platforms that support it (it's what we're doing
// anyway), but some platforms don't support it at all. For example, it appears
// that there's a bug in freebsd that MAP_STACK implies MAP_FIXED (so it always
// fails): http://lists.freebsd.org/pipermail/freebsd-bugs/2011-July/044840.html
//
// DragonFly BSD also seems to suffer from the same problem. When MAP_STACK is
// used, it returns the same `ptr` multiple times.
#[cfg(all(not(windows), not(target_os = "freebsd"), not(target_os = "dragonfly")))]
static STACK_FLAGS: libc::c_int = libc::MAP_STACK | libc::MAP_PRIVATE | libc::MAP_ANON;
#[cfg(any(target_os = "freebsd",
          target_os = "dragonfly"))]
static STACK_FLAGS: libc::c_int = libc::MAP_PRIVATE | libc::MAP_ANON;
#[cfg(windows)]
static STACK_FLAGS: libc::c_int = 0;

impl Stack {
    /// Allocate a new stack of `size`. If size = 0, this will fail. Use
    /// `dummy_stack` if you want a zero-sized stack.
    pub fn new(size: usize) -> Stack {
        // Map in a stack. Eventually we might be able to handle stack
        // allocation failure, which would fail to spawn the task. But there's
        // not many sensible things to do on OOM.  Failure seems fine (and is
        // what the old stack allocation did).
        let stack = match MemoryMap::new(size, &[MapOption::MapReadable,
                                                 MapOption::MapWritable,
                                                 MapOption::MapNonStandardFlags(STACK_FLAGS)]) {
            Ok(map) => map,
            Err(e) => panic!("mmap for stack of size {} failed: {}", size, e)
        };

        // Change the last page to be inaccessible. This is to provide safety;
        // when an FFI function overflows it will (hopefully) hit this guard
        // page. It isn't guaranteed, but that's why FFI is unsafe. buf.data is
        // guaranteed to be aligned properly.
        if !protect_last_page(&stack) {
            panic!("Could not memory-protect guard page. stack={:?}",
                  stack.data());
        }

        Stack {
            buf: Some(stack),
            min_size: size,
            pool: None
        }
    }

    /// Create a 0-length stack which starts (and ends) at 0.
    #[allow(dead_code)]
    pub unsafe fn dummy_stack() -> Stack {
        Stack {
            buf: None,
            min_size: 0,
            pool: None
        }
    }

    #[allow(dead_code)]
    pub fn guard(&self) -> *const usize {
        (self.start() as usize + page_size()) as *const usize
    }

    /// Point to the low end of the allocated stack
    pub fn start(&self) -> *const usize {
        self.buf.as_ref()
            .map(|m| m.data() as *const usize)
            .unwrap_or(ptr::null())
    }

    /// Point one usize beyond the high end of the allocated stack
    pub fn end(&self) -> *const usize {
        self.buf
            .as_ref()
            .map(|buf| unsafe {
                buf.data().offset(buf.len() as isize) as *const usize
            })
            .unwrap_or(ptr::null())
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        match (self.buf.take(), self.pool.clone().take()) {
            (Some(s), Some(p)) => p.give_stack(Stack {
                buf: Some(s),
                min_size: self.min_size,
                pool: self.pool.take()
            }),
            _ => ()
        }
    }
}

unsafe impl Send for Stack {}


#[cfg(unix)]
fn protect_last_page(stack: &MemoryMap) -> bool {
    unsafe {
        // This may seem backwards: the start of the segment is the last page?
        // Yes! The stack grows from higher addresses (the end of the allocated
        // block) to lower addresses (the start of the allocated block).
        let last_page = stack.data() as *mut libc::c_void;
        libc::mprotect(last_page, page_size() as libc::size_t,
                       libc::PROT_NONE) != -1
    }
}

#[cfg(windows)]
fn protect_last_page(stack: &MemoryMap) -> bool {
    unsafe {
        // see above
        let last_page = stack.data() as *mut libc::c_void;
        let mut old_prot: libc::DWORD = 0;
        libc::VirtualProtect(last_page, page_size() as libc::SIZE_T,
                             libc::PAGE_NOACCESS,
                             &mut old_prot as libc::LPDWORD) != 0
    }
}

#[derive(Debug)]
struct InnerPool {
    // Ideally this would be some data structure that preserved ordering on
    // Stack.min_size.
    stacks: Vec<Stack>,    
}

#[derive(Debug, Clone)]
pub struct StackPool(Arc<Mutex<InnerPool>>);

impl StackPool {
    pub fn new() -> StackPool {
        StackPool(Arc::new(Mutex::new(InnerPool{
            stacks: vec![],
        })))
    }

    pub fn take_stack(self, min_size: usize) -> Stack {
        let mut stack = {
            let mut pool = self.0.lock().unwrap();

            // Ideally this would be a binary search
            pool.stacks.iter()
                .position(|s| min_size <= s.min_size)
                .map(|idx| pool.stacks.swap_remove(idx))
        }.unwrap_or_else(|| Stack::new(min_size));

        stack.pool = Some(self);
        stack
    }

    pub fn give_stack(&self, mut stack: Stack) {
        let mut pool = self.0.lock().unwrap();
        stack.pool = None;

        if pool.stacks.len() < 256 {
            pool.stacks.push(stack);
        }
    }
}
