#![feature(scoped)]
extern crate bran;
extern crate num_cpus;

use std::thread;

use bran::{spawn, sched};

fn main() {

    let mut threads = Vec::with_capacity(num_cpus::get());

    for i in 0..num_cpus::get() {
        let t = thread::scoped(move|| {
            spawn(move|| {
                let thread_id = i;
                spawn(move|| {
                    for count in 0..10 {
                        println!("Coroutine running in thread {}: counting {}", thread_id, count);
                        sched();
                    }
                }).join().ok().expect("Failed to join");
            }).join().ok().expect("Failed to join");
        });
        threads.push(t);
    }

    for t in threads.into_iter() {
        t.join();
    }
}
