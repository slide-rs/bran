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

use std::sync::mpsc::channel;

use fiber::Fiber;

#[test]
fn test_fiber_basic() {
    let (tx, rx) = channel();
    Fiber::spawn(move|| {
        tx.send(1).unwrap();
    }).resume().ok().expect("Failed to resume");

    assert_eq!(rx.recv().unwrap(), 1);
}

#[test]
fn test_fiber_yield() {
    let (tx, rx) = channel();
    let coro = Fiber::spawn(move|| {
        tx.send(1).unwrap();

        Fiber::sched();

        tx.send(2).unwrap();
    });
    coro.resume().ok().expect("Failed to resume");
    assert_eq!(rx.recv().unwrap(), 1);
    assert!(rx.try_recv().is_err());

    coro.resume().ok().expect("Failed to resume");

    assert_eq!(rx.recv().unwrap(), 2);
}

#[test]
fn test_fiber_spawn_inside() {
    let (tx, rx) = channel();
    Fiber::spawn(move|| {
        tx.send(1).unwrap();

        Fiber::spawn(move|| {
            tx.send(2).unwrap();
        }).join().ok().expect("Failed to join");

    }).join().ok().expect("Failed to join");

    assert_eq!(rx.recv().unwrap(), 1);
    assert_eq!(rx.recv().unwrap(), 2);
}

#[test]
fn test_fiber_panic() {
    let coro = Fiber::spawn(move|| {
        panic!("Panic inside a fiber!!");
    });
    assert!(coro.join().is_err());
}

#[test]
fn test_fiber_child_panic() {
    Fiber::spawn(move|| {
        let _ = Fiber::spawn(move|| {
            panic!("Panic inside a fiber's child!!");
        }).join();
    }).join().ok().expect("Failed to join");
}

#[test]
fn test_fiber_resume_after_finished() {
    let coro = Fiber::spawn(move|| {});

    // It is already finished, but we try to resume it
    // Idealy it would come back immediately
    assert!(coro.resume().is_ok());

    // Again?
    assert!(coro.resume().is_ok());
}

#[test]
fn test_fiber_resume_itself() {
    let coro = Fiber::spawn(move|| {
        // Resume itself
        Fiber::current().resume().ok().expect("Failed to resume");
    });

    assert!(coro.resume().is_ok());
}

#[test]
fn test_fiber_yield_in_main() {
    Fiber::sched();
}
