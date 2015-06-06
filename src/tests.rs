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
use pulse::Signal;

#[test]
fn test_fiber_basic() {
    let (tx, rx) = channel();
    Fiber::spawn(move|| {
        tx.send(1).unwrap();
    }).run();

    assert_eq!(rx.recv().unwrap(), 1);
}

#[test]
fn test_fiber_yield() {
    let (s0, p0) = Signal::new();
    let (s1, p1) = Signal::new();
    let (s2, p2) = Signal::new();

    let fiber = Fiber::spawn(move|| {
        p0.pulse();
        s1.wait().unwrap();
        p2.pulse();
    });
    assert!(s0.is_pending());
    assert!(fiber.run().is_pending());
    assert!(!s0.is_pending());
    p1.pulse();

    assert!(s2.is_pending());
    assert!(fiber.run().is_finished());
    assert!(!s2.is_pending());
}

#[test]
fn test_fiber_panic() {
    let fiber = Fiber::spawn(move|| {
        panic!("Panic inside a fiber!!");
    });
    assert!(fiber.run().is_panic());
}

#[test]
fn test_fiber_run_after_finished() {
    let fiber = Fiber::spawn(move|| {});

    // It is already finished, but we try to run it
    // Idealy it would come back immediately
    assert!(fiber.run().is_finished());

    // Again?
    assert!(fiber.run().is_finished());
}

